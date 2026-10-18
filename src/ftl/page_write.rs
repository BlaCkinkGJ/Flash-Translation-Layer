//! Page FTL write path, ported from `ftl/page/page-write.c`.
//!
//! [`write_page`] mirrors C's `page_ftl_write` and [`write_end_rq`] its static
//! `page_ftl_write_end_rq` completion callback. [`invalidate`],
//! [`read_for_overwrite`] and [`update_metadata`] mirror C's file-static
//! helpers of the same name. The C-ABI entry point [`ffi_page_ftl_write`]
//! carries the `ffi_` prefix so it can coexist with the still linked C
//! `page_ftl_write` until the Task 14 cutover.
//!
//! The flow is the C flow: resolve the logical page and the in-page offset,
//! reject a logical page past the L2P table and a write that runs past its
//! page, take a free physical page, build the whole page buffer (zeroed, or the
//! previous contents when the LPN is already mapped), splice the caller's bytes
//! into it, hand the page to the device, then update the L2P table, dropping
//! the previous mapping and queueing its segment for garbage collection once
//! that segment has no free page left. The return value is the caller's byte
//! count, not the page the device wrote.
//!
//! Deliberate deviations from the C original:
//!
//! - The device write goes through an internal request instead of the caller's.
//!   C hands the caller's request to `d_op->write`, and its
//!   `page_ftl_write_end_rq` frees that request together with the scratch page
//!   (`ftl/page/page-write.c:59-63`, `:213-218`, `:220`). [`write_page`]
//!   borrows the caller's request and allocates its own request for the device
//!   write, so `write_end_rq` frees the scratch page and that internal request,
//!   and [`ffi_page_ftl_write`] frees the caller's request on the success path.
//!   A native caller keeps its request.
//! - C holds `pgftl->mutex` around the free page allocation, the `is_exist`
//!   probe and the metadata update (`ftl/page/page-write.c:187-189`,
//!   `:201-203`, `:226-228`). [`write_page`] takes `&mut PageFtl`, and the
//!   exclusive borrow replaces the lock.
//! - The scratch page buffer is a `vec!`, which aborts on out of memory like
//!   the rest of the port, so C's `-ENOMEM` from the page `malloc`
//!   (`ftl/page/page-write.c:195-199`) has no Rust equivalent. The internal
//!   request allocation keeps the `-ENOMEM` path.
//! - A failed device write leaves the scratch page allocated in C, and a failed
//!   read for overwrite leaves its read request allocated
//!   (`ftl/page/page-write.c:100-103`, `:220-225`), the caller freeing only its
//!   own request (`ftl/page/page-interface.c:140-144`). The port reclaims both
//!   through their owning types.
//! - C's read for overwrite collapses every `page_ftl_read` failure to
//!   `-EFAULT` (`ftl/page/page-write.c:99-103`). The port keeps that collapse,
//!   and keeps C's `-ENOMEM` for the request allocation that fails before the
//!   read (`ftl/page/page-write.c:91-94`).
//! - A `NULL` `request->data` is refused with `-EINVAL` before the splice,
//!   where C's `memcpy` faults for a non-zero `write_size`
//!   (`ftl/page/page-write.c:211`). The same guard makes a zero `write_size`
//!   with a `NULL` data pointer fail, where C copies nothing and goes on to
//!   write the page.
//! - C returns the device write's value verbatim
//!   (`ftl/page/page-write.c:221-225`), so a short positive write reaches the
//!   caller as a short count. [`write_page`] cannot carry a positive failure in
//!   `io::Result`, so it reports `EIO` for a non-negative mismatch and keeps
//!   the negated errno for a negative one.
//! - [`ffi_page_ftl_write`] returns `-EINVAL` for a `NULL` FTL or request,
//!   where C dereferences them.

use core::ffi::c_void;
use core::mem;
use core::slice;
use core::sync::atomic::Ordering;
use std::io;

use crate::bits::{get_bit, set_bit};
use crate::device::{
    device_address, device_request, ffi_device_alloc_request, ffi_device_free_request,
    DeviceDirection, Request, DEVICE_DEFAULT_REQUEST, PADDR_EMPTY,
};
use crate::ftl::page_map::{get_free_page, update_map};
use crate::ftl::page_read::read_page;
use crate::ftl::{PageFtl, PageFtlSegment};
use crate::list::{ffi_list_prepend, ffi_list_remove};
use crate::{pr_debug, pr_err};

/// C's `page_ftl_write_end_rq`: release the scratch page and the request that
/// carried it.
///
/// # Safety
/// `request` is a live request whose `data` points at a `data_len` byte page
/// buffer allocated by [`write_page`], and it is not used afterwards.
unsafe extern "C" fn write_end_rq(request: *mut device_request) {
    // C's `free(request->data)` (`ftl/page/page-write.c:61`). The buffer came
    // from `vec![0u8; page_size]`, so its length equals its capacity.
    drop(Vec::from_raw_parts(
        (*request).data as *mut u8,
        (*request).data_len,
        (*request).data_len,
    ));
    ffi_device_free_request(request);
}

/// C's `page_ftl_invalidate`: drop the mapping of `lpn`, unlink it from its
/// segment and queue that segment for garbage collection once it has no free
/// page left.
///
/// The free page count of the segment deliberately does not move here: C only
/// reloads `nr_free_pages` (`ftl/page/page-write.c:44`), so the counter
/// keeps its meaning (a page that was written once is still "used" and is
/// reclaimed by an erase, not by an invalidation).
fn invalidate(pgftl: &mut PageFtl, lpn: usize) {
    let paddr = device_address::from(unsafe { *pgftl.trans_map.add(lpn) });
    let segnum = paddr.block() as usize;
    let segment = unsafe { &mut *pgftl.segments.add(segnum) };

    segment.lpn_list = unsafe { ffi_list_remove(segment.lpn_list, lpn as *mut c_void) };
    segment.nr_valid_pages.fetch_sub(1, Ordering::SeqCst);
    let nr_free_pages = segment.free_pages();

    unsafe { *pgftl.trans_map.add(lpn) = PADDR_EMPTY };

    let gc_bits = unsafe { slice::from_raw_parts_mut(pgftl.gc_seg_bits, segnum / 64 + 1) };
    if nr_free_pages == 0 && !get_bit(gc_bits, segnum as u64) {
        pgftl.gc_list = unsafe {
            ffi_list_prepend(pgftl.gc_list, segment as *mut PageFtlSegment as *mut c_void)
        };
        set_bit(gc_bits, segnum as u64);
    }
}

/// C's `page_ftl_read_for_overwrite`: fill `buffer` with the page `lpn` is
/// currently mapped to.
///
/// NAND cannot be overwritten in place, so the write path reads the previous
/// page, splices the new bytes into it and writes the result to a fresh page.
///
/// # Errors
/// `ENOMEM` when the read request cannot be allocated, and `EFAULT` for every
/// failure of the read itself, which is the error code C reports.
fn read_for_overwrite(pgftl: &mut PageFtl, lpn: usize, buffer: &mut [u8]) -> io::Result<()> {
    let page_size = pgftl.dev().page_size();

    let mut read_rq = match Request::new() {
        Ok(request) => request,
        Err(_) => {
            pr_err!("crete read request failed");
            return Err(io::Error::from_raw_os_error(libc::ENOMEM));
        }
    };

    // C reads from the start of the page holding `lpn`, not from the sector the
    // caller wrote at (`ftl/page/page-write.c:95-98`).
    read_rq.flag = DeviceDirection::Read.into();
    read_rq.sector = lpn * page_size;
    read_rq.data_len = page_size;
    read_rq.data = buffer.as_mut_ptr() as *mut c_void;
    read_rq.end_rq = None;

    if read_page(pgftl, &mut read_rq).is_err() {
        pr_err!("previous buffer read failed");
        return Err(io::Error::from_raw_os_error(libc::EFAULT));
    }

    Ok(())
}

/// C's `page_ftl_write_update_metadata`: point `sector`'s logical page at
/// `paddr`, dropping and queueing the mapping it replaces.
fn update_metadata(pgftl: &mut PageFtl, paddr: device_address, sector: usize) {
    let lpn = pgftl.lpn(sector);
    if unsafe { *pgftl.trans_map.add(lpn) } != PADDR_EMPTY {
        invalidate(pgftl, lpn);
        pr_debug!(
            "invalidate address: {} => {}",
            lpn,
            unsafe { *pgftl.trans_map.add(lpn) }
        );
    }

    let segment = unsafe { &mut *pgftl.segments.add(paddr.block() as usize) };
    segment.lpn_list = unsafe { ffi_list_prepend(segment.lpn_list, lpn as *mut c_void) };

    // C discards `page_ftl_update_map`'s result here
    // (`ftl/page/page-write.c:133`), so an out of range mapping is not
    // reported and the write still counts as done.
    let _ = update_map(pgftl, sector, paddr.lpn);

    pr_debug!(
        "new address: {} => {} (seg: {})",
        lpn,
        unsafe { *pgftl.trans_map.add(lpn) },
        paddr.block()
    );
}

/// C's `page_ftl_write`: write `request.data_len` bytes at `request.sector`.
///
/// The bytes go into a freshly allocated physical page: the page is zeroed for
/// an unmapped logical page, and otherwise filled with the page that LPN is
/// currently mapped to before the new bytes are spliced in. The previous
/// mapping is invalidated afterwards, so an overwrite leaves the old page
/// untouched on the device.
///
/// Returns the number of bytes written, which is `request.data_len`, not the
/// page size.
///
/// # Errors
/// `EINVAL` for a logical page past the L2P table or a write that runs past the
/// end of its page, `EFAULT` when no free page is left on the device or the
/// read for overwrite fails, `ENOMEM` when the page request cannot be
/// allocated, and the device's negated error code for a failed device write.
/// The request is borrowed, not consumed.
pub fn write_page(pgftl: &mut PageFtl, request: &mut device_request) -> io::Result<usize> {
    let page_size = pgftl.dev().page_size();
    let write_size = request.data_len;
    let sector = request.sector;
    let lpn = pgftl.lpn(sector);
    let offset = pgftl.page_offset(sector);

    let nr_entries = pgftl.map_entries();
    if lpn > nr_entries {
        pr_err!("invalid lpn detected (lpn: {}, max: {})", lpn, nr_entries);
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }

    if offset + write_size > page_size {
        pr_err!(
            "overflow the write data (offset: {}, length: {})",
            offset,
            write_size
        );
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }

    let Some(paddr) = get_free_page(pgftl) else {
        pr_err!("cannot allocate the valid page from device");
        return Err(io::Error::from_raw_os_error(libc::EFAULT));
    };

    let mut buffer = vec![0u8; page_size];

    let is_exist = unsafe { *pgftl.trans_map.add(lpn) } != PADDR_EMPTY;
    if is_exist {
        read_for_overwrite(pgftl, lpn, &mut buffer)?;
    }

    // C's `memcpy(&buffer[offset], request->data, write_size)`
    // (`ftl/page/page-write.c:211`).
    let src = match unsafe { request.buffer_mut() } {
        Some(src) => src,
        None => {
            pr_err!("write data pointer is NULL");
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
    };
    buffer[offset..offset + write_size].copy_from_slice(src);

    let page_rq = unsafe { ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST) };
    if page_rq.is_null() {
        pr_err!("memory allocation failed");
        return Err(io::Error::from_raw_os_error(libc::ENOMEM));
    }

    let buffer_ptr = buffer.as_mut_ptr();
    mem::forget(buffer);
    unsafe {
        (*page_rq).flag = DeviceDirection::Write.into();
        (*page_rq).data = buffer_ptr as *mut c_void;
        (*page_rq).paddr = paddr;
        (*page_rq).rq_private = pgftl as *mut PageFtl as *mut c_void;
        (*page_rq).data_len = page_size;
        (*page_rq).end_rq = Some(write_end_rq);
    }

    let dev_ptr = pgftl.dev;
    let ret = unsafe {
        let ops = (*dev_ptr).d_op;
        let write = (*ops).write.expect("device backend has no write operation");
        write(dev_ptr, page_rq)
    };
    if ret != page_size as isize {
        pr_err!("device write failed (ppn: {})", paddr);
        // A backend that fails does not run the completion callback, so the
        // scratch page and the request are still ours.
        unsafe {
            drop(Vec::from_raw_parts(buffer_ptr, page_size, page_size));
            ffi_device_free_request(page_rq);
        }
        let errno = if ret < 0 { -ret as i32 } else { libc::EIO };
        return Err(io::Error::from_raw_os_error(errno));
    }

    update_metadata(pgftl, paddr, sector);

    Ok(write_size)
}

/// C-ABI entry point for [`write_page`], mirroring C's `page_ftl_write`.
///
/// Frees the caller's request on the success path, where C's
/// `page_ftl_write_end_rq` does, and leaves it to the caller on every error
/// path, exactly like C.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with its `trans_map`, `segments` and
/// `dev` initialized, and `request` is `NULL` or a live request whose `data`
/// points at `data_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_write(
    pgftl: *mut PageFtl,
    request: *mut device_request,
) -> isize {
    if pgftl.is_null() || request.is_null() {
        pr_err!("page FTL or request is NULL");
        return -(libc::EINVAL as isize);
    }

    match write_page(&mut *pgftl, &mut *request) {
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
    use crate::device::ramdisk::{
        ffi_ramdisk_close, ffi_ramdisk_open, ffi_ramdisk_read, ffi_ramdisk_write,
    };
    use crate::device::{
        device, device_operations, ffi_device_module_exit, ffi_device_module_init, DeviceModule,
        DEVICE_PAGE_SIZE,
    };
    use crate::list::{ffi_list_free, ffi_list_length};
    use core::ptr;

    /// Two segments of four pages each, over a real ramdisk.
    const NR_SEGMENTS: usize = 2;
    const PAGES_PER_SEGMENT: usize = 4;
    const TOTAL_PAGES: usize = NR_SEGMENTS * PAGES_PER_SEGMENT;
    /// `u64` words reserved for one segment's `use_bits` bitmap, so each
    /// segment gets its own region of the shared allocation.
    const WORDS_PER_SEGMENT: usize = 64;

    /// A real ramdisk device plus the page FTL state the write path needs.
    ///
    /// The ramdisk sizes its store from the geometry `open` installs (64
    /// segments of 2048 pages, a 1GiB board). The fixture then narrows
    /// `dev.info` to two segments of four pages so a test can fill a whole
    /// segment and exhaust the free pages in a handful of writes. Every address
    /// the narrowed geometry produces still lands inside the ramdisk's store,
    /// because the packed address keeps the segment in the high bits.
    struct Fixture {
        dev: *mut device,
        trans_map: Vec<u32>,
        segments: Vec<PageFtlSegment>,
        use_bits: Vec<u64>,
        gc_seg_bits: Vec<u64>,
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
                assert_eq!(
                    ffi_ramdisk_open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR),
                    0
                );
                (*dev).info.nr_bus = 1;
                (*dev).info.nr_chips = 1;
                (*dev).info.package.nr_blocks = NR_SEGMENTS;
                (*dev).info.package.block.nr_pages = PAGES_PER_SEGMENT;
            }

            let mut trans_map = vec![PADDR_EMPTY; TOTAL_PAGES + 1];
            let mut use_bits = vec![0u64; NR_SEGMENTS * WORDS_PER_SEGMENT];
            let mut gc_seg_bits = vec![0u64; 1];
            let segments: Vec<PageFtlSegment> = (0..NR_SEGMENTS)
                .map(|segnum| PageFtlSegment {
                    nr_free_pages: core::sync::atomic::AtomicI32::new(PAGES_PER_SEGMENT as i32),
                    nr_valid_pages: core::sync::atomic::AtomicI32::new(0),
                    is_gc: 0,
                    use_bits: unsafe { use_bits.as_mut_ptr().add(segnum * WORDS_PER_SEGMENT) },
                    lpn_list: ptr::null_mut(),
                })
                .collect();

            let mut pgftl =
                Box::new(unsafe { core::mem::MaybeUninit::<PageFtl>::zeroed().assume_init() });
            pgftl.dev = dev;
            pgftl.trans_map = trans_map.as_mut_ptr();
            pgftl.segments = segments.as_ptr() as *mut PageFtlSegment;
            pgftl.gc_seg_bits = gc_seg_bits.as_mut_ptr();

            Fixture {
                dev,
                trans_map,
                segments,
                use_bits,
                gc_seg_bits,
                pgftl,
            }
        }

        fn free(&self, segnum: usize) -> i32 {
            self.segments[segnum].free_pages()
        }

        fn valid(&self, segnum: usize) -> i32 {
            self.segments[segnum]
                .nr_valid_pages
                .load(Ordering::SeqCst)
        }

        fn lpn_list_len(&self, segnum: usize) -> usize {
            unsafe { ffi_list_length(self.segments[segnum].lpn_list) }
        }

        fn gc_list_len(&self) -> usize {
            unsafe { ffi_list_length(self.pgftl.gc_list) }
        }

        fn gc_bit(&self, segnum: usize) -> bool {
            get_bit(&self.gc_seg_bits, segnum as u64)
        }

        /// The first `u64` of a segment's `use_bits` bitmap.
        fn use_bits_word(&self, segnum: usize) -> u64 {
            self.use_bits[segnum * WORDS_PER_SEGMENT]
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                ffi_list_free(self.pgftl.gc_list);
                for segment in &mut self.segments {
                    ffi_list_free(segment.lpn_list);
                }
                assert_eq!(ffi_ramdisk_close(self.dev), 0);
                assert_eq!(ffi_device_module_exit(self.dev), 0);
            }
        }
    }

    /// Write `data.len()` bytes at absolute byte `sector` through
    /// [`write_page`].
    fn write_at(f: &mut Fixture, sector: usize, data: &mut [u8]) -> io::Result<usize> {
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Write.into();
        request.sector = sector;
        request.data_len = data.len();
        request.data = data.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        write_page(&mut f.pgftl, &mut request)
    }

    /// Read `buf.len()` bytes at absolute byte `sector` through the read path.
    fn read_at(f: &mut Fixture, sector: usize, buf: &mut [u8]) -> io::Result<usize> {
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Read.into();
        request.sector = sector;
        request.data_len = buf.len();
        request.data = buf.as_mut_ptr() as *mut c_void;
        read_page(&mut f.pgftl, &mut request)
    }

    /// Read physical page `ppn` straight from the device.
    fn read_physical_page(f: &mut Fixture, ppn: u32, buf: &mut [u8]) {
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Read.into();
        request.paddr = device_address::from(ppn);
        request.data_len = DEVICE_PAGE_SIZE;
        request.data = buf.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        unsafe {
            assert_eq!(
                ffi_ramdisk_read(f.dev, request.as_ptr()),
                DEVICE_PAGE_SIZE as isize
            );
        }
    }

    #[test]
    fn single_page_write_then_read_back() {
        let mut f = Fixture::new();
        let data: Vec<u8> = (0..16u8).collect();

        let mut payload = data.clone();
        assert_eq!(write_at(&mut f, 0, &mut payload).unwrap(), 16);

        // The LPN maps to the first page of segment 0, which counted the page
        // valid and put the LPN on its list.
        assert_eq!(f.trans_map[0], 0);
        assert_eq!(f.pgftl.alloc_segnum, 0);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.valid(0), 1);
        assert_eq!(f.lpn_list_len(0), 1);
        assert_eq!(f.use_bits_word(0), 1);
        assert_eq!(f.gc_list_len(), 0);

        // The whole page reached the device: the payload at offset 0 and zeros
        // after it, which is the zeroed scratch buffer of a fresh LPN.
        let mut page = vec![0xffu8; DEVICE_PAGE_SIZE];
        assert_eq!(read_at(&mut f, 0, &mut page).unwrap(), DEVICE_PAGE_SIZE);
        assert_eq!(&page[..16], &data[..]);
        assert!(page[16..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn overwrite_invalidates_the_previous_mapping() {
        let mut f = Fixture::new();

        // A position dependent pattern: a mis-computed read offset or a short
        // read would show up as a mismatched byte.
        let first: Vec<u8> = (0..16u8).map(|i| 0x10 + i).collect();
        let mut payload = first.clone();
        assert_eq!(write_at(&mut f, 100, &mut payload).unwrap(), 16);
        assert_eq!(f.trans_map[0], 0);

        // Overwriting four bytes in the middle of the page must keep the rest
        // of the previous page, so the write path reads it first.
        let second = [0x11u8, 0x22, 0x33, 0x44];
        let mut payload = second;
        assert_eq!(write_at(&mut f, 102, &mut payload).unwrap(), 4);

        // The LPN moved to a new page, the old one was invalidated, and its
        // segment still has free pages, so nothing is queued for GC.
        let new_ppn = device_address::from_segment_page(0, 1).lpn;
        assert_eq!(f.trans_map[0], new_ppn);
        assert_eq!(f.valid(0), 1);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32 - 2);
        assert_eq!(f.lpn_list_len(0), 1);
        assert_eq!(f.gc_list_len(), 0);
        assert!(!f.gc_bit(0));

        // Out of place update: the bytes around the splice survived at their
        // own offsets, and the page the LPN used to point at is unchanged.
        let mut page = vec![0u8; DEVICE_PAGE_SIZE];
        assert_eq!(read_at(&mut f, 0, &mut page).unwrap(), DEVICE_PAGE_SIZE);
        assert_eq!(&page[100..102], &first[..2]);
        assert_eq!(&page[102..106], &second[..]);
        assert_eq!(&page[106..116], &first[6..]);
        assert!(page[..100].iter().all(|byte| *byte == 0));
        assert!(page[116..].iter().all(|byte| *byte == 0));

        let mut old = vec![0u8; DEVICE_PAGE_SIZE];
        read_physical_page(&mut f, 0, &mut old);
        assert_eq!(&old[100..116], &first[..]);
        assert!(old[..100].iter().all(|byte| *byte == 0));
        assert!(old[116..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn filling_a_segment_queues_it_for_gc_once() {
        let mut f = Fixture::new();

        for lpn in 0..PAGES_PER_SEGMENT {
            let mut payload = vec![lpn as u8; 8];
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                8
            );
            assert_eq!(f.trans_map[lpn], lpn as u32);
        }

        // Segment 0 has no free page left, and a write never queues a segment:
        // only an invalidation does that.
        assert_eq!(f.free(0), 0);
        assert_eq!(f.valid(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.use_bits_word(0), 0b1111);
        assert_eq!(f.gc_list_len(), 0);

        // Overwriting a page of the full segment invalidates it, and the
        // invalidation queues the segment once (`page-write.c:48-50`).
        let mut payload = vec![0xeeu8; 8];
        assert_eq!(write_at(&mut f, 0, &mut payload).unwrap(), 8);

        let seg0 = &f.segments[0] as *const PageFtlSegment;
        assert!(core::ptr::eq(
            unsafe { (*f.pgftl.gc_list).data } as *const PageFtlSegment,
            seg0
        ));
        assert_eq!(f.gc_list_len(), 1);
        assert!(f.gc_bit(0));
        assert!(!f.gc_bit(1));
        assert_eq!(f.trans_map[0], device_address::from_segment_page(1, 0).lpn);
        assert_eq!(f.valid(0), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.lpn_list_len(0), PAGES_PER_SEGMENT - 1);
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.lpn_list_len(1), 1);

        // A second invalidation in the same segment does not queue it twice.
        let mut payload = vec![0xddu8; 8];
        assert_eq!(write_at(&mut f, DEVICE_PAGE_SIZE, &mut payload).unwrap(), 8);
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.valid(0), PAGES_PER_SEGMENT as i32 - 2);
        assert_eq!(f.free(0), 0);
    }

    #[test]
    fn free_page_exhaustion_returns_efault() {
        let mut f = Fixture::new();

        for lpn in 0..TOTAL_PAGES {
            let mut payload = vec![0x5au8; 8];
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                8
            );
        }
        assert_eq!(f.free(0), 0);
        assert_eq!(f.free(1), 0);

        // No free page is left, so the write fails before it touches the map.
        let mut payload = vec![0x99u8; 8];
        let err = write_at(&mut f, 0, &mut payload).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EFAULT));
        assert_eq!(f.trans_map[0], 0);
        assert_eq!(f.valid(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.gc_list_len(), 0);
    }

    #[test]
    fn out_of_range_and_overflowing_writes_are_rejected() {
        let mut f = Fixture::new();
        let entries = f.pgftl.map_entries();
        assert_eq!(entries, TOTAL_PAGES + 1);

        // A write that runs past the end of its page.
        let mut tail = [0u8; 8];
        let err = write_at(&mut f, DEVICE_PAGE_SIZE - 4, &mut tail).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));

        // A logical page past the L2P table, C's `lpn > nr_entries` bound
        // (`page-write.c:175-179`).
        let mut data = [0u8; 8];
        let err = write_at(&mut f, (entries + 1) * DEVICE_PAGE_SIZE, &mut data).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));

        // Neither rejection allocated a page or touched the table, so the guard
        // runs before the free page lookup.
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(0), 0);
        assert!(f.trans_map.iter().all(|entry| *entry == PADDR_EMPTY));
        assert_eq!(f.gc_list_len(), 0);
    }

    #[test]
    fn zero_length_write_maps_the_page() {
        let mut f = Fixture::new();

        let mut nothing = [0u8; 0];
        assert_eq!(write_at(&mut f, 0, &mut nothing).unwrap(), 0);
        assert_eq!(f.trans_map[0], 0);
        assert_eq!(f.valid(0), 1);

        let mut page = vec![0xffu8; 16];
        assert_eq!(read_at(&mut f, 0, &mut page).unwrap(), 16);
        assert!(page.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn null_write_buffer_is_rejected() {
        let mut f = Fixture::new();

        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Write.into();
        request.sector = 0;
        request.data_len = 8;
        request.data = ptr::null_mut();
        request.end_rq = None;

        let err = write_page(&mut f.pgftl, &mut request).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));

        // The guard sits where C splices, so the free page was already taken
        // and stays taken. The map and the segment's LPN list are untouched.
        assert_eq!(f.trans_map[0], PADDR_EMPTY);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.valid(0), 1);
        assert_eq!(f.lpn_list_len(0), 0);
        assert_eq!(f.gc_list_len(), 0);
    }

    #[test]
    fn failed_device_write_keeps_the_page_unmapped() {
        let mut f = Fixture::new();

        // Program device page 0 behind the FTL's back. The ramdisk refuses to
        // write a page twice, so the FTL's first allocation fails on the
        // device.
        let mut page = vec![0x5au8; DEVICE_PAGE_SIZE];
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Write.into();
        request.paddr = device_address::from(0);
        request.data_len = DEVICE_PAGE_SIZE;
        request.data = page.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        unsafe {
            assert_eq!(
                ffi_ramdisk_write(f.dev, request.as_ptr()),
                DEVICE_PAGE_SIZE as isize
            );
        }

        let mut data = [0u8; 8];
        let err = write_at(&mut f, 0, &mut data).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));

        // The mapping is untouched. The page the allocator handed out stays
        // taken, which is what C does when the device write fails
        // (`page-write.c:220-225`).
        assert_eq!(f.trans_map[0], PADDR_EMPTY);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.valid(0), 1);
        assert_eq!(f.lpn_list_len(0), 0);
    }

    /// Reports one byte less than the page the FTL asked for and never runs the
    /// completion callback.
    unsafe extern "C" fn short_write(_dev: *mut device, request: *mut device_request) -> isize {
        (*request).data_len as isize - 1
    }

    static SHORT_WRITE_OPS: device_operations = device_operations {
        open: None,
        write: Some(short_write),
        read: None,
        erase: None,
        close: None,
    };

    /// Fails every device read with `EIO`.
    unsafe extern "C" fn failing_read(_dev: *mut device, _request: *mut device_request) -> isize {
        -(libc::EIO as isize)
    }

    static FAILING_READ_OPS: device_operations = device_operations {
        open: None,
        write: None,
        read: Some(failing_read),
        erase: None,
        close: None,
    };

    #[test]
    fn failed_read_for_overwrite_returns_efault() {
        let mut f = Fixture::new();

        let mut first = vec![0xaau8; 8];
        assert_eq!(write_at(&mut f, 0, &mut first).unwrap(), 8);

        // An overwrite reads the previous page first. Make that read fail: C
        // collapses it to -EFAULT (`page-write.c:99-103`).
        let saved = unsafe { (*f.dev).d_op };
        unsafe { (*f.dev).d_op = &FAILING_READ_OPS };

        let mut second = vec![0x11u8, 0x22, 0x33, 0x44];
        let err = write_at(&mut f, 0, &mut second).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EFAULT));

        unsafe { (*f.dev).d_op = saved };

        // The LPN still points at the first page. The page the allocator took
        // is gone, which is what C does when the read for overwrite fails.
        assert_eq!(f.trans_map[0], 0);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32 - 2);
        assert_eq!(f.valid(0), 2);
        assert_eq!(f.lpn_list_len(0), 1);
        assert_eq!(f.gc_list_len(), 0);
    }

    #[test]
    fn short_device_write_is_reported_as_io_error() {
        let mut f = Fixture::new();
        let saved = unsafe { (*f.dev).d_op };
        unsafe { (*f.dev).d_op = &SHORT_WRITE_OPS };

        let mut data = [0u8; 8];
        let err = write_at(&mut f, 0, &mut data).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EIO));

        unsafe { (*f.dev).d_op = saved };
        assert_eq!(f.trans_map[0], PADDR_EMPTY);
    }

    #[test]
    fn ffi_write_matches_c_contract() {
        unsafe {
            let mut f = Fixture::new();

            assert_eq!(
                ffi_page_ftl_write(ptr::null_mut(), ptr::null_mut()),
                -(libc::EINVAL as isize)
            );
            assert_eq!(
                ffi_page_ftl_write(f.pgftl.as_mut() as *mut PageFtl, ptr::null_mut()),
                -(libc::EINVAL as isize)
            );

            // A successful write frees the caller's request, like C's
            // `page_ftl_write_end_rq` (`page-write.c:59-63`).
            let mut payload: Vec<u8> = (0..8u8).map(|i| 0x30 + i).collect();
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            assert!(!request.is_null());
            (*request).flag = DeviceDirection::Write.into();
            (*request).sector = 0;
            (*request).data_len = payload.len();
            (*request).data = payload.as_mut_ptr() as *mut c_void;
            assert_eq!(ffi_page_ftl_write(f.pgftl.as_mut(), request), 8);
            assert_eq!(f.trans_map[0], 0);
            // `request` was freed by `ffi_page_ftl_write`, so it is not touched.

            // The bytes the splice copied are where the caller put them.
            let mut read_back = vec![0u8; payload.len()];
            assert_eq!(read_at(&mut f, 0, &mut read_back).unwrap(), payload.len());
            assert_eq!(read_back, payload);

            // An error path leaves the request to the caller, also like C.
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            (*request).flag = DeviceDirection::Write.into();
            (*request).sector = DEVICE_PAGE_SIZE - 4;
            (*request).data_len = 8;
            (*request).data = payload.as_mut_ptr() as *mut c_void;
            assert_eq!(
                ffi_page_ftl_write(f.pgftl.as_mut(), request),
                -(libc::EINVAL as isize)
            );
            ffi_device_free_request(request);

            // Free page exhaustion crosses the ABI as `-EFAULT`.
            for lpn in 1..TOTAL_PAGES {
                let mut chunk = [0u8; 8];
                let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
                (*request).flag = DeviceDirection::Write.into();
                (*request).sector = lpn * DEVICE_PAGE_SIZE;
                (*request).data_len = chunk.len();
                (*request).data = chunk.as_mut_ptr() as *mut c_void;
                assert_eq!(ffi_page_ftl_write(f.pgftl.as_mut(), request), 8);
            }
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            (*request).flag = DeviceDirection::Write.into();
            (*request).sector = 0;
            (*request).data_len = 8;
            (*request).data = payload.as_mut_ptr() as *mut c_void;
            assert_eq!(
                ffi_page_ftl_write(f.pgftl.as_mut(), request),
                -(libc::EFAULT as isize)
            );
            ffi_device_free_request(request);
        }
    }
}
