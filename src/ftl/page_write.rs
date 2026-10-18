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
//! Deliberate deviations from C are listed in the [`crate::ftl`] module docs.

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

    // C checks `lpn > nr_entries` (`ftl/page/page-write.c:175-179`), which lets
    // the slot one past the table through and then reads `trans_map` out of
    // bounds. `update_map` already refuses that slot with `lpn >= map_size`
    // (`src/ftl/page_map.rs`), and this guard matches it. See the deviations
    // block in `crate::ftl`.
    let nr_entries = pgftl.map_entries();
    if lpn >= nr_entries {
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
    use crate::device::{device, device_operations, DEVICE_PAGE_SIZE};
    use crate::ftl::test_support::{
        read_at, read_physical_page, write_at, write_physical_page, Fixture,
    };
    use core::ptr;

    /// Two segments of four pages each, over a real ramdisk.
    const NR_SEGMENTS: usize = 2;
    const PAGES_PER_SEGMENT: usize = 4;
    const TOTAL_PAGES: usize = NR_SEGMENTS * PAGES_PER_SEGMENT;

    /// The geometry this module's tests run on. The fixture itself lives in
    /// [`crate::ftl::test_support`], shared with the garbage collection tests.
    fn new_fixture() -> Fixture {
        Fixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT)
    }

    #[test]
    fn single_page_write_then_read_back() {
        let mut f = new_fixture();
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
        let mut f = new_fixture();

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
        let mut f = new_fixture();

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
        let mut f = new_fixture();

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
        let mut f = new_fixture();
        let entries = f.pgftl.map_entries();
        assert_eq!(entries, TOTAL_PAGES + 1);

        // A write that runs past the end of its page.
        let mut tail = [0u8; 8];
        let err = write_at(&mut f, DEVICE_PAGE_SIZE - 4, &mut tail).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));

        // The last valid slot is `entries - 1`, so `entries` and anything past
        // it are rejected. C's `lpn > nr_entries` check
        // (`page-write.c:175-179`) lets the first of the two through, and the
        // out of range `trans_map` read that follows is what this guard
        // prevents.
        let mut data = [0u8; 8];
        for lpn in [entries, entries + 1] {
            let err = write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut data).unwrap_err();
            assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
        }

        // Neither rejection allocated a page or touched the table, so the guard
        // runs before the free page lookup.
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(0), 0);
        assert!(f.trans_map.iter().all(|entry| *entry == PADDR_EMPTY));
        assert_eq!(f.gc_list_len(), 0);
    }

    #[test]
    fn zero_length_write_maps_the_page() {
        let mut f = new_fixture();

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
        let mut f = new_fixture();

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
        let mut f = new_fixture();

        // Program device page 0 behind the FTL's back. The ramdisk refuses to
        // write a page twice, so the FTL's first allocation fails on the
        // device.
        let mut page = vec![0x5au8; DEVICE_PAGE_SIZE];
        write_physical_page(&mut f, 0, &mut page);

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
        let mut f = new_fixture();

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
        let mut f = new_fixture();
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
            let mut f = new_fixture();

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
