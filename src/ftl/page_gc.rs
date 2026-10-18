//! Page FTL garbage collection, ported from `ftl/page/page-gc.c`.
//!
//! [`do_gc`] mirrors C's `page_ftl_do_gc`: it picks the queued segment with the
//! fewest valid pages, copies those pages into free pages elsewhere, erases the
//! segment and reinitializes its metadata. [`gc_from_list`] mirrors
//! `page_ftl_gc_from_list`, the loop that runs `do_gc` up to
//! `gc_ratio * nr_segments` times. [`gc_list_cmp`], [`erase_end_rq`],
//! [`pick_gc_target`], [`segment_erase`], [`read_valid_page`],
//! [`write_valid_page`] and [`valid_page_copy`] mirror the file's static
//! helpers. The C-ABI entry points [`ffi_page_ftl_do_gc`] and
//! [`ffi_page_ftl_gc_from_list`] carry the `ffi_` prefix so they can coexist
//! with the still linked C `page_ftl_do_gc` and `page_ftl_gc_from_list` until
//! the Task 14 cutover. C does not declare `page_ftl_gc_list_cmp` in a header,
//! so its port stays private.
//!
//! The loop's request dispatch is
//! [`crate::ftl::page_core::submit_request`], C's `page_ftl_submit_request`,
//! which lives in `page-core.c`. The segment reset the erase step runs is
//! [`crate::ftl::page_core::segment_data_init`] for the same reason.
//!
//! The flow is the C flow. The GC list is sorted by valid page count and its
//! head taken, the victim's free page count is zeroed so the copy cannot
//! allocate back into it, every LPN on the victim's list is read from its
//! current page and rewritten (which is what moves the mapping and drains the
//! list, since the write path invalidates the old mapping), the segment is
//! erased through `d_op->erase`, and the segment metadata and its
//! `gc_seg_bits` bit are reset.
//!
//! Deliberate deviations from C are listed in the [`crate::ftl`] module docs.

use core::ffi::c_int;
use core::ffi::c_void;
use core::slice;
use core::sync::atomic::Ordering;
use std::io;

use crate::bits::reset_bit;
use crate::device::{
    device_address, device_request, ffi_device_alloc_request, ffi_device_free_request,
    DeviceDirection, Request, DEVICE_DEFAULT_REQUEST,
};
use crate::ftl::page_core::{segment_data_init, submit_request};
use crate::ftl::page_read::read_page;
use crate::ftl::page_write::write_page;
use crate::ftl::{PageFtl, PageFtlSegment};
use crate::list::{ffi_list_remove, ffi_list_sort};
use crate::{pr_debug, pr_err};

/// C's `page_ftl_gc_list_cmp`: order two segments by their valid page count,
/// ascending, so the head of the sorted list is the best victim.
///
/// C loads both counters atomically, widens them to `uint64_t`, subtracts and
/// truncates back to `int`. `wrapping_sub` on `i32` is that same result. The
/// comparator is not declared in a header and nothing outside `page-gc.c` calls
/// it, so it stays private.
unsafe extern "C" fn gc_list_cmp(a: *const c_void, b: *const c_void) -> c_int {
    let a = unsafe { (*(a as *const PageFtlSegment)).nr_valid_pages.load(Ordering::SeqCst) };
    let b = unsafe { (*(b as *const PageFtlSegment)).nr_valid_pages.load(Ordering::SeqCst) };
    a.wrapping_sub(b)
}

/// C's `page_ftl_erase_end_rq`: release the request the erase was submitted
/// with. Runs on the device's completion path, so it only touches the request
/// pointer it is handed.
///
/// # Safety
/// `request` came from [`ffi_device_alloc_request`] and is not used afterwards.
unsafe extern "C" fn erase_end_rq(request: *mut device_request) {
    ffi_device_free_request(request);
}

/// C's `page_ftl_pick_gc_target`: sort the GC list, take its head and unlink
/// it, then pin the victim's free page count to 0 so the copy that follows
/// cannot allocate a page inside the segment it is draining.
///
/// Returns the segment pointer, or `None` when the list is empty.
fn pick_gc_target(pgftl: &mut PageFtl) -> Option<*mut PageFtlSegment> {
    if pgftl.gc_list.is_null() {
        return None;
    }

    pgftl.gc_list = unsafe { ffi_list_sort(pgftl.gc_list, Some(gc_list_cmp)) };
    let segment = unsafe { (*pgftl.gc_list).data as *mut PageFtlSegment };

    pr_debug!(
        "gc target: {} (valid: {}) => {:p}",
        unsafe { pgftl.segment_number(segment) },
        unsafe { (*segment).nr_valid_pages.load(Ordering::SeqCst) },
        segment
    );

    pgftl.gc_list = unsafe { ffi_list_remove(pgftl.gc_list, segment as *mut c_void) };
    unsafe { (*segment).nr_free_pages.store(0, Ordering::SeqCst) };
    Some(segment)
}

/// C's `page_ftl_segment_erase`: hand an erase request for `paddr` to the
/// device.
///
/// The request is owned by its completion callback, `erase_end_rq`, once the
/// backend accepts it. A backend that fails without running the callback
/// leaves the request to this function, which is why the failure path releases
/// it (C leaks it there).
fn segment_erase(pgftl: &PageFtl, paddr: device_address) -> io::Result<()> {
    let dev = pgftl.dev;

    let request = unsafe { ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST) };
    if request.is_null() {
        pr_err!("request allocation failed");
        return Err(io::Error::from_raw_os_error(libc::ENOMEM));
    }

    unsafe {
        (*request).flag = DeviceDirection::Erase.into();
        (*request).paddr = paddr;
        (*request).end_rq = Some(erase_end_rq);
    }

    let ret = unsafe {
        let ops = (*dev).d_op;
        let erase = (*ops).erase.expect("device backend has no erase operation");
        erase(dev, request)
    };
    if ret != 0 {
        pr_err!("erase error detected(errno: {})", ret);
        unsafe { ffi_device_free_request(request) };
        let errno = if ret < 0 { -ret } else { libc::EIO };
        return Err(io::Error::from_raw_os_error(errno));
    }
    Ok(())
}

/// C's `page_ftl_read_valid_page`: read the whole page `lpn` is mapped to.
///
/// The buffer is zeroed first, because the read path only fills the whole page
/// when the LPN is mapped, and the copy then writes whatever it got back.
///
/// # Errors
/// `ENOMEM` when the read request cannot be allocated, and `EFAULT` for every
/// failure of the read itself and for a short read, which is the collapse C
/// makes.
fn read_valid_page(pgftl: &mut PageFtl, lpn: usize) -> io::Result<Vec<u8>> {
    let page_size = pgftl.dev().page_size();
    let mut buffer = vec![0u8; page_size];

    let mut request = Request::new().map_err(|_| {
        pr_err!("request allocation failed");
        io::Error::from_raw_os_error(libc::ENOMEM)
    })?;
    request.flag = DeviceDirection::Read.into();
    request.data_len = page_size;
    request.sector = lpn * page_size;
    request.data = buffer.as_mut_ptr() as *mut c_void;
    request.end_rq = None;

    let ret = match read_page(pgftl, &mut request) {
        Ok(len) => len as isize,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    };
    if ret != page_size as isize {
        pr_err!(
            "invalid read size detected (expected: {}, actual: {})",
            page_size,
            ret
        );
        return Err(io::Error::from_raw_os_error(libc::EFAULT));
    }

    Ok(buffer)
}

/// C's `page_ftl_write_valid_page`: write `buffer` to a fresh page as `lpn`.
///
/// The write path allocates the page, remaps `lpn` to it and invalidates the
/// old mapping, so this is the step that both moves the payload and unlinks
/// `lpn` from the victim segment's list.
///
/// # Errors
/// `ENOMEM` when the write request cannot be allocated, and `EFAULT` for every
/// failure of the write itself and for a short write, which is the collapse C
/// makes.
fn write_valid_page(pgftl: &mut PageFtl, lpn: usize, buffer: &mut [u8]) -> io::Result<()> {
    let page_size = pgftl.dev().page_size();
    let data = &mut buffer[..page_size];

    let mut request = Request::new().map_err(|_| {
        pr_err!("request allocation failed");
        io::Error::from_raw_os_error(libc::ENOMEM)
    })?;
    request.flag = DeviceDirection::Write.into();
    request.data_len = page_size;
    request.sector = lpn * page_size;
    request.data = data.as_mut_ptr() as *mut c_void;
    request.end_rq = None;

    let ret = match write_page(pgftl, &mut request) {
        Ok(len) => len as isize,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    };
    if ret != page_size as isize {
        pr_err!(
            "invalid write size detected (expected: {}, actual: {})",
            page_size,
            ret
        );
        return Err(io::Error::from_raw_os_error(libc::EFAULT));
    }

    Ok(())
}

/// C's `page_ftl_valid_page_copy`: move every valid page of `segment` to a free
/// page elsewhere.
///
/// The walk captures the next node before each copy, because the copy's write
/// path invalidates the mapping it replaces and that unlinks and frees the node
/// the walk is sitting on (`ftl/page/page-write.c:40-41`). The captured node is
/// untouched by that removal.
///
/// # Errors
/// Propagates the first read or write failure, which stops the walk where it
/// is.
fn valid_page_copy(pgftl: &mut PageFtl, segment: *mut PageFtlSegment) -> io::Result<()> {
    let mut list = unsafe { (*segment).lpn_list };

    while !list.is_null() {
        let next = unsafe { (*list).next };
        let lpn = unsafe { (*list).data } as usize;

        let mut buffer = match read_valid_page(pgftl, lpn) {
            Ok(buffer) => buffer,
            Err(err) => {
                pr_err!("read valid page failed");
                return Err(err);
            }
        };
        if let Err(err) = write_valid_page(pgftl, lpn, &mut buffer) {
            pr_err!("write valid page failed");
            return Err(err);
        }

        list = next;
    }
    Ok(())
}

/// C's `page_ftl_do_gc`: garbage collect one segment.
///
/// Picks the queued segment with the fewest valid pages, copies its valid pages
/// elsewhere, erases it, reinitializes its metadata and clears its bit in
/// `gc_seg_bits`. An empty GC list is not an error: C logs and returns 0.
///
/// # Errors
/// The copy's error when a valid page cannot be relocated, and the erase's
/// error when the device fails, which is C's order. Both leave the victim
/// unlinked from the GC list and its `gc_seg_bits` bit set, as C does, because
/// the bit is only cleared after the erase.
pub fn do_gc(pgftl: &mut PageFtl) -> io::Result<()> {
    let Some(segment) = pick_gc_target(pgftl) else {
        pr_debug!("gc target segment doesn't exist");
        return Ok(());
    };
    let segnum = unsafe { pgftl.segment_number(segment) };
    pr_debug!("current segnum: {}", segnum);

    if let Err(err) = valid_page_copy(pgftl, segment) {
        pr_err!("valid page copy failed");
        return Err(err);
    }

    // C: `paddr.lpn = 0; paddr.format.block = (uint16_t)segnum;`.
    let paddr = device_address::from_segment_page(segnum as u16, 0);
    if let Err(err) = segment_erase(pgftl, paddr) {
        pr_err!("do erase failed");
        return Err(err);
    }

    unsafe { segment_data_init(pgftl, segment) };
    reset_bit(
        unsafe { slice::from_raw_parts_mut(pgftl.gc_seg_bits, segnum / 64 + 1) },
        segnum as u64,
    );

    Ok(())
}

/// C's `page_ftl_gc_from_list`: garbage collect up to `gc_ratio * nr_segments`
/// segments, stopping early once the GC list is empty.
///
/// The return value is C's, and it has two quirks worth naming because the
/// tests pin them:
///
/// - The loop `break`s as soon as the list is empty, *before* the counter
///   increments (`ftl/page/page-gc.c:317-321`), so a call that drains the list
///   reports one fewer segment than it erased. The count is right when the
///   ratio bound stops the loop instead.
/// - A request the dispatch hands to the read or write path reports that
///   path's byte count, and the loop returns it as if it were the segment
///   count (`ftl/page/page-gc.c:312-316`). Every caller in the tree passes
///   `DEVICE_ERASE`.
///
/// # Errors
/// The failing dispatch's error, unchanged.
pub fn gc_from_list(
    pgftl: &mut PageFtl,
    request: &mut device_request,
    gc_ratio: f64,
) -> io::Result<isize> {
    let nr_segments = pgftl.dev().nr_segments();
    let nr_gc_segments = (nr_segments as f64 * gc_ratio) as usize;

    let mut idx = 0usize;
    while idx < nr_gc_segments {
        let ret = submit_request(pgftl, request)?;
        if ret != 0 {
            pr_err!("garbage collection from list failed");
            return Ok(ret);
        }
        if pgftl.gc_list.is_null() {
            break;
        }
        idx += 1;
    }

    Ok(idx as isize)
}

/// C-ABI entry point for [`do_gc`], mirroring C's `page_ftl_do_gc`.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with its `segments`, `gc_list` and
/// `gc_seg_bits` initialized.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_do_gc(pgftl: *mut PageFtl) -> isize {
    if pgftl.is_null() {
        pr_err!("page FTL is NULL");
        return -(libc::EINVAL as isize);
    }

    match do_gc(&mut *pgftl) {
        Ok(()) => 0,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    }
}

/// C-ABI entry point for [`gc_from_list`], mirroring C's
/// `page_ftl_gc_from_list`.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with its `segments`, `gc_list` and
/// `gc_seg_bits` initialized, and `request` is `NULL` or a live request whose
/// flag selects the dispatch.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_gc_from_list(
    pgftl: *mut PageFtl,
    request: *mut device_request,
    gc_ratio: f64,
) -> isize {
    if pgftl.is_null() || request.is_null() {
        pr_err!("page FTL or request is NULL");
        return -(libc::EINVAL as isize);
    }

    match gc_from_list(&mut *pgftl, &mut *request, gc_ratio) {
        Ok(count) => count,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::set_bit;
    use crate::device::DEVICE_PAGE_SIZE;
    use crate::ftl::test_support::{
        read_at, read_physical_page, write_at, write_physical_page, Fixture,
    };
    use crate::list::ffi_list_prepend;
    use core::ptr;

    /// Four segments of four pages each: enough for two full victim segments, a
    /// destination segment and one segment left free.
    const NR_SEGMENTS: usize = 4;
    const PAGES_PER_SEGMENT: usize = 4;
    /// The LPNs [`two_queued_victims`] overwrites, and with them the pages whose
    /// first eight bytes differ from the tag the fill step wrote.
    const OVERWRITTEN: [usize; 4] = [0, 4, 5, 6];
    /// Payload length every write uses. Position dependent tags make a copy
    /// that reads or writes the wrong page show up as a mismatched byte.
    const LEN: usize = 16;

    /// The 16 bytes the fill step writes into `lpn`'s page.
    fn untouched_page(lpn: usize) -> Vec<u8> {
        vec![0x40 + lpn as u8; LEN]
    }

    /// The 8 bytes [`two_queued_victims`] splices over the head of `lpn`'s
    /// page.
    fn overwrite_bytes(lpn: usize) -> Vec<u8> {
        vec![0xa0 + lpn as u8; 8]
    }

    /// What `lpn`'s page must read back as once it has been relocated.
    fn expected_page(lpn: usize) -> Vec<u8> {
        let mut page = untouched_page(lpn);
        if OVERWRITTEN.contains(&lpn) {
            page[..8].copy_from_slice(&overwrite_bytes(lpn));
        }
        page
    }

    /// Read `lpn`'s page back through the FTL and compare it against
    /// [`expected_page`]. This is what proves the copy carried the payload.
    fn assert_page_payload(f: &mut Fixture, lpn: usize) {
        let mut buf = vec![0u8; LEN];
        assert_eq!(read_at(f, lpn * DEVICE_PAGE_SIZE, &mut buf).unwrap(), LEN);
        assert_eq!(buf, expected_page(lpn), "payload of lpn {lpn}");
    }

    /// A fixture with segments 0 and 1 full, both on the GC list.
    ///
    /// Segment 1 ends up with one valid page and segment 0 with three, and the
    /// list head is segment 0, so the greedy sort has to reorder the list
    /// before it can pick the right victim.
    fn two_queued_victims() -> Fixture {
        let mut f = Fixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);

        // Two full segments: writes 0-3 land in segment 0, writes 4-7 in
        // segment 1, because the allocator skips the full segment 0.
        for lpn in 0..2 * PAGES_PER_SEGMENT {
            let mut payload = untouched_page(lpn);
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                LEN
            );
        }

        // Three overwrites inside segment 1 drain it to one valid page and
        // queue it on the first invalidation.
        for lpn in &OVERWRITTEN[1..] {
            let mut payload = overwrite_bytes(*lpn);
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                8
            );
        }
        // One overwrite inside segment 0 queues it after segment 1, so the list
        // head holds the *most* valid pages.
        let mut payload = overwrite_bytes(OVERWRITTEN[0]);
        assert_eq!(write_at(&mut f, 0, &mut payload).unwrap(), 8);

        assert_eq!(f.free(0), 0);
        assert_eq!(f.valid(0), 3);
        assert_eq!(f.lpn_list_len(0), 3);
        assert_eq!(f.free(1), 0);
        assert_eq!(f.valid(1), 1);
        assert_eq!(f.lpn_list_len(1), 1);
        assert_eq!(f.gc_list_len(), 2);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));
        assert!(f.gc_bit(0) && f.gc_bit(1));

        f
    }

    /// Put `segnum` on the GC list even though it still has free pages and
    /// valid pages to relocate. `invalidate` only queues a segment with no free
    /// page left, so this is the state a segment reaches through any other
    /// producer of the list.
    fn queue_segment(f: &mut Fixture, segnum: usize) {
        let segment = f.segment_ptr(segnum) as *mut PageFtlSegment as *mut c_void;
        f.pgftl.gc_list = unsafe { ffi_list_prepend(f.pgftl.gc_list, segment) };
        set_bit(&mut f.gc_seg_bits, segnum as u64);
    }

    /// An erase request, the flag C's callers set before
    /// `page_ftl_gc_from_list`.
    fn erase_request() -> Request {
        let mut request = Request::new().expect("allocate a request");
        request.flag = DeviceDirection::Erase.into();
        request.end_rq = None;
        request
    }

    /// Call the C ABI entry point for [`do_gc`] on the fixture's FTL, so a test
    /// that swaps a device operation can hand a plain function to the fixture.
    fn do_gc_abi(f: &mut Fixture) -> isize {
        unsafe { ffi_page_ftl_do_gc(f.pgftl.as_mut()) }
    }

    #[test]
    fn gc_relocates_the_valid_pages_and_erases_the_victim() {
        let mut f = Fixture::new(3, PAGES_PER_SEGMENT);

        // Fill segment 0, then overwrite its first page: the new page goes to
        // segment 1 and the invalidation queues segment 0.
        for lpn in 0..PAGES_PER_SEGMENT {
            let mut payload = untouched_page(lpn);
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                LEN
            );
        }
        let mut payload = overwrite_bytes(0);
        assert_eq!(write_at(&mut f, 0, &mut payload).unwrap(), 8);

        assert_eq!(f.free(0), 0);
        assert_eq!(f.valid(0), 3);
        assert_eq!(f.lpn_list_len(0), 3);
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));
        assert!(f.gc_bit(0));
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32 - 1);

        // Relocate the three remaining pages, erase segment 0, reinitialize it.
        assert!(do_gc(&mut f.pgftl).is_ok());

        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(0), 0);
        assert_eq!(f.lpn_list_len(0), 0);
        assert_eq!(f.use_bits_word(0), 0);
        assert!(!f.gc_bit(0));
        assert_eq!(f.gc_list_len(), 0);

        // The copies landed in segment 1, which is now full, and every LPN
        // still resolves to its payload.
        assert_eq!(f.free(1), 0);
        assert_eq!(f.valid(1), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.lpn_list_len(1), PAGES_PER_SEGMENT);
        for lpn in 0..PAGES_PER_SEGMENT {
            assert_eq!(device_address::from(f.trans_map[lpn]).block(), 1);
            assert_page_payload(&mut f, lpn);
        }

        // The erased pages are gone from the device, which is what the
        // `DEVICE_ERASE` reached.
        let mut erased = vec![0xffu8; DEVICE_PAGE_SIZE];
        read_physical_page(
            &mut f,
            device_address::from_segment_page(0, 0).lpn,
            &mut erased,
        );
        assert!(erased.iter().all(|byte| *byte == 0));

        // An empty GC list is not an error, and it changes nothing. C returns 0
        // there (`page-gc.c:262-265`).
        assert!(do_gc(&mut f.pgftl).is_ok());
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(1), PAGES_PER_SEGMENT as i32);
        assert_eq!(
            unsafe { ffi_page_ftl_do_gc(f.pgftl.as_mut()) },
            0,
            "an empty list crosses the ABI as 0"
        );
    }

    #[test]
    fn greedy_victim_choice_prefers_the_least_valid_segment() {
        let mut f = two_queued_victims();

        assert!(do_gc(&mut f.pgftl).is_ok());

        // Segment 1, the least valid one, was drained and reinitialized even
        // though segment 0 was the list head.
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(1), 0);
        assert_eq!(f.lpn_list_len(1), 0);
        assert_eq!(f.use_bits_word(1), 0);
        assert!(!f.gc_bit(1));

        // Its single valid page moved into the only segment with free pages,
        // and segment 0 is untouched and still queued.
        assert_eq!(f.free(3), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.valid(3), 1);
        assert_eq!(
            f.trans_map[7],
            device_address::from_segment_page(3, 0).lpn
        );
        assert_page_payload(&mut f, 7);
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));
        assert!(f.gc_bit(0));
        assert_eq!(f.valid(0), 3);
        assert_eq!(f.free(0), 0);

        // A second pass takes segment 0, the last queued victim.
        assert!(do_gc(&mut f.pgftl).is_ok());

        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(0), 0);
        assert_eq!(f.lpn_list_len(0), 0);
        assert_eq!(f.use_bits_word(0), 0);
        assert!(!f.gc_bit(0));
        assert_eq!(f.gc_list_len(), 0);
        assert_eq!(f.free(3), 0);
        assert_eq!(f.valid(3), PAGES_PER_SEGMENT as i32);
        for lpn in 1..PAGES_PER_SEGMENT {
            assert_page_payload(&mut f, lpn);
        }
    }

    #[test]
    fn gc_ratio_caps_the_segments_one_call_processes() {
        let mut f = two_queued_victims();

        // A zero ratio collects nothing, without even looking at the list.
        let mut request = erase_request();
        assert_eq!(
            gc_from_list(&mut f.pgftl, &mut request, 0.0).unwrap(),
            0
        );
        assert_eq!(f.gc_list_len(), 2);
        assert_eq!(f.valid(0), 3);
        assert_eq!(f.valid(1), 1);

        // A quarter of four segments is one segment, so the call collects
        // exactly one victim and leaves the other queued. The count is right
        // because the ratio bound stopped the loop, not the empty list.
        assert_eq!(gc_from_list(&mut f.pgftl, &mut request, 0.25).unwrap(), 1);
        assert_eq!(f.valid(1), 0);
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));
        assert_eq!(f.valid(0), 3);
        assert!(f.gc_bit(0));

        // A full ratio reaches the second victim, which empties the list. C
        // reports one fewer than it erased there (`page-gc.c:317-321`), and the
        // port keeps that: the loop breaks before the counter increments.
        assert_eq!(gc_from_list(&mut f.pgftl, &mut request, 1.0).unwrap(), 0);
        assert_eq!(f.valid(0), 0);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert!(!f.gc_bit(0));
        assert_eq!(f.gc_list_len(), 0);

        // Nothing left to collect.
        assert_eq!(gc_from_list(&mut f.pgftl, &mut request, 1.0).unwrap(), 0);
    }

    #[test]
    fn gc_from_list_reports_the_dispatch_result() {
        let mut f = Fixture::new(2, PAGES_PER_SEGMENT);
        let mut request = erase_request();

        // C's dispatch returns the read or write byte count, and the loop hands
        // it straight back instead of the segment count (`page-gc.c:312-316`).
        let mut payload = untouched_page(0);
        request.flag = DeviceDirection::Write.into();
        request.sector = 0;
        request.data_len = LEN;
        request.data = payload.as_mut_ptr() as *mut c_void;
        assert_eq!(gc_from_list(&mut f.pgftl, &mut request, 1.0).unwrap(), 16);
        assert_eq!(f.trans_map[0], 0);
        assert_eq!(f.valid(0), 1);

        let mut buf = vec![0u8; LEN];
        request.flag = DeviceDirection::Read.into();
        request.sector = 0;
        request.data_len = LEN;
        request.data = buf.as_mut_ptr() as *mut c_void;
        assert_eq!(gc_from_list(&mut f.pgftl, &mut request, 1.0).unwrap(), 16);
        assert_eq!(buf, untouched_page(0));

        // An undefined flag is C's `-EINVAL` (`page-core.c:372-374`), and it
        // crosses the ABI the same way.
        request.flag = 3;
        assert_eq!(
            gc_from_list(&mut f.pgftl, &mut request, 1.0)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        unsafe {
            assert_eq!(
                ffi_page_ftl_gc_from_list(f.pgftl.as_mut(), request.as_ptr(), 1.0),
                -(libc::EINVAL as isize)
            );
        }
    }

    #[test]
    fn pick_gc_target_zeroes_the_victim_free_pages() {
        let mut f = Fixture::new(2, PAGES_PER_SEGMENT);

        // Two pages in segment 0 leaves it with free pages, so only a manually
        // queued segment reaches `pick_gc_target` in that state.
        for lpn in 0..2 {
            let mut payload = untouched_page(lpn);
            assert_eq!(
                write_at(&mut f, lpn * DEVICE_PAGE_SIZE, &mut payload).unwrap(),
                LEN
            );
        }
        assert_eq!(f.free(0), 2);
        queue_segment(&mut f, 0);

        // The copy's first read fails, so `do_gc` stops after the pick.
        assert_eq!(
            unsafe { f.with_failing_read(do_gc_abi) },
            -(libc::EFAULT as isize)
        );

        // The victim was unlinked from the list, its free page count was pinned
        // to 0 (`page-gc.c:67`) and its `gc_seg_bits` bit stays set because the
        // erase never ran (`page-gc.c:289`).
        assert_eq!(f.gc_list_len(), 0);
        assert_eq!(f.free(0), 0);
        assert_eq!(f.valid(0), 2);
        assert_eq!(f.lpn_list_len(0), 2);
        assert!(f.gc_bit(0));

        // Nothing was relocated: both LPNs still point into segment 0.
        for lpn in 0..2 {
            assert_eq!(
                f.trans_map[lpn],
                device_address::from_segment_page(0, lpn as u32).lpn
            );
        }
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32);
    }

    #[test]
    fn device_write_failure_during_the_copy_aborts_before_the_erase() {
        let mut f = two_queued_victims();

        // The copy's first write targets segment 3 page 0 (both other segments
        // are full). Program that page behind the FTL's back: the ramdisk
        // refuses to write a page twice, so the copy's write fails on the
        // device.
        let mut page = vec![0x5au8; DEVICE_PAGE_SIZE];
        write_physical_page(
            &mut f,
            device_address::from_segment_page(3, 0).lpn,
            &mut page,
        );

        // The write path's device error collapses to `-EFAULT`
        // (`page-gc.c:198-202`).
        assert_eq!(
            unsafe { ffi_page_ftl_do_gc(f.pgftl.as_mut()) },
            -(libc::EFAULT as isize)
        );

        // Segment 1, the chosen victim, was unlinked but not erased: its one
        // valid page still resolves into it and its bit is still set, while
        // segment 0 stays queued behind it.
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));
        assert!(f.gc_bit(1));
        assert_eq!(f.valid(1), 1);
        assert_eq!(f.lpn_list_len(1), 1);
        assert_eq!(f.free(1), 0);
        assert_eq!(f.use_bits_word(1), 0b1111);
        assert_eq!(
            f.trans_map[7],
            device_address::from_segment_page(1, 3).lpn
        );

        // The page the allocator handed out stays taken, which is what the
        // write path does when the device write fails.
        assert_eq!(f.free(3), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(f.valid(3), 1);

        // The segment's pages are still programmed, so the erase did not run.
        let mut still_there = vec![0u8; DEVICE_PAGE_SIZE];
        read_physical_page(
            &mut f,
            device_address::from_segment_page(1, 3).lpn,
            &mut still_there,
        );
        assert!(still_there[..LEN].iter().all(|byte| *byte == 0x47));
    }

    #[test]
    fn device_erase_failure_keeps_the_segment_queued() {
        let mut f = two_queued_victims();

        // The copy succeeds, then the erase fails: the erase's own errno is
        // what crosses the ABI (`page-gc.c:277-281`).
        assert_eq!(
            unsafe { f.with_failing_erase(do_gc_abi) },
            -(libc::EIO as isize)
        );

        // The victim's valid page moved to segment 3, its list and counter
        // drained by the copy.
        assert_eq!(
            f.trans_map[7],
            device_address::from_segment_page(3, 0).lpn
        );
        assert_page_payload(&mut f, 7);
        assert_eq!(f.valid(1), 0);
        assert_eq!(f.lpn_list_len(1), 0);

        // The segment is not reinitialized: no free pages, `use_bits` still
        // set, and its `gc_seg_bits` bit still raised for the next pass.
        assert_eq!(f.free(1), 0);
        assert_eq!(f.use_bits_word(1), 0b1111);
        assert!(f.gc_bit(1));
        assert_eq!(f.gc_list_len(), 1);
        assert_eq!(f.gc_list_head(), f.segment_ptr(0));

        // The device was not erased: segment 1's first page still holds the
        // bytes lpn 4 was written with.
        let mut still_there = vec![0u8; DEVICE_PAGE_SIZE];
        read_physical_page(
            &mut f,
            device_address::from_segment_page(1, 0).lpn,
            &mut still_there,
        );
        assert!(still_there[..LEN]
            .iter()
            .all(|byte| *byte == 0x40 + 4));
    }

    #[test]
    fn null_ftl_or_request_is_rejected_on_the_abi() {
        let mut f = Fixture::new(2, PAGES_PER_SEGMENT);
        let request = erase_request();

        unsafe {
            assert_eq!(
                ffi_page_ftl_do_gc(ptr::null_mut()),
                -(libc::EINVAL as isize)
            );
            assert_eq!(
                ffi_page_ftl_gc_from_list(ptr::null_mut(), request.as_ptr(), 0.1),
                -(libc::EINVAL as isize)
            );
            assert_eq!(
                ffi_page_ftl_gc_from_list(f.pgftl.as_mut(), ptr::null_mut(), 0.1),
                -(libc::EINVAL as isize)
            );
        }

        // The guard runs before anything is touched.
        assert_eq!(f.gc_list_len(), 0);
        assert_eq!(f.free(0), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(0), 0);
    }
}
