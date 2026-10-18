//! Page-mapping FTL types ported from `include/page.h`.
//!
//! Two layers live here, following the split in [`crate::device`]:
//!
//! - The C ABI layer: [`PageFtl`] and [`PageFtlSegment`] are `#[repr(C)]`
//!   mirrors of `struct page_ftl` / `struct page_ftl_segment`. C reads these by
//!   offset (`pgftl->trans_map`, `pgftl->alloc_segnum`, `pgftl->segments`,
//!   `pgftl->dev`, `segment->nr_free_pages`, `segment->use_bits`, ...), so the
//!   layout is the contract. It is pinned on both sides: `layout_matches_c`
//!   here, and the `PAGE_FTL_STATIC_ASSERT` block next to the C `struct`
//!   definitions. Both assert the same offset formulas, so either side
//!   drifting breaks a build.
//! - The native layer: safe accessors on [`PageFtl`] (`map_size`, `lpn`,
//!   `page_offset`, `segment_number`, `free_pages`) plus the address mapping
//!   in [`page_map`]. Rust callers (Tasks 9-13) use these and never touch raw
//!   pointers or `__atomic_*` builtins.
//!
//! `page_ftl_get_free_page` and `page_ftl_update_map` from `ftl/page/page-map.c`
//! live in [`page_map`], with `ffi_`-prefixed entry points so they can coexist
//! with the still-linked C implementations until the cutover.
//!
//! Deliberate deviations from the C original:
//!
//! - The structs use Rust names ([`PageFtl`] / [`PageFtlSegment`]). Only the
//!   layout matters to C, and Rust naming keeps them out of `#[allow]`s.
//! - The `rwlock` field is behind the `page_ftl_global_rwlock` cargo feature,
//!   mirroring C's `PAGE_FTL_USE_GLOBAL_RWLOCK` (the Makefile sets it for the
//!   non-default `USE_ZONE_DEVICE` build). The default layout omits it, and
//!   `layout_matches_c` asserts both the present and absent offsets.
//! - The header's `static inline` helpers are safe methods here, not exported
//!   C-ABI symbols. The C inlines stay live until the cutover.
//! - `nr_free_pages` and `nr_valid_pages` are `AtomicI32` fields. C reaches
//!   them only through `__atomic_*` builtins, and `AtomicI32` has the same size
//!   and alignment as `int`, so the offsets hold and Rust cannot touch them
//!   non-atomically. `is_gc` stays a plain `c_int` because no C code reads or
//!   writes it.

use core::sync::atomic::{AtomicI32, Ordering};

use libc::{pthread_mutex_t, pthread_rwlock_t, pthread_t};

use crate::device::device;
use crate::list::list_node_t;

pub mod page_map;

/// C's `PAGE_FTL_CACHE_SIZE`, the write-back cache slot count (unused while
/// `PAGE_FTL_USE_CACHE` stays commented out in the header).
pub const PAGE_FTL_CACHE_SIZE: usize = 1 << 10;

/// C's `PAGE_FTL_GC_RATIO`: maximum fraction of segments one GC pass collects.
pub const PAGE_FTL_GC_RATIO: f64 = 10.0 / 100.0;

/// C's `PAGE_FTL_GC_ALL`: collect every dirty segment.
pub const PAGE_FTL_GC_ALL: f64 = 1.0;

/// C's `PAGE_FTL_GC_THRESHOLD`: GC triggers once the free-page fraction drops
/// below this.
pub const PAGE_FTL_GC_THRESHOLD: f64 = 20.0 / 100.0;

/// C's `enum { PAGE_FTL_IOCTL_TRIM = 0 }`.
pub const PAGE_FTL_IOCTL_TRIM: core::ffi::c_int = 0;

/// Mirrors `struct page_ftl_segment` in `include/page.h`.
///
/// One entry per segment (block). `use_bits` is a bitmap of `pages_per_segment`
/// bits owned by the FTL, `lpn_list` links the valid pages for GC. Both are
/// borrowed raw pointers here because C allocates and frees them: see
/// `page_ftl_segment_data_init` in `ftl/page/page-core.c` (Task 9+).
///
/// The two counters are `AtomicI32` because C reaches them only through
/// `__atomic_load_n`, `__atomic_fetch_add`, `__atomic_fetch_sub` and
/// `__atomic_store_n` (see `page-map.c`, `page-write.c`, `page-gc.c`,
/// `page-core.c`). `AtomicI32` has the same size and alignment as `int`, which
/// `layout_matches_c` asserts, so the mirror keeps C's offsets. A plain field
/// would let Rust write them non-atomically and race with C.
///
/// `is_gc` is an `int` because nothing reads or writes it: the C sources never
/// touch `segment->is_gc`, and it is kept only because it is part of the C
/// layout (`use_bits` lands at 16 because of it).
#[repr(C)]
#[derive(Debug)]
pub struct PageFtlSegment {
    pub nr_free_pages: AtomicI32,
    pub nr_valid_pages: AtomicI32,
    pub is_gc: core::ffi::c_int,
    pub use_bits: *mut u64,
    pub lpn_list: *mut list_node_t,
}

impl PageFtlSegment {
    /// Set the bit for `page` in this segment's `use_bits` bitmap.
    ///
    /// # Safety
    /// `use_bits` points to at least `page / 64 + 1` writable `u64`s.
    pub unsafe fn set_use_bit(&mut self, page: u64) {
        let bucket = (page / 64) as usize;
        let bits = core::slice::from_raw_parts_mut(self.use_bits, bucket + 1);
        crate::bits::set_bit(bits, page);
    }

    /// Return whether `page` is marked used in this segment.
    ///
    /// # Safety
    /// `use_bits` points to at least `page / 64 + 1` readable `u64`s.
    pub unsafe fn use_bit(&self, page: u64) -> bool {
        let bucket = (page / 64) as usize;
        let bits = core::slice::from_raw_parts(self.use_bits, bucket + 1);
        crate::bits::get_bit(bits, page)
    }

    /// Atomically read `nr_free_pages`, mirroring C's `__atomic_load_n(...,
    /// __ATOMIC_SEQ_CST)`.
    pub fn free_pages(&self) -> i32 {
        self.nr_free_pages.load(Ordering::SeqCst)
    }
}

/// Mirrors `struct page_ftl` in `include/page.h`.
///
/// `trans_map` is the page-level L2P table (`uint32_t` per logical page),
/// `segments` a contiguous array of `nr_segments` [`PageFtlSegment`] entries,
/// and `alloc_segnum` the round-robin cursor. C reads `trans_map`,
/// `alloc_segnum`, `segments` and `dev` directly, so their offsets are pinned
/// by `layout_matches_c`.
#[repr(C)]
pub struct PageFtl {
    pub trans_map: *mut u32,
    pub alloc_segnum: u64,
    pub segments: *mut PageFtlSegment,
    pub dev: *mut device,
    pub mutex: pthread_mutex_t,
    pub gc_mutex: pthread_mutex_t,
    pub bus_rwlock: *mut pthread_rwlock_t,
    /// Present only under `-DPAGE_FTL_USE_GLOBAL_RWLOCK` in C. Enable the
    /// `page_ftl_global_rwlock` cargo feature to match that layout.
    #[cfg(feature = "page_ftl_global_rwlock")]
    pub rwlock: pthread_rwlock_t,
    pub gc_thread: pthread_t,
    pub o_flags: core::ffi::c_int,
    pub gc_list: *mut list_node_t,
    pub gc_seg_bits: *mut u64,
}

impl PageFtl {
    /// Borrow the backing device. Panics when `dev` is null, which C would
    /// dereference anyway.
    fn dev(&self) -> &device {
        unsafe { &*self.dev }
    }

    /// C's `page_ftl_get_map_size`: bytes in the L2P table.
    ///
    /// One extra `uint32_t` slot, so a full device still has a terminator.
    /// When `page_size` is 0 the division diverges from C: C raises SIGFPE,
    /// Rust panics, and a panic crossing an `extern "C"` boundary aborts.
    pub fn map_size(&self) -> usize {
        let dev = self.dev();
        (dev.total_size() / dev.page_size() + 1) * core::mem::size_of::<u32>()
    }

    /// Number of `uint32_t` slots in the L2P table, the unit [`page_map`]
    /// indexes with. C divides `page_ftl_get_map_size` by `sizeof(uint32_t)`
    /// at each call site.
    pub fn map_entries(&self) -> usize {
        self.map_size() / core::mem::size_of::<u32>()
    }

    /// C's `page_ftl_get_lpn`: logical page number of `sector`.
    pub fn lpn(&self, sector: usize) -> usize {
        sector / self.dev().page_size()
    }

    /// C's `page_ftl_get_page_offset`: byte offset of `sector` within its page.
    pub fn page_offset(&self, sector: usize) -> usize {
        sector % self.dev().page_size()
    }

    /// C's `page_ftl_get_segment_number`: index of `segment` within the
    /// `segments` array. Undefined for a pointer outside that array, as in C.
    ///
    /// # Safety
    /// `segment` points into the `segments` array.
    pub unsafe fn segment_number(&self, segment: *const PageFtlSegment) -> usize {
        (segment as usize - self.segments as usize) / core::mem::size_of::<PageFtlSegment>()
    }

    /// C's `page_ftl_get_free_pages`: sum of every segment's free-page count.
    ///
    /// # Safety
    /// `dev` and `segments` are live and `segments` holds `dev.nr_segments()`
    /// entries.
    pub unsafe fn free_pages(&self) -> usize {
        let nr_segments = self.dev().nr_segments();
        let mut free_pages = 0usize;
        for segnum in 0..nr_segments {
            let segment = &*self.segments.add(segnum);
            free_pages += segment.free_pages() as usize;
        }
        free_pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DEVICE_PAGE_SIZE;
    use core::mem::{align_of, offset_of, size_of, MaybeUninit};

    const PTR: usize = size_of::<*const core::ffi::c_void>();

    fn zeroed_pgftl() -> PageFtl {
        unsafe { MaybeUninit::<PageFtl>::zeroed().assume_init() }
    }

    #[test]
    fn layout_matches_c() {
        // The counters are `AtomicI32` in Rust and `int` in C. std guarantees
        // the size, and the alignment must match too or the offsets below move.
        assert_eq!(size_of::<AtomicI32>(), size_of::<core::ffi::c_int>());
        assert_eq!(align_of::<AtomicI32>(), align_of::<core::ffi::c_int>());

        // struct page_ftl_segment: 3 ints, then use_bits, lpn_list.
        assert_eq!(offset_of!(PageFtlSegment, nr_free_pages), 0);
        assert_eq!(
            offset_of!(PageFtlSegment, nr_valid_pages),
            size_of::<core::ffi::c_int>()
        );
        assert_eq!(
            offset_of!(PageFtlSegment, is_gc),
            2 * size_of::<core::ffi::c_int>()
        );
        assert_eq!(
            offset_of!(PageFtlSegment, use_bits),
            (3 * size_of::<core::ffi::c_int>()).next_multiple_of(PTR)
        );
        assert_eq!(
            offset_of!(PageFtlSegment, lpn_list),
            offset_of!(PageFtlSegment, use_bits) + PTR
        );
        assert_eq!(
            size_of::<PageFtlSegment>(),
            offset_of!(PageFtlSegment, lpn_list) + PTR
        );

        // struct page_ftl: trans_map, alloc_segnum, segments, dev, mutex,
        // gc_mutex, bus_rwlock, gc_thread, o_flags, gc_list, gc_seg_bits.
        assert_eq!(offset_of!(PageFtl, trans_map), 0);
        assert_eq!(offset_of!(PageFtl, alloc_segnum), PTR);
        assert_eq!(offset_of!(PageFtl, segments), PTR + size_of::<u64>());
        assert_eq!(offset_of!(PageFtl, dev), offset_of!(PageFtl, segments) + PTR);
        assert_eq!(
            offset_of!(PageFtl, mutex),
            (offset_of!(PageFtl, dev) + PTR).next_multiple_of(align_of::<pthread_mutex_t>())
        );
        assert_eq!(
            offset_of!(PageFtl, gc_mutex),
            offset_of!(PageFtl, mutex) + size_of::<pthread_mutex_t>()
        );
        assert_eq!(
            offset_of!(PageFtl, bus_rwlock),
            offset_of!(PageFtl, gc_mutex) + size_of::<pthread_mutex_t>()
        );
        // `rwlock` sits between `bus_rwlock` and `gc_thread` only under the
        // C `PAGE_FTL_USE_GLOBAL_RWLOCK` build.
        #[cfg(feature = "page_ftl_global_rwlock")]
        {
            assert_eq!(
                offset_of!(PageFtl, rwlock),
                offset_of!(PageFtl, bus_rwlock) + PTR
            );
            assert_eq!(
                offset_of!(PageFtl, gc_thread),
                offset_of!(PageFtl, rwlock) + size_of::<pthread_rwlock_t>()
            );
        }
        #[cfg(not(feature = "page_ftl_global_rwlock"))]
        assert_eq!(
            offset_of!(PageFtl, gc_thread),
            offset_of!(PageFtl, bus_rwlock) + PTR
        );
        assert_eq!(
            offset_of!(PageFtl, o_flags),
            offset_of!(PageFtl, gc_thread) + size_of::<pthread_t>()
        );
        assert_eq!(
            offset_of!(PageFtl, gc_list),
            (offset_of!(PageFtl, o_flags) + size_of::<core::ffi::c_int>())
                .next_multiple_of(PTR)
        );
        assert_eq!(
            offset_of!(PageFtl, gc_seg_bits),
            offset_of!(PageFtl, gc_list) + PTR
        );
        assert_eq!(
            size_of::<PageFtl>(),
            (offset_of!(PageFtl, gc_seg_bits) + PTR).next_multiple_of(align_of::<PageFtl>())
        );

        // PAGE_FTL_CACHE_SIZE is a bit shift, and the GC ratios are the exact
        // fractions the header spells out.
        assert_eq!(PAGE_FTL_CACHE_SIZE, 1024);
        assert_eq!(PAGE_FTL_GC_RATIO, 0.1);
        assert_eq!(PAGE_FTL_GC_ALL, 1.0);
        assert_eq!(PAGE_FTL_GC_THRESHOLD, 0.2);
        assert_eq!(PAGE_FTL_IOCTL_TRIM, 0);
    }

    #[test]
    fn geometry_helpers_match_c() {
        let mut dev = unsafe { MaybeUninit::<device>::zeroed().assume_init() };
        dev.info.nr_bus = 2;
        dev.info.nr_chips = 2;
        dev.info.package.nr_blocks = 4;
        dev.info.package.block.nr_pages = 128;
        dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

        let mut pgftl = zeroed_pgftl();
        pgftl.dev = &mut dev;

        // 4 segments * (2*2*128) pages * 8192 bytes.
        let total = 4 * 512 * DEVICE_PAGE_SIZE;
        assert_eq!(dev.total_size(), total);
        assert_eq!(pgftl.map_size(), (total / DEVICE_PAGE_SIZE + 1) * 4);
        assert_eq!(pgftl.map_entries(), total / DEVICE_PAGE_SIZE + 1);
        assert_eq!(pgftl.lpn(DEVICE_PAGE_SIZE + 7), 1);
        assert_eq!(pgftl.page_offset(DEVICE_PAGE_SIZE + 7), 7);
    }

    #[test]
    fn free_pages_sums_segments_and_sets_use_bit() {
        let mut use_bits = [0u64; 2];
        let mut segments = [PageFtlSegment {
            nr_free_pages: AtomicI32::new(0),
            nr_valid_pages: AtomicI32::new(0),
            is_gc: 0,
            use_bits: use_bits.as_mut_ptr(),
            lpn_list: core::ptr::null_mut(),
        }];
        segments[0].nr_free_pages.store(3, Ordering::SeqCst);

        let mut dev = unsafe { MaybeUninit::<device>::zeroed().assume_init() };
        dev.info.package.nr_blocks = 1;

        let mut pgftl = zeroed_pgftl();
        pgftl.dev = &mut dev;
        pgftl.segments = segments.as_mut_ptr();

        unsafe {
            assert_eq!(pgftl.free_pages(), 3);
            segments[0].set_use_bit(5);
            assert!(segments[0].use_bit(5));
            assert!(!segments[0].use_bit(6));
            assert_eq!(pgftl.segment_number(&segments[0]), 0);
        }
    }
}
