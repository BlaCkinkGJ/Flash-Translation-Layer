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
//! with the still-linked C implementations until the cutover. The read, write,
//! garbage collection and core lifecycle paths live in [`page_read`],
//! [`page_write`], [`page_gc`] and [`page_core`] the same way.
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
//! - When a segment's `nr_free_pages` and its `use_bits` bitmap disagree, the
//!   port refuses the out-of-range page C hands out. C's `find_first_zero_bit`
//!   (`include/bits.h:63`, called from `ftl/page/page-map.c:71` as
//!   `find_first_zero_bit(segment->use_bits, pages_per_segment, 0)`) checks
//!   only `idx < size` before scanning a bucket, so once every bit in
//!   `[0, pages_per_segment)` is set it returns the tail bucket's first zero
//!   bit, at or past `pages_per_segment`, instead of a miss. C allocates that
//!   page from the same segment, sets its bit, decrements the counter, and
//!   does not retry. `page_map` uses
//!   [`crate::bits::find_first_zero_bit`], which clamps to `size` and returns
//!   `BITS_NOT_FOUND`, so it warns and retries the next segment. The Rust
//!   behaviour never allocates a page at or past `pages_per_segment`.
//! - The native `page_read::read_page` borrows the caller's request and never
//!   frees it, because Rust owns it through [`crate::device::Request`]. C's
//!   `page_ftl_read` frees the caller's request on the zero-fill path
//!   (`ftl/page/page-read.c:85`) and the success path
//!   (`ftl/page/page-read.c:135`). `page_read::ffi_page_ftl_read` reproduces
//!   C's free.
//! - `page_read::read_page` takes `&mut PageFtl`, so the exclusive borrow
//!   replaces C's `pthread_mutex_lock(&pgftl->mutex)` around the `trans_map`
//!   read (`ftl/page/page-read.c:76-78`).
//! - `page_read::read_page` refuses a logical page at or past the L2P table
//!   with `lpn >= map_entries()`. C reads `trans_map[lpn]` with no bound at all
//!   (`ftl/page/page-read.c:71-78`), so a sector past the table reads one entry
//!   past its end. The port uses the bound `update_map` already applies.
//! - `page_read::read_page` clears the request's `is_finish` word before it
//!   submits the read, so a request reused across calls cannot return early on
//!   the previous completion's flag. C frees the request on the zero fill and
//!   success paths (`ftl/page/page-read.c:85`, `ftl/page/page-read.c:135`) and
//!   never sets the flag on its error paths, so it can never observe a stale
//!   one.
//! - When `d_op->read` returns negative, C leaks the scratch page buffer and
//!   the internal read request by nulling both locals before the cleanup label
//!   (`ftl/page/page-read.c:121-127`). The Rust read path reclaims both.
//! - The read path's scratch page buffer is a `vec!`, which aborts on out of
//!   memory like the rest of the port, so C's `-ENOMEM` from the page `malloc`
//!   (`ftl/page/page-read.c:98-102`) has no Rust equivalent. The internal
//!   request allocation keeps the `-ENOMEM` path.
//! - `page_read::ffi_page_ftl_read` returns `-EINVAL` for a `NULL` FTL or
//!   request, where C dereferences them (`ftl/page/page-read.c:71-74`).
//! - The write path's device write goes through an internal request instead of
//!   the caller's. C hands the caller's request to `d_op->write` and its
//!   `page_ftl_write_end_rq` frees both that request and the scratch page
//!   (`ftl/page/page-write.c:59-63`, `:213-218`, `:220`).
//!   `page_write::write_page` borrows the caller's request instead, so
//!   `page_write::write_end_rq` frees the scratch page and the internal
//!   request, and `page_write::ffi_page_ftl_write` frees the caller's request
//!   on the success path.
//! - The write path replaces C's three `pthread_mutex_lock(&pgftl->mutex)`
//!   regions around the free page allocation, the `is_exist` probe and the
//!   metadata update (`ftl/page/page-write.c:187-189`, `:201-203`, `:226-228`)
//!   with the `&mut PageFtl` borrow.
//! - The write path's scratch page is a `vec!`, which aborts on out of memory
//!   like the rest of the port, so C's `-ENOMEM` from the page `malloc`
//!   (`ftl/page/page-write.c:195-199`) has no Rust equivalent. The internal
//!   request allocation keeps the `-ENOMEM` path.
//! - C leaks the scratch page when the device write fails and the read request
//!   when the read for overwrite fails (`ftl/page/page-write.c:100-103`,
//!   `:220-225`), since the caller only frees its own request
//!   (`ftl/page/page-interface.c:140-144`). `page_write` reclaims both through
//!   their owning types.
//! - `page_write::write_page` keeps C's collapse of every read-for-overwrite
//!   failure to `-EFAULT` (`ftl/page/page-write.c:99-103`) and C's `-ENOMEM`
//!   for the request allocation that fails first (`ftl/page/page-write.c:91-94`).
//! - A `NULL` `request->data` is refused with `-EINVAL` before the write path's
//!   splice, where C's `memcpy` faults for a non-zero `write_size`
//!   (`ftl/page/page-write.c:211`). That also covers a zero `write_size` with a
//!   `NULL` data pointer, where C copies nothing and writes the page.
//! - C returns the device write's value verbatim
//!   (`ftl/page/page-write.c:221-225`), so a short positive write reaches the
//!   caller as a short count. `page_write::write_page` cannot carry a positive
//!   failure in `io::Result` and reports `EIO` for a non-negative mismatch,
//!   keeping the negated errno for a negative one.
//! - `page_write::ffi_page_ftl_write` returns `-EINVAL` for a `NULL` FTL or
//!   request, where C dereferences them.
//! - `page_write::write_page` refuses a logical page at or past the L2P table
//!   with `lpn >= map_entries()`, where C writes `lpn > nr_entries`
//!   (`ftl/page/page-write.c:175-179`) and lets the first slot past the table
//!   through, which then reads `trans_map` one entry past its end. The port
//!   uses the bound `update_map` already applies.
//! - `page_gc::do_gc` takes `&mut PageFtl` for the whole call, where C drops
//!   `pgftl->mutex` around the valid page copy and the erase
//!   (`ftl/page/page-gc.c:259-261`, `:283-290`). The exclusive borrow is what
//!   keeps the copy's `read_page` / `write_page` calls from interleaving with
//!   another writer, the same trade the read and write paths make.
//! - `page_core::submit_request` is C's `page_ftl_submit_request`
//!   (`ftl/page/page-core.c:329`), the dispatch the GC loop and the interface
//!   layer drive. It replaces C's `gc_mutex` hold around the erase case
//!   (`:360-369`) with the `&mut PageFtl` borrow.
//! - `page_gc::read_valid_page` returns an owned `Vec<u8>` and
//!   `page_gc::write_valid_page` a `Result`, instead of C's buffer
//!   out-parameter plus length return, so the page buffer and the request are
//!   reclaimed on every path. C leaks both when the read or write comes back
//!   short (`ftl/page/page-gc.c:148-152`, `:198-202`) and the buffer when the
//!   write path fails (`ftl/page/page-write.c:220-224`), because each site
//!   returns before its own cleanup: the read helper ahead of its `exception`
//!   label, the write helper ahead of the `free(buffer)` on the next line, and
//!   `page_ftl_write` ahead of the end callback that normally frees it
//!   (`ftl/page/page-write.c:59-63`). C also hands the request to
//!   `page_ftl_read` / `page_ftl_write`, which free it themselves
//!   (`ftl/page/page-read.c:85`, `:135`, `ftl/page/page-write.c:59-63`).
//! - The GC copy's scratch page is a `vec!`, which aborts on out of memory like
//!   the rest of the port, so C's `-ENOMEM` from the page `malloc`
//!   (`ftl/page/page-gc.c:127-132`) has no Rust equivalent. The request
//!   allocations keep the `-ENOMEM` path (`:135-140`, `:186-190`, `:88-92`).
//! - `page_gc::read_valid_page` and `page_gc::write_valid_page` keep C's
//!   collapse of every read or write failure, device errors included, to
//!   `-EFAULT` (`ftl/page/page-gc.c:148-152`, `:198-202`).
//! - C returns the erase's value verbatim and leaks the request when the
//!   backend fails without running the completion callback
//!   (`ftl/page/page-gc.c:97-101`). `page_gc::segment_erase` reclaims the
//!   request there and reports `EIO` for a non-negative failure, keeping the
//!   negated errno for a negative one.
//! - `page_core::segment_data_init` is C's `page_ftl_segment_data_init`
//!   (`ftl/page/page-core.c:110`), which the GC reset step and the
//!   checkpoint's rebuild both call. It returns `()`, because the C function
//!   returns 0 unconditionally, so the error branch C checks at
//!   `ftl/page/page-gc.c:285-288` cannot be taken.
//! - `page_gc::gc_from_list` builds the segment count with Rust's saturating
//!   float to `usize` cast, where C's cast of the same product is undefined for
//!   a negative or out of range `gc_ratio` (`ftl/page/page-gc.c:310`). The
//!   return value keeps both of C's quirks: a call that drains the list reports
//!   one fewer segment than it erased (`:317-321`), and a request the dispatch
//!   sends to the read or write path reports that path's byte count
//!   (`:312-316`).
//! - `page_gc::ffi_page_ftl_do_gc` and `page_gc::ffi_page_ftl_gc_from_list`
//!   return `-EINVAL` for a `NULL` FTL or request, where C dereferences them
//!   (`ftl/page/page-gc.c:260`, `:309-312`).
//! - `page_core::submit_request` returns `-EINVAL` for a flag C does not define
//!   without releasing C's `gc_mutex`, which C holds across the whole switch
//!   (`ftl/page/page-core.c:338`, `:370-374`), so C's next dispatch blocks
//!   forever. The port holds the borrow, not a lock.
//! - `page_core::open` reports a failed `pthread_mutex_init`,
//!   `pthread_rwlock_init` or `pthread_create` as the negated pthread code
//!   (`ftl/page/page-core.c:197-200`, `:254-263`, `:303-307`). C returns
//!   `errno` for the rwlock case, which is not the pthread result and is
//!   usually 0, so C reports success after a failure, and it returns the
//!   positive pthread code for the two mutex cases instead of a negated errno.
//! - C tests `gc_thread_status < 0` after `pthread_create`
//!   (`ftl/page/page-core.c:305`), but the call reports a positive error code,
//!   so C ignores a collector that never started and reports a successful open.
//!   The port fails the open with `-status`.
//! - `page_core::open`'s error path runs `page_core::close`, which joins
//!   `pgftl->gc_thread` (`ftl/page/page-core.c:423`) even when the open never
//!   created the thread, so C joins its zeroed field. The port joins only a
//!   non-zero thread id, and clears the field after the join so a second close
//!   cannot join twice where C joins again.
//! - The GC thread's exit request is keyed by FTL. C's is one
//!   `static int is_gc_thread_exit` for the whole process
//!   (`ftl/page/page-core.c:24`, `:302`, `:422`), so one FTL's close stops
//!   every other FTL's collector, and an `open` that runs between another FTL's
//!   close and the thread's next turn leaves that thread waiting for a request
//!   that never comes.
//! - `page_core::init_segment` saturates the `sizeof(segment) * nr_segments`
//!   product (`ftl/page/page-core.c:142-146`), which C wraps, so an absurd
//!   geometry reports `-ENOMEM` there instead of taking a short allocation the
//!   loop then writes past.
//! - The partial allocation paths reclaim what they took. C leaves the segment
//!   array and the bitmaps already handed out unreachable when a later bitmap
//!   fails, because `pgftl->segments` is only assigned after the loop
//!   (`ftl/page/page-core.c:148-183`), and it destroys and frees an
//!   uninitialized rwlock array on the error path (`:452-460`). The port
//!   installs each array in the FTL before anything is handed out and zeroes
//!   it, so `close` reclaims every allocation and sees zeroed locks.
//! - `page_core::close` reports `-ENODEV` when the device has no `d_op` or no
//!   `close` (`ftl/page/page-core.c:462-465`), where C calls through the
//!   missing entry, and `page_core::open` reports `-EINVAL` for a device
//!   without an `open` (`:267-272`), where C calls a `NULL` function pointer.
//! - The core lifecycle's C-ABI entry points refuse a `NULL` FTL, device,
//!   request or segment with `-EINVAL`, where C dereferences them and asserts
//!   on the device (`ftl/page/page-core.c:252`). `ffi_page_ftl_close` keeps C's
//!   0 for a `NULL` FTL (`:418-421`).
//! - C's GC thread loop body is a block inside the loop
//!   (`ftl/page/page-core.c:55-71`). The port splits it into
//!   `page_core::gc_thread_iteration` and `page_core::gc_pass`, so a test can
//!   drive one turn of the collector instead of racing the thread, and builds
//!   the erase request per pass where C reuses one for the thread's life
//!   (`:38-42`). `page_ftl_do_gc` never reads the request either way.
//! - The thread's pause is `std::thread::sleep`. C asserts that `nanosleep`
//!   returns 0 (`ftl/page/page-core.c:55`), so a signal that interrupts the
//!   sleep aborts the process there, while the port resumes the pause.
//! - The checkpoint is the port's own addition: C never persists the mapping
//!   table and leaves recovery as a `@todo` (`ftl/page/page-core.c:238`).
//!   `page_core::close` writes the magic and version, `alloc_segnum`, the
//!   device's segment and page counts, the L2P table and each segment's free
//!   page count to `<device name>.ckpt`, and `page_core::open` loads it back.
//!   The path cannot ride in `struct page_ftl`, whose layout is pinned, so it
//!   is keyed by FTL in a static map that `close` clears.
//! - The checkpoint's `use_bits` bits come from the segment free page counts,
//!   not from the mapping: a page whose mapping an overwrite invalidated is
//!   still programmed and still counted until the erase
//!   (`ftl/page/page-write.c:43-47`), so a mapping alone would present it as free
//!   and the next allocation would program it twice. The programmed pages are
//!   the prefix below the free count, because the allocator takes the lowest
//!   zero bit (`ftl/page/page-map.c:71`) and only an erase clears a bit
//!   (`ftl/page/page-core.c:110-124`). An entry outside the segment array, on a
//!   page that is not programmed, or on a page an earlier entry claimed is
//!   dropped, which C does not have to do because it never reads a file.
//! - The checkpoint's checksum covers the header as well as the body, and the
//!   file is written to a sibling and renamed over the target, so a corrupted
//!   `alloc_segnum` is caught rather than trusted and a save that cannot
//!   complete leaves the last good file intact. A missing, foreign, truncated,
//!   oversized, other-geometry, other-size, checksum-failing or otherwise
//!   inconsistent file leaves C's empty mapping in place, an unreadable file is
//!   a warning, and a failed write never turns a successful close into a
//!   failure. The load reads the header first and sizes the body from the
//!   device, so a foreign file cannot decide an allocation here.

use core::sync::atomic::{AtomicI32, Ordering};

use libc::{pthread_mutex_t, pthread_rwlock_t, pthread_t};

use crate::device::device;
use crate::list::list_node_t;

pub mod page_core;
pub mod page_gc;
pub mod page_map;
pub mod page_read;
pub mod page_write;

#[cfg(test)]
pub(crate) mod test_support;

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
/// `page_ftl_segment_data_init` in `ftl/page/page-core.c`, now
/// [`page_core::segment_data_init`], which frees the list and clears the
/// bitmap.
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
        unsafe { self.dev.as_ref().expect("page FTL has no backing device") }
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
