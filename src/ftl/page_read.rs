//! Page FTL read path, ported from `ftl/page/page-read.c`.
//!
//! [`read_page`] mirrors C's `page_ftl_read` and [`read_end_rq`] mirrors its
//! static `page_ftl_read_end_rq` completion callback. The C-ABI entry point is
//! [`ffi_page_ftl_read`].
//!
//! The flow matches C's. Resolve the logical page and the in-page offset, then
//! look the physical address up in the L2P table. An unmapped page is served
//! with zeros, and a read that runs past the page is rejected. A mapped read
//! copies the whole page into a scratch buffer through the device layer, then
//! waits on the request's condition variable. The completion callback copies
//! the requested subrange out.
//!
//! Deliberate deviations from C are listed in the [`crate::ftl`] module docs.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};
use std::io;

use crate::device::{
    device_address, device_request, ffi_device_alloc_request, ffi_device_free_request,
    DeviceDirection, DEVICE_DEFAULT_REQUEST, PADDR_EMPTY,
};
use crate::ftl::{PageFtl, PageFtlLock};
use crate::{pr_err, pr_warn};

/// Release the scratch page buffer and the internal read request that were
/// handed to the device but never completed.
///
/// C nulls both locals before its cleanup label on a negative `d_op->read`
/// (`ftl/page/page-read.c:121-127`) and leaks them. Keeping the reclaim here
/// keeps the `(page_ptr, page_size, page_size)` invariant in one place. A
/// `NULL` request is a no-op in [`ffi_device_free_request`], so the request
/// allocation failure path can use this too.
///
/// # Safety
/// `page_ptr` came from a `Vec<u8>` of `page_size` bytes that was forgotten,
/// and `read_rq` is either `NULL` or came from [`ffi_device_alloc_request`].
unsafe fn reclaim_scratch(page_ptr: *mut u8, page_size: usize, read_rq: *mut device_request) {
    drop(Vec::from_raw_parts(page_ptr, page_size, page_size));
    ffi_device_free_request(read_rq);
}

/// Copy the requested subrange out of the finished page read, release the
/// scratch buffer and the internal request, then signal the caller's request
/// finished. C's static `page_ftl_read_end_rq` does the same.
///
/// The device's completion path runs this while [`read_page`] still holds its
/// borrows, so it works entirely through raw pointers. It never forms a second
/// reference to the page FTL or to the caller's request.
///
/// # Safety
/// `read_rq` is a live request produced by [`read_page`]. Its `data` came from
/// a `Vec<u8>` of `data_len` bytes that was forgotten, so its length equals its
/// capacity. `rq_private` holds the live caller request, whose `data` points at
/// its own `data_len` writable bytes and stays valid until this returns.
unsafe extern "C" fn read_end_rq(read_rq: *mut device_request) {
    let request = (*read_rq).rq_private as *mut device_request;

    // `read_rq.data_len` is the whole page size, so the in-page offset is the
    // sector remainder against it.
    let page_size = (*read_rq).data_len;
    let offset = (*request).sector % page_size;
    let len = (*request).data_len;

    if len > 0 {
        let src = core::slice::from_raw_parts((*read_rq).data as *const u8, offset + len);
        let dst = core::slice::from_raw_parts_mut((*request).data as *mut u8, len);
        dst.copy_from_slice(&src[offset..offset + len]);
    }

    // C's `free(read_rq->data)`, then `device_free_request(read_rq)`
    // (`ftl/page/page-read.c:33-34`).
    drop(Vec::from_raw_parts(
        (*read_rq).data as *mut u8,
        page_size,
        page_size,
    ));
    ffi_device_free_request(read_rq);

    // C signals `request->cond` under `request->mutex` and stores `is_finish`
    // (`ftl/page/page-read.c:36-41`).
    let finish = AtomicI32::from_ptr(ptr::addr_of_mut!((*request).is_finish));
    let mutex = ptr::addr_of_mut!((*request).mutex);
    libc::pthread_mutex_lock(mutex);
    if finish.load(Ordering::SeqCst) == 0 {
        libc::pthread_cond_signal(ptr::addr_of_mut!((*request).cond));
    }
    finish.store(1, Ordering::SeqCst);
    libc::pthread_mutex_unlock(mutex);
}

/// Read `request.data_len` bytes at `request.sector`. C calls this
/// `page_ftl_read`.
///
/// An unmapped logical page is served by zero filling `request.data`, which is
/// what C does at `ftl/page/page-read.c:80-86`. A read that runs past the end
/// of its page fails with `EINVAL`. A device failure returns the device's
/// negated error code.
///
/// The request is borrowed, not consumed. Its `data` must point at
/// `data_len` writable bytes that stay valid for the duration of the call.
pub fn read_page(pgftl: &mut PageFtl, request: &mut device_request) -> io::Result<usize> {
    let page_size = pgftl.dev().page_size();
    let lpn = pgftl.lpn(request.sector);
    let offset = pgftl.page_offset(request.sector);

    // C reads `trans_map[lpn]` with no bound at all
    // (`ftl/page/page-read.c:71-78`), so a sector past the L2P table reads one
    // entry past its end. The port refuses it with the bound `update_map`
    // already applies. The C ABI mirror reports the same `-EINVAL`. See the
    // deviations in `crate::ftl`.
    let map_entries = pgftl.map_entries();
    if lpn >= map_entries {
        pr_err!("invalid lpn detected (lpn: {}, max: {})", lpn, map_entries);
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }

    let paddr = device_address::from(pgftl.trans_map[lpn]);

    if paddr.lpn == PADDR_EMPTY {
        pr_warn!("cannot find the mapping information (lpn: {})", lpn);
        let len = request.data_len;
        if let Some(buf) = unsafe { request.buffer_mut() } {
            buf.fill(0);
        }
        return Ok(len);
    }

    request.rq_private = ptr::from_mut(pgftl).cast::<c_void>();

    if offset + request.data_len > page_size {
        pr_err!(
            "overflow the read data (offset: {}, length: {})",
            offset,
            request.data_len
        );
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }

    let mut page = vec![0u8; page_size];
    let page_ptr = page.as_mut_ptr();
    core::mem::forget(page);

    // The native API borrows the request, so a request reused across reads can
    // still carry the previous completion. C frees the request on the zero fill
    // and success paths and never sets the flag on its error paths, so it can
    // never observe a stale one.
    unsafe {
        AtomicI32::from_ptr(ptr::addr_of_mut!(request.is_finish)).store(0, Ordering::SeqCst);
    }

    let read_rq = unsafe { ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST) };
    if read_rq.is_null() {
        pr_err!("request allocation failed");
        unsafe { reclaim_scratch(page_ptr, page_size, read_rq) };
        return Err(io::Error::from_raw_os_error(libc::ENOMEM));
    }

    unsafe {
        (*read_rq).flag = DeviceDirection::Read.into();
        (*read_rq).data = page_ptr as *mut c_void;
        (*read_rq).data_len = page_size;
        (*read_rq).paddr = paddr;
        (*read_rq).rq_private = request as *mut device_request as *mut c_void;
        (*read_rq).end_rq = Some(read_end_rq);
    }

    let data_len = request.data_len;
    let dev_ptr = pgftl.dev;
    let ret = unsafe {
        let ops = (*dev_ptr).d_op;
        let read = (*ops).read.expect("device backend has no read operation");
        read(dev_ptr, read_rq)
    };
    if ret < 0 {
        pr_err!("device read failed (ppn: {})", paddr);
        unsafe { reclaim_scratch(page_ptr, page_size, read_rq) };
        return Err(io::Error::from_raw_os_error((-ret) as i32));
    }

    // C waits on the request's condition variable (`page-read.c:129-133`).
    unsafe {
        let finish = AtomicI32::from_ptr(ptr::addr_of_mut!(request.is_finish));
        let mutex = ptr::addr_of_mut!(request.mutex);
        libc::pthread_mutex_lock(mutex);
        while finish.load(Ordering::SeqCst) == 0 {
            libc::pthread_cond_wait(ptr::addr_of_mut!(request.cond), mutex);
        }
        libc::pthread_mutex_unlock(mutex);
    }

    Ok(data_len)
}

/// The C-ABI entry point for [`read_page`], and the mirror of C's
/// `page_ftl_read`.
///
/// Frees the caller's request on success and on the zero-fill path, exactly
/// like C, and leaves it to the caller on the error paths.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with its `trans_map` and `dev`
/// initialized, and `request` is `NULL` or a live request whose `data` points
/// at `data_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_read(
    pgftl: *mut PageFtl,
    request: *mut device_request,
) -> isize {
    if pgftl.is_null() || request.is_null() {
        pr_err!("page FTL or request is NULL");
        return -(libc::EINVAL as isize);
    }

    let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
    match read_page(lock.get(), &mut *request) {
        Ok(len) => {
            ffi_device_free_request(request);
            len as isize
        }
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::ramdisk::ffi_ramdisk_close;
    use crate::device::{
        device, ffi_device_module_exit, ffi_device_module_init, DeviceModule, DeviceOperations,
        Request, DEVICE_PAGE_SIZE,
    };
    use crate::ftl::page_map::update_map;
    use core::ffi::{c_char, c_int};
    use core::mem::MaybeUninit;

    /// A real ramdisk device plus a page FTL L2P table over it.
    struct Fixture {
        dev: *mut device,
        pgftl: Box<PageFtl>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut dev: *mut device = ptr::null_mut();
            unsafe {
                assert_eq!(
                    ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                    0
                );
                let open = (*(*dev).d_op).open.expect("ramdisk vtable has open");
                assert_eq!(open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR), 0);
            }

            let entries = unsafe { (*dev).total_pages() + 1 };
            let mut pgftl = Box::new(PageFtl::empty());
            pgftl.dev = dev;
            pgftl.trans_map = vec![PADDR_EMPTY; entries];

            Fixture { dev, pgftl }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                assert_eq!(ffi_ramdisk_close(self.dev), 0);
                assert_eq!(ffi_device_module_exit(self.dev), 0);
            }
        }
    }

    /// A backend whose every operation fails, so the read path's device error
    /// branch can be exercised without a broken ramdisk.
    struct FailingRead;

    impl DeviceOperations for FailingRead {
        unsafe fn open(&self, _dev: *mut device, _name: *const c_char, _flags: c_int) -> c_int {
            0
        }

        unsafe fn write(&self, _dev: *mut device, _request: *mut device_request) -> isize {
            -(libc::EIO as isize)
        }

        unsafe fn read(&self, _dev: *mut device, _request: *mut device_request) -> isize {
            -(libc::EIO as isize)
        }

        unsafe fn erase(&self, _dev: *mut device, _request: *mut device_request) -> c_int {
            -libc::EIO
        }

        unsafe fn close(&self, _dev: *mut device) -> c_int {
            0
        }
    }

    /// A position dependent byte, so a copy from the wrong page or the wrong
    /// in-page offset cannot pass.
    fn pattern_byte(lpn: usize, offset: usize) -> u8 {
        ((lpn as u32).wrapping_mul(31).wrapping_add(offset as u32) & 0xff) as u8
    }

    /// The byte expected at absolute device byte `sector`.
    fn expected_byte(sector: usize) -> u8 {
        pattern_byte(sector / DEVICE_PAGE_SIZE, sector % DEVICE_PAGE_SIZE)
    }

    /// Program logical page `lpn` through the device layer, then record its
    /// physical page in the L2P table through `page_map`.
    fn program(f: &mut Fixture, lpn: usize) {
        unsafe {
            let ppn = 1024 + lpn as u32;
            let mut page: Vec<u8> = (0..DEVICE_PAGE_SIZE)
                .map(|offset| pattern_byte(lpn, offset))
                .collect();
            let mut request = Request::new().expect("allocate a request");
            request.paddr = device_address::from(ppn);
            request.data_len = DEVICE_PAGE_SIZE;
            request.flag = DeviceDirection::Write.into();
            request.data = page.as_mut_ptr() as *mut c_void;
            request.end_rq = None;
            let write = (*(*f.dev).d_op).write.expect("ramdisk vtable has write");
            assert_eq!(
                write(f.dev, request.as_ptr()),
                DEVICE_PAGE_SIZE as isize
            );
            update_map(&mut f.pgftl, lpn * DEVICE_PAGE_SIZE, ppn).unwrap();
        }
    }

    /// Read `buf.len()` bytes at absolute byte `sector` through [`read_page`].
    fn read_at(f: &mut Fixture, sector: usize, buf: &mut [u8]) -> io::Result<usize> {
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Read.into();
        request.sector = sector;
        request.data_len = buf.len();
        request.data = buf.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        read_page(&mut f.pgftl, &mut request)
    }

    /// Assert `buf` holds exactly `page[offset..offset + len]` for the page
    /// holding absolute byte `sector`.
    fn assert_pattern(buf: &[u8], sector: usize) {
        for (index, byte) in buf.iter().enumerate() {
            assert_eq!(
                *byte,
                expected_byte(sector + index),
                "byte {index} of a read at sector {sector}"
            );
        }
    }

    #[test]
    fn unmapped_read_returns_zeros() {
        let mut f = Fixture::new();
        let mut buf = vec![0xffu8; 32];
        let n = read_at(&mut f, 5, &mut buf).unwrap();
        assert_eq!(n, 32);
        assert!(buf.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn read_returns_programmed_data() {
        let mut f = Fixture::new();
        program(&mut f, 0);

        let mut whole = vec![0u8; 64];
        let n = read_at(&mut f, 0, &mut whole).unwrap();
        assert_eq!(n, 64);
        assert_pattern(&whole, 0);

        // A read that starts mid page copies the matching subrange.
        program(&mut f, 1);
        let start = DEVICE_PAGE_SIZE + 100;
        let mut mid = vec![0u8; 8];
        let n = read_at(&mut f, start, &mut mid).unwrap();
        assert_eq!(n, 8);
        assert_pattern(&mid, start);
    }

    #[test]
    fn read_starts_mid_page_and_ends_at_boundary() {
        let mut f = Fixture::new();
        program(&mut f, 0);

        // Starts four bytes before the end of page 0 and ends exactly on the
        // page boundary.
        let start = DEVICE_PAGE_SIZE - 4;
        let mut tail = vec![0u8; 4];
        let n = read_at(&mut f, start, &mut tail).unwrap();
        assert_eq!(n, 4);
        assert_pattern(&tail, start);

        // A whole page read from a page-aligned sector.
        let mut whole = vec![0u8; DEVICE_PAGE_SIZE];
        let n = read_at(&mut f, 0, &mut whole).unwrap();
        assert_eq!(n, DEVICE_PAGE_SIZE);
        assert_pattern(&whole, 0);
    }

    #[test]
    fn read_past_the_page_is_rejected() {
        let mut f = Fixture::new();
        program(&mut f, 0);

        let mut buf = vec![0u8; 8];
        let err = read_at(&mut f, DEVICE_PAGE_SIZE - 4, &mut buf).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn read_past_the_l2p_table_is_rejected() {
        let mut f = Fixture::new();
        let entries = f.pgftl.map_entries();
        assert!(entries > 1, "the fixture needs at least one table slot");

        // The last valid table slot is `entries - 1`. The first slot past the
        // table, `entries`, and anything beyond it must fail before `trans_map`
        // is read. C has no bound here at all.
        let mut buf = vec![0u8; 8];
        for lpn in [entries, entries + 1] {
            let err = read_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut buf).unwrap_err();
            assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
        }

        // The last in bounds slot is the unused terminator, so it reads zeros
        // like every unmapped page.
        let n = read_at(&mut f, (entries - 1) * DEVICE_PAGE_SIZE, &mut buf).unwrap();
        assert_eq!(n, 8);
        assert!(buf.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn ffi_read_matches_c_contract() {
        unsafe {
            let mut f = Fixture::new();

            assert_eq!(
                ffi_page_ftl_read(ptr::null_mut(), ptr::null_mut()),
                -(libc::EINVAL as isize)
            );
            assert_eq!(
                ffi_page_ftl_read(f.pgftl.as_mut() as *mut PageFtl, ptr::null_mut()),
                -(libc::EINVAL as isize)
            );

            // An unmapped LPN zero fills and frees the request, like C.
            let mut buf = vec![0xffu8; 16];
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            assert!(!request.is_null());
            (*request).flag = DeviceDirection::Read.into();
            (*request).sector = 0;
            (*request).data_len = buf.len();
            (*request).data = buf.as_mut_ptr() as *mut c_void;

            assert_eq!(ffi_page_ftl_read(f.pgftl.as_mut(), request), 16);
            assert!(buf.iter().all(|byte| *byte == 0));
            // `request` was freed by `ffi_page_ftl_read`, so it is not touched.
        }
    }

    #[test]
    fn ffi_mapped_read_returns_data_and_frees_the_request() {
        unsafe {
            let mut f = Fixture::new();
            program(&mut f, 0);

            let sector = 100;
            let mut buf = vec![0u8; 24];
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            assert!(!request.is_null());
            (*request).flag = DeviceDirection::Read.into();
            (*request).sector = sector;
            (*request).data_len = buf.len();
            (*request).data = buf.as_mut_ptr() as *mut c_void;

            // C frees the caller's request once on the success path
            // (`ftl/page/page-read.c:135`), so the wrapper owns it from here.
            assert_eq!(ffi_page_ftl_read(f.pgftl.as_mut(), request), 24);
            assert_pattern(&buf, sector);
            // `request` is not freed a second time, so the process stays
            // consistent.
            let probe = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            assert!(!probe.is_null());
            ffi_device_free_request(probe);
        }
    }

    #[test]
    fn device_read_failure_releases_the_scratch_and_internal_request() {
        unsafe {
            let mut dev = Box::new(MaybeUninit::<device>::zeroed().assume_init());
            dev.info.package.block.page.size = DEVICE_PAGE_SIZE;
            dev.install_backend(FailingRead);

            let mut pgftl = Box::new(PageFtl::empty());
            pgftl.dev = &mut *dev;
            pgftl.trans_map = vec![7u32];

            let mut buf = vec![0u8; 16];
            let mut request = Request::new().expect("allocate a request");
            request.flag = DeviceDirection::Read.into();
            request.sector = 0;
            request.data_len = buf.len();
            request.data = buf.as_mut_ptr() as *mut c_void;
            request.end_rq = None;

            let err = read_page(&mut pgftl, &mut request).unwrap_err();
            assert_eq!(err.raw_os_error(), Some(libc::EIO));

            // The scratch page buffer and the internal read request are
            // reclaimed at the single `reclaim_scratch` site before the error
            // returns, so the allocator and the request pool stay usable.
            let probe = Request::new().expect("allocator still healthy");
            drop(probe);

            (*pgftl.dev).uninstall_backend::<FailingRead>();
        }
    }
}
