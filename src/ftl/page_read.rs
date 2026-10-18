//! Page FTL read path, ported from `ftl/page/page-read.c`.
//!
//! [`read_page`] mirrors C's `page_ftl_read` and [`read_end_rq`] mirrors its
//! static `page_ftl_read_end_rq` completion callback. The C-ABI entry point
//! [`ffi_page_ftl_read`] carries the `ffi_` prefix so it can coexist with the
//! still linked C `page_ftl_read` until the Task 14 cutover.
//!
//! The flow is the C flow: resolve the logical page and the in-page offset,
//! look the physical address up in the L2P table, serve an unmapped page with
//! zeros, reject a read that runs past the page, read the whole page into a
//! scratch buffer through the device layer, wait for the request to finish,
//! then let the completion callback copy the requested subrange out.
//!
//! Deliberate deviations from the C original:
//!
//! - The native [`read_page`] borrows the caller's request and never frees it,
//!   because Rust owns the request through [`crate::device::Request`]. C's `page_ftl_read`
//!   frees it on the zero-fill path (`ftl/page/page-read.c:85`) and on the
//!   success path (`ftl/page/page-read.c:135`). [`ffi_page_ftl_read`]
//!   reproduces C's free.
//! - C locks `pgftl->mutex` around the `trans_map` read
//!   (`ftl/page/page-read.c:76-78`). The native API takes `&mut PageFtl`, so
//!   the exclusive borrow replaces the lock.
//! - C waits for completion with `pthread_cond_wait` on the request's
//!   condition variable and stores `is_finish` under its mutex
//!   (`ftl/page/page-read.c:36-41`, `ftl/page/page-read.c:129-133`). Rust
//!   stores the same atomic `is_finish` word but polls it instead of parking on
//!   the condition variable.
//! - C leaks the scratch page buffer and the internal read request when
//!   `d_op->read` returns negative by nulling both locals before the cleanup
//!   label (`ftl/page/page-read.c:121-127`). Rust reclaims both through their
//!   owning types.
//! - The scratch page buffer uses `vec!`, which aborts on out of memory like
//!   the rest of the port, so C's `-ENOMEM` from the page `malloc`
//!   (`ftl/page/page-read.c:98-102`) has no Rust equivalent. The internal
//!   request allocation keeps the `-ENOMEM` path.
//! - [`ffi_page_ftl_read`] returns `-EINVAL` for a `NULL` FTL or request where
//!   C dereferences them.

use core::ffi::c_void;
use std::io;

use crate::device::{
    device_address, device_request, ffi_device_alloc_request, ffi_device_free_request,
    DeviceDirection, DEVICE_DEFAULT_REQUEST, PADDR_EMPTY,
};
use crate::ftl::PageFtl;
use crate::{pr_err, pr_warn};

/// C's `page_ftl_read_end_rq`: copy the requested subrange out of the finished
/// page read and release the scratch buffer and the internal request.
///
/// `read_rq->rq_private` names the caller's request and that request's
/// `rq_private` names the page FTL, exactly as C threads them through.
///
/// # Safety
/// `read_rq` is a live request produced by [`read_page`] with a non-null
/// `data` buffer of `data_len` bytes and a caller request in `rq_private`.
unsafe extern "C" fn read_end_rq(read_rq: *mut device_request) {
    let request = (*read_rq).rq_private as *mut device_request;
    let pgftl = (*request).rq_private as *mut PageFtl;
    let offset = (*pgftl).page_offset((*request).sector);
    let len = (*request).data_len;

    let page = core::slice::from_raw_parts((*read_rq).data as *const u8, offset + len);
    if let Some(dst) = (*request).buffer_mut() {
        dst.copy_from_slice(&page[offset..offset + len]);
    }

    // C's `free(read_rq->data)` (`ftl/page/page-read.c:33`). The buffer came
    // from `vec![0u8; page_size]`, so length equals capacity.
    drop(Vec::from_raw_parts(
        (*read_rq).data as *mut u8,
        (*read_rq).data_len,
        (*read_rq).data_len,
    ));
    ffi_device_free_request(read_rq);

    (*request).mark_finished();
}

/// C's `page_ftl_read`: read `request.data_len` bytes at `request.sector`.
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

    let paddr = device_address::from(unsafe { *pgftl.trans_map.add(lpn) });

    if paddr.lpn == PADDR_EMPTY {
        pr_warn!("cannot find the mapping information (lpn: {})", lpn);
        let len = request.data_len;
        if let Some(buf) = unsafe { request.buffer_mut() } {
            buf.fill(0);
        }
        return Ok(len);
    }

    request.rq_private = pgftl as *mut PageFtl as *mut c_void;

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

    let read_rq = unsafe { ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST) };
    if read_rq.is_null() {
        pr_err!("request allocation failed");
        drop(unsafe { Vec::from_raw_parts(page_ptr, page_size, page_size) });
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
        // C nulls both locals and leaks them here (`page-read.c:121-127`).
        unsafe {
            drop(Vec::from_raw_parts(page_ptr, page_size, page_size));
            ffi_device_free_request(read_rq);
        }
        return Err(io::Error::from_raw_os_error((-ret) as i32));
    }

    while !request.is_finish() {
        std::thread::yield_now();
    }

    Ok(data_len)
}

/// C-ABI entry point for [`read_page`], mirroring C's `page_ftl_read`.
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

    match read_page(&mut *pgftl, &mut *request) {
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
    use crate::device::ramdisk::{ffi_ramdisk_close, ffi_ramdisk_open, ffi_ramdisk_write};
    use crate::device::{
        ffi_device_module_exit, ffi_device_module_init, DeviceModule, Request, DEVICE_PAGE_SIZE,
    };
    use crate::ftl::page_map::update_map;
    use core::mem::MaybeUninit;
    use core::ptr;

    /// A real ramdisk device plus a page FTL L2P table over it.
    struct Fixture {
        dev: *mut crate::device::device,
        _trans_map: Vec<u32>,
        pgftl: Box<PageFtl>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut dev: *mut crate::device::device = ptr::null_mut();
            unsafe {
                assert_eq!(
                    ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                    0
                );
                assert_eq!(
                    ffi_ramdisk_open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR),
                    0
                );
            }

            let entries = unsafe { (*dev).total_pages() + 1 };
            let mut trans_map = vec![PADDR_EMPTY; entries];
            let mut pgftl = Box::new(unsafe { MaybeUninit::<PageFtl>::zeroed().assume_init() });
            pgftl.dev = dev;
            pgftl.trans_map = trans_map.as_mut_ptr();

            Fixture {
                dev,
                _trans_map: trans_map,
                pgftl,
            }
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

    /// Program logical page `lpn` with `fill` through the device layer, then
    /// record its physical page in the L2P table through `page_map`.
    fn program(f: &mut Fixture, lpn: usize, fill: u8) {
        unsafe {
            let ppn = 4096 + lpn as u32;
            let mut page = vec![fill; DEVICE_PAGE_SIZE];
            let mut request = Request::new().expect("allocate a request");
            request.paddr = device_address::from(ppn);
            request.data_len = DEVICE_PAGE_SIZE;
            request.flag = DeviceDirection::Write.into();
            request.data = page.as_mut_ptr() as *mut c_void;
            request.end_rq = None;
            assert_eq!(
                ffi_ramdisk_write(f.dev, request.as_ptr()),
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
        program(&mut f, 0, 0xab);

        let mut whole = vec![0u8; 16];
        let n = read_at(&mut f, 0, &mut whole).unwrap();
        assert_eq!(n, 16);
        assert!(whole.iter().all(|byte| *byte == 0xab));

        // A read that starts mid page copies the matching subrange.
        program(&mut f, 1, 0xcd);
        let mut mid = vec![0u8; 8];
        let n = read_at(&mut f, DEVICE_PAGE_SIZE + 100, &mut mid).unwrap();
        assert_eq!(n, 8);
        assert!(mid.iter().all(|byte| *byte == 0xcd));
    }

    #[test]
    fn read_starts_mid_page_and_ends_at_boundary() {
        let mut f = Fixture::new();
        program(&mut f, 0, 0x11);

        // Starts four bytes before the end of page 0 and ends exactly on the
        // page boundary.
        let mut tail = vec![0u8; 4];
        let n = read_at(&mut f, DEVICE_PAGE_SIZE - 4, &mut tail).unwrap();
        assert_eq!(n, 4);
        assert!(tail.iter().all(|byte| *byte == 0x11));

        // A whole page read from a page-aligned sector.
        let mut whole = vec![0u8; DEVICE_PAGE_SIZE];
        let n = read_at(&mut f, 0, &mut whole).unwrap();
        assert_eq!(n, DEVICE_PAGE_SIZE);
        assert!(whole.iter().all(|byte| *byte == 0x11));
    }

    #[test]
    fn read_past_the_page_is_rejected() {
        let mut f = Fixture::new();
        program(&mut f, 0, 0x22);

        let mut buf = vec![0u8; 8];
        let err = read_at(&mut f, DEVICE_PAGE_SIZE - 4, &mut buf).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
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
}
