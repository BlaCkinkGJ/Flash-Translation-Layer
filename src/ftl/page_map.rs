//! Page-level address mapping, ported from `ftl/page/page-map.c`.
//!
//! [`get_free_page`] hands out a physical page for the next write. It uses the
//! same round-robin cursor (`pgftl->alloc_segnum`) and first-zero-bit scan as
//! C. [`update_map`] writes one L2P entry. Both export an `ffi_`-prefixed
//! raw-pointer entry point next to the native function the Rust callers use.
//!
//! The free-page path mirrors C's `__ATOMIC_SEQ_CST` updates of
//! `nr_free_pages` and `nr_valid_pages`, and its `find_first_zero_bit` scan. It
//! keeps C's "skip and retry" behaviour when the free counter and the bitmap
//! disagree. C logs a warning and moves to the next segment.
//!
//! The packed address mirrors C exactly.
//! `paddr.format.block = (uint16_t)segnum` then `paddr.lpn |= page`, so the
//! segment number truncates to 16 bits the same way C's cast does. The low
//! (bus + chip + page) bits carry the page-within-segment index.

use core::ffi::c_int;
use core::sync::atomic::Ordering;
use std::io;

use crate::bits::{find_first_zero_bit, get_bit, BITS_NOT_FOUND};
use crate::device::{device_address, PADDR_EMPTY};
use crate::ftl::PageFtl;
use crate::{pr_err, pr_warn};

/// Pick the next free physical page. C's `page_ftl_get_free_page` does the
/// same.
///
/// Starts at `pgftl.alloc_segnum` and walks every segment once, skipping bad
/// segments and segments with no free pages. It then returns the first zero bit
/// in the chosen segment's `use_bits` bitmap, marks the page used, decrements
/// `nr_free_pages`, increments `nr_valid_pages`, and advances `alloc_segnum`.
/// Returns `None` when nothing is free, where C returns a [`PADDR_EMPTY`] word.
/// Take [`PageFtl`] with its mutex held. This takes `&mut` because C reads
/// `alloc_segnum` and the counters concurrently.
pub fn get_free_page(pgftl: &mut PageFtl) -> Option<device_address> {
    if pgftl.segments.is_empty() {
        pr_err!("fatal error detected: segment array is NULL");
        return None;
    }

    let dev = pgftl.dev();
    let nr_segments = dev.nr_segments();
    let pages_per_segment = dev.pages_per_segment();
    let badseg_bitmap = dev.badseg_bitmap;

    let mut idx = 0usize;
    loop {
        if idx == nr_segments {
            pr_err!("cannot find the free page in the device");
            return None;
        }
        let segnum = (pgftl.alloc_segnum as usize + idx) % nr_segments;
        idx += 1;

        if !badseg_bitmap.is_null() {
            let bucket = segnum / 64;
            let bits = unsafe { core::slice::from_raw_parts(badseg_bitmap, bucket + 1) };
            if get_bit(bits, segnum as u64) {
                continue;
            }
        }

        let nr_free_pages = pgftl.segments[segnum].free_pages();
        if nr_free_pages == 0 {
            continue;
        }
        pgftl.alloc_segnum = segnum as u64;

        let page = find_first_zero_bit(
            &pgftl.segments[segnum].use_bits,
            pages_per_segment as u64,
            0,
        );
        if page == BITS_NOT_FOUND {
            pr_warn!(
                "nr_free_pages and use_bits bitmap are not synchronized(nr_free_pages: {}, page: {})",
                nr_free_pages,
                page
            );
            continue;
        }

        // C: `paddr.lpn = 0; paddr.format.block = (uint16_t)segnum; paddr.lpn |= page;`.
        let paddr = device_address::from_segment_page(segnum as u16, page as u32);

        pgftl.segments[segnum].set_use_bit(page);
        pgftl.segments[segnum]
            .nr_free_pages
            .fetch_sub(1, Ordering::SeqCst);
        pgftl.segments[segnum]
            .nr_valid_pages
            .fetch_add(1, Ordering::SeqCst);

        return Some(paddr);
    }
}

/// Store `ppn` for the logical page holding `sector`. C's
/// `page_ftl_update_map` does the same.
///
/// Fails with `EINVAL` when the logical page number would run past the L2P
/// table, exactly like C.
pub fn update_map(pgftl: &mut PageFtl, sector: usize, ppn: u32) -> io::Result<()> {
    let lpn = pgftl.lpn(sector) as u64;
    let map_size = pgftl.map_entries() as u64;
    if lpn >= map_size {
        pr_err!(
            "lpn value overflow detected (max: {}, cur: {})",
            map_size,
            lpn
        );
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }

    pgftl.trans_map[lpn as usize] = ppn;
    Ok(())
}

/// The C-ABI entry point for [`get_free_page`]. The native `None` becomes
/// `PADDR_EMPTY`, and a `NULL` FTL also yields `PADDR_EMPTY`, where C would
/// dereference it.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with its geometry and `segments`
/// array initialized.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_get_free_page(pgftl: *mut PageFtl) -> device_address {
    match pgftl.as_mut() {
        Some(pgftl) => get_free_page(pgftl).unwrap_or(device_address::from(PADDR_EMPTY)),
        None => {
            pr_err!("page FTL is NULL");
            device_address::from(PADDR_EMPTY)
        }
    }
}

/// The C-ABI entry point for [`update_map`]. The native error becomes its
/// negated `errno`, which is the `-EINVAL` C returns, and a `NULL` FTL also
/// yields `-EINVAL`, where C would dereference it.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL with `trans_map` and its geometry
/// initialized.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_update_map(
    pgftl: *mut PageFtl,
    sector: usize,
    ppn: u32,
) -> c_int {
    match pgftl.as_mut() {
        Some(pgftl) => match update_map(pgftl, sector, ppn) {
            Ok(()) => 0,
            Err(err) => -(err.raw_os_error().unwrap_or(libc::EINVAL)),
        },
        None => {
            pr_err!("page FTL is NULL");
            -libc::EINVAL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::set_bit;
    use crate::device::{
        device, DEVICE_NR_BUS_BITS, DEVICE_NR_CHIPS_BITS, DEVICE_NR_PAGES_BITS,
        DEVICE_PAGE_SIZE,
    };
    use crate::ftl::PageFtlSegment;
    use core::mem::MaybeUninit;

    /// Small but valid geometry: 3 segments of 16 pages each.
    ///
    /// The L2P table, the segment array and its `use_bits` bitmaps belong to
    /// the FTL, so the tests read them through `f.pgftl`.
    struct Fixture {
        dev: Box<device>,
        badseg: Vec<u64>,
        pgftl: PageFtl,
    }

    impl Fixture {
        fn new() -> Self {
            // Boxed so `pgftl.dev` stays valid when the `Fixture` moves.
            let mut dev = Box::new(unsafe { MaybeUninit::<device>::zeroed().assume_init() });
            dev.info.nr_bus = 2;
            dev.info.nr_chips = 2;
            dev.info.package.nr_blocks = 3;
            dev.info.package.block.nr_pages = 4;
            dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

            let mut badseg = vec![0u64; 1];
            let total = dev.total_size();

            let mut pgftl = PageFtl::empty();
            pgftl.dev = &mut *dev;
            pgftl.trans_map = vec![u32::MAX; total / DEVICE_PAGE_SIZE + 1];
            // One `u64` per segment: `pages_per_segment` is 16 here.
            pgftl.segments = (0..3)
                .map(|_| PageFtlSegment::empty(vec![0u64; 1], 16))
                .collect();

            dev.badseg_bitmap = badseg.as_mut_ptr();

            Fixture { dev, badseg, pgftl }
        }

        fn set_free(&mut self, segnum: usize, n: i32) {
            self.pgftl.segments[segnum]
                .nr_free_pages
                .store(n, Ordering::SeqCst);
        }

        fn free(&self, segnum: usize) -> i32 {
            self.pgftl.segments[segnum].free_pages()
        }
    }

    #[test]
    fn round_robin_walks_segments_in_order() {
        let mut f = Fixture::new();
        // 1 free page in every segment, all bitmaps clear.
        for segnum in 0..3 {
            f.set_free(segnum, 1);
        }

        let first = get_free_page(&mut f.pgftl).expect("segment 0 has one free page");
        assert_eq!(first, device_address::new(0, 0, 0, 0));
        assert_eq!(first.lpn, 0);
        assert_eq!(f.pgftl.alloc_segnum, 0);
        assert_eq!(f.free(0), 0);

        let second = get_free_page(&mut f.pgftl).expect("segment 1 has one free page");
        assert_eq!(second.block(), 1);
        assert_eq!(
            second.lpn,
            1 << (DEVICE_NR_BUS_BITS
                + DEVICE_NR_CHIPS_BITS
                + DEVICE_NR_PAGES_BITS)
        );
        assert_eq!(f.pgftl.alloc_segnum, 1);

        let third = get_free_page(&mut f.pgftl).expect("segment 2 has one free page");
        assert_eq!(third.block(), 2);
        assert_eq!(f.pgftl.alloc_segnum, 2);

        // Every segment is full now.
        assert_eq!(get_free_page(&mut f.pgftl), None);
    }

    #[test]
    fn first_zero_bit_picks_lowest_free_page() {
        let mut f = Fixture::new();
        f.set_free(0, 2);
        for segnum in 1..3 {
            f.set_free(segnum, 0);
        }
        // Pages 0 and 1 used, page 2 free.
        set_bit(&mut f.pgftl.segments[0].use_bits, 0);
        set_bit(&mut f.pgftl.segments[0].use_bits, 1);

        let addr = get_free_page(&mut f.pgftl).expect("segment 0 has a free page");
        assert_eq!(addr.block(), 0);
        assert_eq!(addr.lpn & 0x7ff, 2);
        assert_eq!(f.free(0), 1);
        assert_eq!(f.pgftl.segments[0].nr_valid_pages.load(Ordering::SeqCst), 1);
        assert!(f.pgftl.segments[0].use_bit(2));
    }

    #[test]
    fn bad_segments_are_skipped() {
        let mut f = Fixture::new();
        for segnum in 0..3 {
            f.set_free(segnum, 1);
        }
        set_bit(&mut f.badseg, 0);
        f.dev.badseg_bitmap = f.badseg.as_mut_ptr();

        let addr = get_free_page(&mut f.pgftl).expect("segment 1 has a free page");
        assert_eq!(addr.block(), 1);
        assert_eq!(f.pgftl.alloc_segnum, 1);
    }

    #[test]
    fn update_map_writes_in_range_and_rejects_overflow() {
        let mut f = Fixture::new();

        assert!(update_map(&mut f.pgftl, 7, 0xabcd).is_ok());
        assert_eq!(f.pgftl.trans_map[0], 0xabcd);

        // Second page of the device.
        let sector = DEVICE_PAGE_SIZE + 3;
        assert!(update_map(&mut f.pgftl, sector, 0x1234).is_ok());
        assert_eq!(f.pgftl.trans_map[1], 0x1234);

        // Past the end of the table.
        let last = f.pgftl.trans_map.len() - 1;
        let overflow = (last + 1) * DEVICE_PAGE_SIZE;
        let err = update_map(&mut f.pgftl, overflow, 1).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn bitmap_desync_retries_to_next_segment() {
        let mut f = Fixture::new();
        // Every segment claims one free page, but segment 0's bitmap is full:
        // its free counter and its bitmap disagree.
        for segnum in 0..3 {
            f.set_free(segnum, 1);
        }
        for bit in 0..16 {
            set_bit(&mut f.pgftl.segments[0].use_bits, bit);
        }

        let addr = get_free_page(&mut f.pgftl).expect("segment 1 has a free page");
        // Segment 0's stale counter made its scan miss, so the page comes from
        // segment 1.
        assert_eq!(addr.block(), 1);
        // Rust's find_first_zero_bit clamps to `size` and returns
        // BITS_NOT_FOUND, so the port warns, retries, and re-assigns
        // alloc_segnum to the segment that supplied the page. It does not stay
        // on the desynced segment 0. C's scan returns a bit at or past
        // pages_per_segment here, hands out that out-of-range page from
        // segment 0, and does not retry. See the deliberate deviation in
        // `crate::ftl`.
        assert_eq!(f.pgftl.alloc_segnum, 1);
        // The desynced segment keeps its stale free-page counter, unlike C,
        // which decrements it for the out-of-range page.
        assert_eq!(f.free(0), 1);
    }

    #[test]
    fn zero_page_segments_report_no_free_page() {
        let mut f = Fixture::new();
        // A degenerate geometry: no pages in any segment, so every bitmap is
        // zero bits long. C scans nothing, warns, and ends with PADDR_EMPTY.
        f.dev.info.package.block.nr_pages = 0;

        assert_eq!(get_free_page(&mut f.pgftl), None);
        // C assigns `alloc_segnum` before the scan and `goto retry` walks on,
        // so the cursor ends on the segment whose scan failed last (0, 1, 0
        // for three segments), not on the last one visited.
        assert_eq!(f.pgftl.alloc_segnum, 0);
        // Nothing was handed out: no counter moved and no bit was set.
        assert_eq!(f.free(0), 16);
        assert_eq!(f.pgftl.segments[0].nr_valid_pages.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn ffi_null_ftl_is_rejected() {
        unsafe {
            assert_eq!(
                ffi_page_ftl_get_free_page(core::ptr::null_mut()),
                device_address::from(PADDR_EMPTY)
            );
            assert_eq!(
                ffi_page_ftl_update_map(core::ptr::null_mut(), 0, 0),
                -libc::EINVAL
            );
        }
    }

    #[test]
    fn ffi_dispatch_matches_native() {
        let mut f = Fixture::new();
        for segnum in 0..3 {
            f.set_free(segnum, 1);
        }
        let addr = unsafe { ffi_page_ftl_get_free_page(&mut f.pgftl) };
        assert_eq!(addr.block(), 0);
        assert_eq!(unsafe { ffi_page_ftl_update_map(&mut f.pgftl, 0, 99) }, 0);
        assert_eq!(f.pgftl.trans_map[0], 99);

        // The native `Err` crosses the ABI as the negated `errno`.
        let last = f.pgftl.trans_map.len() - 1;
        let overflow = (last + 1) * DEVICE_PAGE_SIZE;
        assert_eq!(
            unsafe { ffi_page_ftl_update_map(&mut f.pgftl, overflow, 1) },
            -libc::EINVAL
        );

        // The native `None` crosses the ABI as `PADDR_EMPTY`.
        for segnum in 0..3 {
            f.set_free(segnum, 0);
        }
        assert_eq!(
            unsafe { ffi_page_ftl_get_free_page(&mut f.pgftl) },
            device_address::from(PADDR_EMPTY)
        );
    }
}
