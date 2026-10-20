//! Page-mapping FTL types, ported from the deleted C `include/page.h`.
//!
//! [`PageFtl`] and [`PageFtlSegment`] are plain Rust types. C used to read them
//! by offset (`pgftl->trans_map`, `pgftl->alloc_segnum`, `pgftl->segments`,
//! `segment->use_bits`, ...), which pinned `#[repr(C)]` here and a matching
//! `PAGE_FTL_STATIC_ASSERT` block in the header. Task 16b removed both: C now
//! reaches the FTL through `page_interface::ffi_page_ftl_module_init` and the
//! `flash_operations` table, so the fields are free to change.
//!
//! The safe accessors on [`PageFtl`] (`map_size`, `lpn`, `page_offset`,
//! `free_pages`) and the address mapping in [`page_map`] are the native layer.
//! Callers use these and never touch raw pointers or `__atomic_*` builtins.
//!
//! `page_ftl_get_free_page` and `page_ftl_update_map` from `ftl/page/page-map.c`
//! live in [`page_map`]. The read, write, garbage collection and core lifecycle
//! paths live in [`page_read`], [`page_write`], [`page_gc`] and [`page_core`]
//! the same way, and [`page_interface`] is the flash operations bridge over
//! them, C's `ftl/page/page-interface.c`.
//!
//! Deliberate deviations from the C original:
//!
//! - Both rwlocks are gone. C's `pgftl->rwlock` and its
//!   `PAGE_FTL_USE_GLOBAL_RWLOCK` switch (which the Makefile set for the
//!   `USE_ZONE_DEVICE` build) made `page_ftl_submit_request` take one lock for
//!   the whole device, a writer for a write or an erase and a reader for a
//!   read, on top of the mutex rather than instead of it
//!   (`ftl/page/page-core.c:341-367`). The port's dispatch already holds one
//!   exclusive borrow of the FTL for the whole operation, so that lock had no
//!   reader concurrency left to hand out and nothing read the field. C's
//!   per-bus array goes with it: `pgftl->bus_rwlock` and
//!   `page_ftl_init_bus_lock` (`ftl/page/page-core.c:183-205`) built one lock
//!   per bus and `page_ftl_close` released them, and no caller in C ever locked
//!   one either, so a `Vec` of locks nothing takes would imply a per-bus
//!   serialization that does not exist. The `Mutex<()>` below is the port's
//!   only lock, and it is what serializes every entry point.
//! - C's `pgftl->mutex` becomes the `Mutex<()>` field [`PageFtlLock`] takes,
//!   and C's `pgftl->gc_mutex` is dropped: both become the `&mut PageFtl`
//!   borrow everywhere inside the native layer, so an entry point has to hold
//!   the real mutex for that borrow to exclude a second caller. Every entry
//!   point two C threads can reach takes [`PageFtlLock`] across the whole
//!   operation: the three `page_interface` flash hooks and the `ffi_` entry
//!   points of `page_core::submit_request`, `page_read::read_page`,
//!   `page_write::write_page`, `page_gc::do_gc`, `page_gc::gc_from_list`, plus
//!   the GC thread's iteration. C's regions are narrower, three small ones per
//!   write (`ftl/page/page-write.c:187-189`, `:201-203`, `:226-228`), so a
//!   Rust entry serializes the whole operation instead. `page_core::open` and
//!   `page_core::close` hold nothing: the `Mutex` is built with the structure
//!   and needs no init or destroy call, where C ran `pthread_mutex_init` in
//!   `open` and `pthread_mutex_destroy` in `close`. `gc_mutex` guarded the
//!   dispatch switch (`ftl/page/page-core.c:338`), which the borrow of
//!   `page_core::submit_request` covers, so a second lock would guard nothing.
//! - The lock policy is to recover a poisoned guard. Every lock site goes
//!   through `lock_recover`, which logs and takes the guarded state back,
//!   because C has no poisoning concept: a thread that died mid operation left
//!   the mutex free and the next caller carried on. The alternative would turn
//!   one panic into a panic in every later caller. What can poison a lock here
//!   is the collector, which is a Rust thread, so `page_core::close` logs the
//!   panic its join reports and discards the checkpoint, and the next `open`
//!   starts from an empty mapping rather than from state a panicked pass may
//!   have torn.
//! - `nr_free_pages` and `nr_valid_pages` are `AtomicI32` fields, because the
//!   GC thread and the callers' threads both reach them. C touched them through
//!   `__atomic_*` builtins. `is_gc` existed only to hold `use_bits` at C's
//!   offset 16 and nothing read it, so it is gone.
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
//! - `page_core::open` has no lock failure to report any more. C's two
//!   `pthread_mutex_init` calls, the bus array's `malloc` and its
//!   `pthread_rwlock_init` could fail (`ftl/page/page-core.c:189-200`,
//!   `:254-263`), and C reports those with a mixture of negated errno and
//!   positive pthread code, plus `errno` for the array's rwlock case, which is
//!   not the pthread result and is usually 0, so C reports success after a
//!   failure. `Mutex::new` cannot fail and the array is gone. What is left is
//!   the collector's thread creation: the port spawns it with
//!   `std::thread::Builder::spawn` and reports a failure as the negated
//!   `io::Error` errno, C's `-status` from `pthread_create` (`:303-307`).
//! - C tests `gc_thread_status < 0` after `pthread_create`
//!   (`ftl/page/page-core.c:305`), but the call reports a positive error code,
//!   so C ignores a collector that never started and reports a successful open.
//!   The port fails the open on a spawn error.
//! - The collector is a `std::thread` and `pgftl->gc_thread` is its
//!   `Option<JoinHandle<()>>`, so `open` spawns it and `close` joins it where C
//!   called `pthread_create` and `pthread_join` on a `pthread_t`. The handle
//!   reaches the thread through a `Send` wrapper over the FTL pointer, because
//!   a Rust thread's closure must be `Send` and C's `void *` argument had no
//!   such bound. A collector that panics poisons the FTL's lock and the join
//!   reports it, and the port then treats the mapping as untrustworthy: a pass
//!   that died mid way can leave a half updated `use_bits` bitmap, free page
//!   count or allocation cursor, and the checkpoint's own counters are what the
//!   next `open` would believe. `close` logs the panic, removes the checkpoint
//!   instead of writing one, and still closes the device. C has no thread
//!   result and no recovery step at all, so an empty mapping is the closest
//!   match: the next `open` finds no file and starts unmapped
//!   (`ftl/page/page-core.c:238`).
//! - `page_core::open`'s error path runs `page_core::close`, which joins
//!   `pgftl->gc_thread` (`ftl/page/page-core.c:423`) even when the open never
//!   created the thread, so C joins its zeroed field. The port joins only a
//!   handle it holds, and takes it out of the field so a second close cannot
//!   join twice where C joins again.
//! - The GC thread's exit request is keyed by FTL. C's is one
//!   `static int is_gc_thread_exit` for the whole process
//!   (`ftl/page/page-core.c:24`, `:302`, `:422`), so one FTL's close stops
//!   every other FTL's collector, and an `open` that runs between another FTL's
//!   close and the thread's next turn leaves that thread waiting for a request
//!   that never comes.
//! - `page_core::init_segment` builds the segment array and the per-segment
//!   `use_bits` bitmaps as owning `Vec`s, so C's wrap of the
//!   `sizeof(segment) * nr_segments` product (`ftl/page/page-core.c:142-146`)
//!   is gone: `Vec::try_reserve_exact` reports the overloaded count as
//!   `-ENOMEM` instead of taking a short allocation the loop then writes past.
//!   Every other buffer C allocated with `malloc` keeps the same trade, so a
//!   refused reservation is the `-ENOMEM` C returned
//!   (`ftl/page/page-core.c:85-100`, `:213-221`, `:291-296`).
//! - The partial allocation paths reclaim what they took. C leaves the segment
//!   array and the bitmaps already handed out unreachable when a later bitmap
//!   fails, because `pgftl->segments` is only assigned after the loop
//!   (`ftl/page/page-core.c:148-183`), and its `close` destroys and frees the
//!   bus rwlock array on the error path (`:452-460`) even when the open failed
//!   before that array was initialized. The segment array, its bitmaps and the
//!   L2P table are owned here, so a failed `open` drops them with the local the
//!   loop was filling, and there is no lock array left to release.
//! - `page_core::close` reports `-ENODEV` when the device has no `d_op` or no
//!   `close` (`ftl/page/page-core.c:462-465`), where C calls through the
//!   missing entry, and `page_core::open` reports `-EINVAL` for a device
//!   without an `open` (`:267-272`), where C calls a `NULL` function pointer.
//! - The core lifecycle refuses a `NULL` FTL, device, request or segment with
//!   `-EINVAL`, where C dereferences them and asserts on the device
//!   (`ftl/page/page-core.c:252`).
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
//!   C had no field for the name, because its layout was pinned, so the port
//!   first keyed the path by FTL in a static map. The path now rides in
//!   [`PageFtl::checkpoint`], which `open` sets and `close` takes.
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
//! - The `ioctl` slot of [`crate::flash::flash_operations`] is narrowed to the
//!   one argument shape the two callers in this repository use
//!   (`integration-test.rs:322`, `example/rw_example.rs:97`), because C
//!   declares the operation variadic (`include/flash.h:59-60`) and stable Rust
//!   cannot define a variadic function. The field stays one pointer, so C
//!   tables and the pinned offsets are unaffected, and
//!   `page_interface::page_ftl_ioctl_interface` implements C's
//!   `page_ftl_ioctl_interface` (`ftl/page/page-interface.c:291-327`) natively.
//! - `page_interface::ffi_page_ftl_module_init` returns `-EINVAL` for a `NULL`
//!   flash pointer, where C dereferences it
//!   (`ftl/page/page-interface.c:355`). Its `Box` allocation aborts on out of
//!   memory like the rest of the port, so C's `malloc` failure returning
//!   `-ENOMEM` (`:357-362`) has no Rust equivalent, the same trade
//!   `ffi_flash_module_init` records.
//! - C leaks the `struct page_ftl` allocation when `device_module_init` fails,
//!   because it runs `page_ftl_module_exit` while `f_private` is still `NULL`
//!   (`ftl/page/page-interface.c:357-377`). The port's `Box` releases the
//!   structure on that path.
//! - `page_interface::ffi_page_ftl_module_exit` clears `f_private` after it
//!   frees the FTL, so a second exit is a logged no-op where C frees the same
//!   pointer twice (`ftl/page/page-interface.c:406-407`).
//! - The page sized staging buffer C allocates in `page_ftl_read_interface`
//!   (`ftl/page/page-interface.c:200-205`) and copies each chunk out of
//!   (`:241`) is gone: [`page_read::read_page`] stages the device read in its
//!   own scratch page and copies the requested subrange into the request's
//!   buffer. C's `-ENOMEM` from that `malloc` therefore has no Rust
//!   equivalent, while the per chunk request allocation keeps its `-ENOMEM`
//!   (`ftl/page/page-interface.c:222-227`), and the read interface reports
//!   `-EINVAL` for every other failure exactly as C does (`:236-239`).
//! - A `NULL` write buffer reaches an interface call that C lets through to the
//!   `memcpy` in `page_ftl_write` (`ftl/page/page-write.c:211`), which faults
//!   there. The port's write path refuses it with `-EINVAL` before the splice,
//!   and the write interface reports its accumulated count, which is `0` for
//!   the first chunk.
//! - C leaks the ioctl request on its unknown command branch
//!   (`ftl/page/page-interface.c:320-322`), since it returns before the
//!   `device_free_request`. `page_interface::page_ftl_ioctl_interface` owns the
//!   request through [`crate::device::Request`], which reclaims it there.
//! - `page_interface::ffi_flash_write` and `ffi_flash_read` have no C
//!   counterpart: they drive `flash->f_op` for Rust callers, which C callers
//!   reach themselves (`integration-test.c:95`, `benchmark.c:646`). A `NULL`
//!   handle, a handle with no table and a table with no operation report
//!   `-EINVAL`, where C dereferences the first two and calls the third.
//! - `tests/integration.rs` drives C's `integration-test.c` scenario over the
//!   same ABI with deterministic wait jitter: C reseeds `srand()` from
//!   `time(NULL)` inside every loop (`integration-test.c:55`, `:91`, `:129`)
//!   and draws `rand() % 500` for the same three bases (`:74`, `:107`, `:145`),
//!   where the port gives each worker one fixed xorshift stream. It also bounds
//!   the `is_check` index against the range, where C stores whatever the device
//!   returned (`integration-test.c:71`), and fails on a missing block, where C
//!   only prints the offsets and returns 0 (`:240-252`). It also bounds each
//!   read retry with a thirty second per offset deadline, where C's reader
//!   spins on the current offset forever (`integration-test.c:60-65`), so a
//!   lost block fails the suite instead of hanging the CI job.

use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::device::device;
use crate::pr_warn;

pub mod page_core;
pub mod page_gc;
pub mod page_interface;
pub mod page_map;
pub mod page_read;
pub mod page_write;

#[cfg(test)]
pub(crate) mod test_support;

/// The page FTL's GC tuning values, generated from `ftl.yaml` by
/// `tools/gen_config.py`. `make config` regenerates them and
/// `make check-config` fails when they are stale.
pub use crate::ftl_config::{
    PAGE_FTL_GC_ALL, PAGE_FTL_GC_RATIO, PAGE_FTL_GC_THRESHOLD,
};

/// C's `enum { PAGE_FTL_IOCTL_TRIM = 0 }`, the ioctl C still issues.
pub const PAGE_FTL_IOCTL_TRIM: core::ffi::c_int = 0;

/// One segment (block) of the FTL.
///
/// One entry per segment. `use_bits` is a bitmap of `pages_per_segment` bits
/// and `lpn_list` the logical pages whose current mapping lands in the segment,
/// oldest first. Both are owned `Vec`s here, where C allocated them with
/// `malloc` and reached them through `segment->use_bits` / `segment->lpn_list`
/// (see `page_ftl_segment_data_init` in `ftl/page/page-core.c`, now
/// [`page_core::segment_data_init`]). The last element of `lpn_list` is C's
/// list head, the page written most recently, which is the end the GC walk
/// reads first.
///
/// The two counters are `AtomicI32` because the GC thread and the callers'
/// threads both reach them, which C did through `__atomic_load_n`,
/// `__atomic_fetch_add`, `__atomic_fetch_sub` and `__atomic_store_n` (see
/// `page-map.c`, `page-write.c`, `page-gc.c`, `page-core.c`).
#[derive(Debug)]
pub struct PageFtlSegment {
    pub nr_free_pages: AtomicI32,
    pub nr_valid_pages: AtomicI32,
    pub use_bits: Vec<u64>,
    pub lpn_list: Vec<usize>,
}

impl PageFtlSegment {
    /// A segment with `use_bits` as its bitmap, `free_pages` free pages and no
    /// valid page: the state C's `page_ftl_segment_data_init` leaves a segment
    /// in.
    pub fn empty(use_bits: Vec<u64>, free_pages: i32) -> Self {
        PageFtlSegment {
            nr_free_pages: AtomicI32::new(free_pages),
            nr_valid_pages: AtomicI32::new(0),
            use_bits,
            lpn_list: Vec::new(),
        }
    }

    /// Set the bit for `page` in this segment's `use_bits` bitmap.
    pub fn set_use_bit(&mut self, page: u64) {
        crate::bits::set_bit(&mut self.use_bits, page);
    }

    /// Return whether `page` is marked used in this segment.
    pub fn use_bit(&self, page: u64) -> bool {
        crate::bits::get_bit(&self.use_bits, page)
    }

    /// Atomically read `nr_free_pages`, mirroring C's `__atomic_load_n(...,
    /// __ATOMIC_SEQ_CST)`.
    pub fn free_pages(&self) -> i32 {
        self.nr_free_pages.load(Ordering::SeqCst)
    }
}

/// The page FTL: the mapping table, the segment array and the lifecycle state.
///
/// `trans_map` is the page-level L2P table, `segments` the segment array and
/// `alloc_segnum` the round-robin cursor. `gc_list` names the segments queued
/// for collection, most recently queued first, so its first element is C's list
/// head, and `gc_seg_bits` is the bitmap that says which of them are already
/// queued. All four are owned here, where C allocated them with `malloc` and
/// freed them in `page_ftl_close` ([`page_core::close`]).
///
/// The synchronization is Rust's: `mutex` is the lock [`PageFtlLock`] takes and
/// `gc_thread` is the collector's [`JoinHandle`]. See the deviations in
/// [`crate::ftl`] for what happened to C's `gc_mutex` and to its two rwlocks.
///
/// `checkpoint` is the port's own: C never persists the mapping, so it had no
/// field for a path, and its layout was pinned anyway, so the path would have
/// lived in a table keyed by the FTL address.
pub struct PageFtl {
    pub trans_map: Vec<u32>,
    pub alloc_segnum: u64,
    pub segments: Vec<PageFtlSegment>,
    pub dev: *mut device,
    /// C's `pgftl->mutex`, the lock every entry point takes through
    /// [`PageFtlLock`].
    pub mutex: Mutex<()>,
    /// C's `pgftl->gc_thread`, as the handle [`page_core::close`] joins.
    /// `None` outside an open and close pair: `page_core::open` spawns the
    /// collector and `page_core::close` takes the handle back out.
    pub gc_thread: Option<JoinHandle<()>>,
    pub o_flags: core::ffi::c_int,
    pub gc_list: Vec<usize>,
    pub gc_seg_bits: Vec<u64>,
    /// The checkpoint file `close` writes the mapping to, or `None` when `open`
    /// was handed no name. It is a boxed slice and not a `String` because the
    /// path needs no spare capacity, which keeps the field two words smaller.
    /// [`PageFtl::empty`] sets it to `None` explicitly, so nothing here depends
    /// on the layout of a zeroed `Option`.
    pub checkpoint: Option<Box<str>>,
}

impl PageFtl {
    /// An FTL with no buffers, no device and no lock held: C's `memset(0)` of
    /// `struct page_ftl`, with the empty buffers and the lock Rust needs where C
    /// had `NULL` pointers and an uninitialized `pthread_mutex_t`.
    /// [`page_core::open`] and the test fixtures build from here.
    pub fn empty() -> PageFtl {
        PageFtl {
            trans_map: Vec::new(),
            alloc_segnum: 0,
            segments: Vec::new(),
            dev: ptr::null_mut(),
            mutex: Mutex::new(()),
            gc_thread: None,
            o_flags: 0,
            gc_list: Vec::new(),
            gc_seg_bits: Vec::new(),
            checkpoint: None,
        }
    }

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

    /// C's `page_ftl_get_free_pages`: sum of every segment's free-page count.
    pub fn free_pages(&self) -> usize {
        self.segments.iter().map(|seg| seg.free_pages() as usize).sum()
    }
}

/// Take `mutex`, recovering a guard a panicking thread poisoned.
///
/// This is the crate's lock policy, and every lock site goes through it: the
/// FTL's own lock in [`PageFtlLock`] and the collector's stop set in
/// `page_core`. C has no poisoning concept, so a thread that died mid operation
/// left the mutex free and the next caller carried on. The port keeps that
/// behaviour rather than turning one panic into a panic in every later caller:
/// a poisoned guard is logged and recovered, and the mapping a panicked pass
/// may have torn is dropped with its checkpoint by `page_core::close`, so the
/// next `open` starts from an empty mapping instead of from state nothing can
/// vouch for.
///
/// A panic inside a guarded operation cannot reach the caller anyway when it
/// unwinds through an `extern "C"` entry point, which aborts. What this covers
/// is the collector: it is a Rust thread like any other, so a panic there
/// poisons the lock of a process that keeps running. `page_core::close` logs
/// that thread's result on the join and discards its checkpoint.
fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            pr_warn!("a page FTL lock was poisoned by a panicking thread");
            poisoned.into_inner()
        }
    }
}

/// C's `pthread_mutex_lock(&pgftl->mutex)` / `unlock`, as an RAII guard.
///
/// The native layer models `pgftl->mutex` as an exclusive `&mut PageFtl`
/// borrow, so that borrow only excludes another thread if whoever takes it also
/// holds the mutex. Every entry point two C threads can reach at the same time
/// takes one of these before it borrows the FTL. Without it, two callers can
/// borrow the same FTL at once and hand out the same free page, which is what
/// C's lock in `page_ftl_get_free_page`'s callers prevents
/// (`ftl/page/page-write.c:187-189`).
///
/// [`page_core::open`] and [`page_core::close`] are the exception: they
/// establish and end the lifecycle the lock belongs to, so they hold nothing.
/// Neither can the operations [`page_core::open`] runs before the collector
/// exists (`page_core::segment_data_init`, `page_core::checkpoint_load`), which
/// is why they take their borrow directly.
///
/// The guard is what releases the mutex, so it is held for the lock's whole
/// life even though nothing reads it.
pub struct PageFtlLock<'a> {
    _guard: MutexGuard<'a, ()>,
    pgftl: &'a mut PageFtl,
}

impl<'a> PageFtlLock<'a> {
    /// Take the FTL's mutex and borrow the FTL behind it.
    ///
    /// # Safety
    /// `pgftl` is a live, opened FTL that no other thread holds the mutex of,
    /// and it stays alive and unmoved until the lock is dropped.
    pub unsafe fn acquire(pgftl: *mut PageFtl) -> PageFtlLock<'a> {
        PageFtlLock {
            _guard: lock_recover(&(*pgftl).mutex),
            pgftl: &mut *pgftl,
        }
    }

    /// The FTL behind the lock.
    pub fn get(&mut self) -> &mut PageFtl {
        self.pgftl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DEVICE_PAGE_SIZE;
    use core::mem::MaybeUninit;

    #[test]
    fn geometry_helpers_match_c() {
        let mut dev = unsafe { MaybeUninit::<device>::zeroed().assume_init() };
        dev.info.nr_bus = 2;
        dev.info.nr_chips = 2;
        dev.info.package.nr_blocks = 4;
        dev.info.package.block.nr_pages = 128;
        dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

        let mut pgftl = PageFtl::empty();
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
        let mut dev = unsafe { MaybeUninit::<device>::zeroed().assume_init() };
        dev.info.package.nr_blocks = 1;

        let mut pgftl = PageFtl::empty();
        pgftl.dev = &mut dev;
        pgftl.segments = vec![PageFtlSegment::empty(vec![0u64; 1], 3)];

        assert_eq!(pgftl.free_pages(), 3);
        pgftl.segments[0].set_use_bit(5);
        assert!(pgftl.segments[0].use_bit(5));
        assert!(!pgftl.segments[0].use_bit(6));
    }
}
