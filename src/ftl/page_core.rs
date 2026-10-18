//! Page FTL core lifecycle, checkpoint and request dispatch, ported from
//! `ftl/page/page-core.c`.
//!
//! [`open`] mirrors C's `page_ftl_open`: initialize the two mutexes, open the
//! device, allocate the bus rwlock array, the L2P table, the segment array with
//! its `use_bits` bitmaps and the `gc_seg_bits` bitmap, load the checkpoint and
//! start the GC thread. [`close`] mirrors C's `page_ftl_close`: save the
//! checkpoint, join the GC thread, destroy the two mutexes, release every
//! allocation and close the device. [`submit_request`] mirrors C's
//! `page_ftl_submit_request`, the dispatch the interface layer and the GC loop
//! drive.
//!
//! The statics mirror the file's statics: [`alloc_bitmap`], [`init_segment`],
//! [`init_bus_lock`], [`init_map`], [`free_segments`], [`gc_thread_main`] with
//! its [`gc_thread_iteration`] turn and [`gc_pass`] body, and the checkpoint
//! pair [`checkpoint_save`] / [`checkpoint_load`]. C's
//! `page_ftl_segment_data_init` lives here as [`segment_data_init`], its C home
//! (`ftl/page/page-core.c:110`), and [`crate::ftl::page_gc`] calls it.
//!
//! Deliberate deviations from C are listed in the [`crate::ftl`] module docs.

use core::ffi::{c_char, c_int, c_void};
use core::mem::size_of;
use core::ptr;
use core::slice;
use core::sync::atomic::Ordering;
use core::time::Duration;
use std::collections::HashSet;
use std::ffi::CStr;
use std::io;
use std::io::Read;
use std::io::Write;
use std::sync::Mutex;

use libc::pthread_rwlock_t;

use crate::bits::set_bit;
use crate::crc32::crc32;
use crate::device::{device_address, device_request, DeviceDirection, PADDR_EMPTY};
use crate::ftl::page_gc::{do_gc, gc_from_list};
use crate::ftl::page_read::read_page;
use crate::ftl::page_write::write_page;
use crate::ftl::{PageFtl, PageFtlLock, PageFtlSegment, PAGE_FTL_GC_RATIO, PAGE_FTL_GC_THRESHOLD};
use crate::list::{ffi_list_free, ffi_list_prepend};
use crate::{pr_debug, pr_err, pr_warn};

/// C's `CRC32_INIT` (`include/crc32.h:9`), the seed of the checkpoint's
/// checksum.
const CRC32_INIT: u32 = 0xFFFF_FFFF;

/// C's `req.tv_sec = 1` (`ftl/page/page-core.c:35`), the pause between two GC
/// thread turns.
const GC_THREAD_INTERVAL: Duration = Duration::from_secs(1);

/// The FTLs whose collector has been asked to stop, keyed by the FTL's
/// address.
///
/// C keeps one `static int is_gc_thread_exit` for the whole process
/// (`ftl/page/page-core.c:24`), so `open`'s clear (`:302`) and `close`'s set
/// (`:422`) reach every FTL in it, and one FTL's close stops another's
/// collector. The request is keyed by FTL here instead. `close` removes the key
/// after the join, so the map holds one entry per FTL being shut down.
static GC_THREAD_STOP: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// The GC thread stop set.
fn gc_thread_stop() -> std::sync::MutexGuard<'static, Option<HashSet<usize>>> {
    GC_THREAD_STOP
        .lock()
        .expect("the GC thread stop set is never poisoned")
}

/// Ask the collector of the FTL at `key` to stop, C's
/// `__atomic_store_n(&is_gc_thread_exit, 1)`.
fn request_gc_stop(key: usize) {
    gc_thread_stop()
        .get_or_insert_with(HashSet::new)
        .insert(key);
}

/// Forget the stop request of the FTL at `key`, C's
/// `__atomic_store_n(&is_gc_thread_exit, 0)`.
fn clear_gc_stop(key: usize) {
    if let Some(stops) = gc_thread_stop().as_mut() {
        stops.remove(&key);
    }
}

/// Whether the collector of the FTL at `key` has been asked to stop, C's
/// `__atomic_load_n(&is_gc_thread_exit)`.
fn gc_stop_requested(key: usize) -> bool {
    gc_thread_stop()
        .as_ref()
        .is_some_and(|stops| stops.contains(&key))
}

/// Checkpoint file magic, `PFTC` read as a little endian word.
const CHECKPOINT_MAGIC: u32 = 0x5046_5443;

/// Checkpoint layout version, so a later format can reject an older file
/// instead of misreading it.
const CHECKPOINT_VERSION: u32 = 1;

/// Offset of the CRC32 field inside the header.
const CHECKPOINT_CRC_OFFSET: usize = 4 + 4 + 8 + 8 + 8 + 8;

/// Bytes before the body: magic, version, `alloc_segnum`, the segment count,
/// the pages per segment, the entry count and the CRC32 of the whole file
/// except that field.
///
/// The geometry is part of the file because the entry count alone does not
/// identify the device it came from: a `1 x 8` and a `2 x 4` device share an
/// entry count, and applying one device's entries to the other would place them
/// in the wrong segment.
const CHECKPOINT_HEADER_BYTES: usize = CHECKPOINT_CRC_OFFSET + 4;

/// The checkpoint file of the device called `name`, or `None` when the caller
/// passed no name and the FTL therefore persists nothing.
fn checkpoint_path(name: *const c_char) -> Option<Box<str>> {
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_str().ok()?;
    if name.is_empty() {
        return None;
    }
    Some(format!("{name}.ckpt").into_boxed_str())
}

/// C's `BITS_TO_UINT64_ALIGN` (`include/bits.h:17`): the bytes one bitmap of
/// `bits` bits takes, one `u64` bucket per 64 bits.
fn bits_to_bytes(bits: usize) -> usize {
    (bits / 64 + 1) * size_of::<u64>()
}

/// C's `malloc` plus the `memset(0)` every bitmap and lock array gets. A null
/// return is C's allocation failure.
fn alloc_zeroed(bytes: usize) -> *mut c_void {
    unsafe { libc::calloc(1, bytes) }
}

/// C's `page_ftl_alloc_bitmap` (`ftl/page/page-core.c:85`).
///
/// Returns the zeroed bitmap, or a null pointer where C reports `-ENOMEM`
/// through its out parameter.
fn alloc_bitmap(pgftl: &PageFtl) -> *mut u64 {
    let pages_per_segment = pgftl.dev().pages_per_segment();
    alloc_zeroed(bits_to_bytes(pages_per_segment)) as *mut u64
}

/// C's `page_ftl_segment_data_init` (`ftl/page/page-core.c:110`): reset a
/// segment to empty.
///
/// The free page count goes back to the segment's page count, the valid count
/// and the `use_bits` bitmap are cleared, and the LPN list is freed.
///
/// # Safety
/// `segment` points into a live segment array and its `use_bits` holds
/// `pages_per_segment` bits.
pub unsafe fn segment_data_init(pgftl: &PageFtl, segment: *mut PageFtlSegment) {
    let pages_per_segment = pgftl.dev().pages_per_segment();

    unsafe {
        let segment = &mut *segment;
        segment
            .nr_free_pages
            .store(pages_per_segment as i32, Ordering::SeqCst);
        segment.nr_valid_pages.store(0, Ordering::SeqCst);

        let words = pages_per_segment / 64 + 1;
        slice::from_raw_parts_mut(segment.use_bits, words).fill(0);

        if !segment.lpn_list.is_null() {
            ffi_list_free(segment.lpn_list);
            segment.lpn_list = ptr::null_mut();
        }
    }
}

/// C's `page_ftl_init_segment` (`ftl/page/page-core.c:134`): allocate the
/// segment array and one zeroed bitmap per segment.
///
/// The array goes into the FTL before the bitmaps are handed out, where C keeps
/// it in a local until the loop ends, so the `-ENOMEM` path here reclaims the
/// array and the bitmaps allocated so far instead of leaking them.
fn init_segment(pgftl: &mut PageFtl) -> c_int {
    let nr_segments = pgftl.dev().nr_segments();

    let bytes = size_of::<PageFtlSegment>().saturating_mul(nr_segments);
    let segments = alloc_zeroed(bytes) as *mut PageFtlSegment;
    if segments.is_null() {
        pr_err!("memory allocation failed");
        return -libc::ENOMEM;
    }
    pgftl.segments = segments;

    for segnum in 0..nr_segments {
        let bits = alloc_bitmap(pgftl);
        if bits.is_null() {
            pr_err!("initialize the use bitmap failed (segnum: {})", segnum);
            return -libc::ENOMEM;
        }
        let segment = unsafe { segments.add(segnum) };
        unsafe { (*segment).use_bits = bits };
        unsafe { segment_data_init(pgftl, segment) };
        pr_debug!(
            "initialize the segment {} (bits: {}, size: {})",
            segnum,
            pgftl.dev().pages_per_segment(),
            pgftl.dev().pages_per_segment() / 8
        );
    }

    0
}

/// C's `page_ftl_init_bus_lock` (`ftl/page/page-core.c:183`): one rwlock per
/// bus.
///
/// The array lands in the FTL before the locks are initialized, so close
/// reclaims it after a partial failure. It is zeroed, which makes the locks the
/// error path destroys a valid initial state instead of C's uninitialized
/// memory.
fn init_bus_lock(pgftl: &mut PageFtl) -> c_int {
    let nr_bus = pgftl.dev().info.nr_bus;

    let bus_rwlock = alloc_zeroed(size_of::<pthread_rwlock_t>().saturating_mul(nr_bus))
        as *mut pthread_rwlock_t;
    if bus_rwlock.is_null() {
        pr_err!("rwlock allocation failed");
        return -libc::ENOMEM;
    }
    pgftl.bus_rwlock = bus_rwlock;

    for bus in 0..nr_bus {
        let err = unsafe { libc::pthread_rwlock_init(bus_rwlock.add(bus), ptr::null()) };
        if err != 0 {
            pr_err!("rwlock initialize failed");
            return -err;
        }
    }

    0
}

/// C's `page_ftl_init_map` (`ftl/page/page-core.c:213`): allocate the L2P table
/// and point every entry at [`PADDR_EMPTY`].
fn init_map(pgftl: &mut PageFtl) -> c_int {
    let map_size = pgftl.map_size();

    let trans_map = unsafe { libc::malloc(map_size) } as *mut u32;
    if trans_map.is_null() {
        pr_err!("cannot allocate the memory for mapping table");
        return -libc::ENOMEM;
    }
    pgftl.trans_map = trans_map;

    for lpn in 0..map_size / size_of::<u32>() {
        unsafe { *trans_map.add(lpn) = PADDR_EMPTY };
    }

    0
}

/// C's `page_ftl_submit_request` (`ftl/page/page-core.c:329`): dispatch one
/// request to the read, write or collection path.
///
/// Returns C's `ssize_t`: 0 for an erase, the byte count the read or write path
/// reports, or a negated errno. C's `gc_mutex`, which the erase case holds
/// across `page_ftl_do_gc`, is the `&mut PageFtl` borrow here.
///
/// # Errors
/// `EINVAL` for a flag C does not define, otherwise the dispatched path's
/// error.
pub fn submit_request(pgftl: &mut PageFtl, request: &mut device_request) -> io::Result<isize> {
    match request.direction() {
        Some(DeviceDirection::Write) => write_page(pgftl, request).map(|len| len as isize),
        Some(DeviceDirection::Read) => read_page(pgftl, request).map(|len| len as isize),
        Some(DeviceDirection::Erase) => do_gc(pgftl).map(|()| 0),
        None => {
            pr_err!("invalid flag detected: {}", request.flag);
            Err(io::Error::from_raw_os_error(libc::EINVAL))
        }
    }
}

/// C's GC thread loop body
/// (`ftl/page/page-core.c:57-71`): skip the pass while more than
/// [`PAGE_FTL_GC_THRESHOLD`] of the device's pages are free, otherwise collect
/// up to [`PAGE_FTL_GC_RATIO`] of the segments.
///
/// C builds one erase request before its loop and reuses it. `page_ftl_do_gc`
/// never reads the request, so this builds one per pass from the same zeroed
/// `struct device_request`.
fn gc_pass(pgftl: &mut PageFtl) -> io::Result<isize> {
    let total_pages = pgftl.dev().total_pages();
    let free_pages = unsafe { pgftl.free_pages() };
    if free_pages as f64 > total_pages as f64 * PAGE_FTL_GC_THRESHOLD {
        return Ok(0);
    }

    let mut request = unsafe { core::mem::MaybeUninit::<device_request>::zeroed().assume_init() };
    request.flag = DeviceDirection::Erase.into();
    gc_from_list(pgftl, &mut request, PAGE_FTL_GC_RATIO)
}

/// One turn of C's GC thread loop after the sleep
/// (`ftl/page/page-core.c:55-70`): stop on the caller's exit request, otherwise
/// run one pass and stop when it fails.
///
/// Returns whether the loop continues. `exit_requested` is C's
/// `__atomic_load_n(&is_gc_thread_exit)` and arrives as a parameter so a test
/// can drive both outcomes without racing the global the real thread shares.
fn gc_thread_iteration(pgftl: &mut PageFtl, exit_requested: bool) -> bool {
    if exit_requested {
        return false;
    }
    match gc_pass(pgftl) {
        Ok(_) => true,
        Err(err) => {
            let errno = err.raw_os_error().unwrap_or(libc::EIO);
            pr_err!(
                "critical garbage collection error detected (errno: {})",
                -errno
            );
            false
        }
    }
}

/// C's `page_ftl_gc_thread` (`ftl/page/page-core.c:33`): sleep, then run one
/// [`gc_thread_iteration`] until the exit flag is raised or a pass fails.
///
/// `data` is the [`PageFtl`] that created the thread, which stays alive until
/// the join in [`close`], the contract C's `pthread_create` call site relies
/// on. The body dereferences it, so the function is only reachable through
/// `pthread_create`.
extern "C" fn gc_thread_main(data: *mut c_void) -> *mut c_void {
    let pgftl = data as *mut PageFtl;
    assert!(!pgftl.is_null(), "the garbage collector has no page FTL");
    assert!(
        !unsafe { (*pgftl).dev }.is_null(),
        "the garbage collector has no device"
    );

    loop {
        std::thread::sleep(GC_THREAD_INTERVAL);
        let exit_requested = gc_stop_requested(pgftl as usize);
        // C's GC thread reaches the FTL through `page_ftl_gc_from_list`, which
        // takes `pgftl->mutex` itself (`ftl/page/page-gc.c:259`, `:283`). The
        // native path below owns the borrow instead, so the mutex is held here.
        let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
        if !gc_thread_iteration(lock.get(), exit_requested) {
            break;
        }
    }

    ptr::null_mut()
}

/// Save `pgftl`'s L2P table and the segment state to `path`.
///
/// Layout, all little endian: magic, version, `alloc_segnum`, the device's
/// segment count and pages per segment, the entry count, the CRC32 of
/// everything before that field and of the body, then one `uint32_t` per L2P
/// slot followed by one `uint32_t` free page count per segment. The geometry is
/// part of the file so a checkpoint from a device with the same page count but
/// another split is refused rather than applied to the wrong segments.
fn checkpoint_save(pgftl: &PageFtl, path: &str) -> io::Result<()> {
    let entries = pgftl.map_entries();
    let nr_segments = pgftl.dev().nr_segments();
    let pages_per_segment = pgftl.dev().pages_per_segment();
    let body_bytes = entries * size_of::<u32>() + nr_segments * size_of::<u32>();
    let mut buffer = Vec::with_capacity(CHECKPOINT_HEADER_BYTES + body_bytes);

    buffer.extend_from_slice(&CHECKPOINT_MAGIC.to_le_bytes());
    buffer.extend_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    buffer.extend_from_slice(&pgftl.alloc_segnum.to_le_bytes());
    buffer.extend_from_slice(&(nr_segments as u64).to_le_bytes());
    buffer.extend_from_slice(&(pages_per_segment as u64).to_le_bytes());
    buffer.extend_from_slice(&(entries as u64).to_le_bytes());
    buffer.extend_from_slice(&0u32.to_le_bytes());
    for lpn in 0..entries {
        buffer.extend_from_slice(&unsafe { *pgftl.trans_map.add(lpn) }.to_le_bytes());
    }
    for segnum in 0..nr_segments {
        // The programmed pages of a segment are the pages below its free page
        // count, so the count is enough to rebuild `use_bits`. It is clamped
        // into that range, because the load refuses a file outside it and a
        // counter the FTL itself broke must not make every mapping in the file
        // unreadable.
        let free = unsafe { &*pgftl.segments.add(segnum) }
            .free_pages()
            .clamp(0, pages_per_segment as i32);
        buffer.extend_from_slice(&(free as u32).to_le_bytes());
    }

    let crc = checkpoint_crc(
        &buffer[..CHECKPOINT_CRC_OFFSET],
        &buffer[CHECKPOINT_HEADER_BYTES..],
    );
    buffer[CHECKPOINT_CRC_OFFSET..CHECKPOINT_HEADER_BYTES].copy_from_slice(&crc.to_le_bytes());

    // The replacement is atomic: the new file is complete in a sibling and only
    // then renamed over the target, so a crash or a full disk mid write cannot
    // destroy the last good copy. The temporary is removed when the write
    // fails, and left to the next save when the rename does.
    let temporary = format!("{path}.tmp");
    let result = (|| -> io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&buffer)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// The checksum of a checkpoint: `before_crc` is the header up to the checksum
/// field and `body` everything after it.
///
/// `crc32`'s incremental form takes the previous result with its final xor
/// undone (`src/crc32.rs`), which is how the two parts are chained.
fn checkpoint_crc(before_crc: &[u8], body: &[u8]) -> u32 {
    let crc = crc32(
        before_crc.as_ptr() as *const c_void,
        before_crc.len(),
        CRC32_INIT,
    );
    crc32(body.as_ptr() as *const c_void, body.len(), crc ^ 0xFFFF_FFFF)
}

/// Load the checkpoint at `path` into `pgftl`.
///
/// Reports `false` for a missing file, a foreign magic or version, a truncated
/// file, a checkpoint of another geometry or of another entry count, a bad
/// checksum, an allocation cursor outside the segment array, a segment whose
/// free page count is outside the segment, an oversized file, and any
/// individual entry the mapping cannot honour. Every one of them leaves the
/// freshly initialized empty mapping in place, which is C's start state: C never
/// fails `page_ftl_open` for a storage problem, because it has no recovery step
/// at all (`ftl/page/page-core.c:238`).
///
/// The checksum covers the header as well as the body, so a corrupted
/// `alloc_segnum` is caught rather than trusted. The body is read only after
/// the header's geometry and entry count have been checked against the device,
/// so a foreign or oversized file is refused before its length decides an
/// allocation here.
///
/// # Errors
/// The read error when the file exists but cannot be read.
fn checkpoint_load(pgftl: &mut PageFtl, path: &str) -> io::Result<bool> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    let mut header = [0u8; CHECKPOINT_HEADER_BYTES];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
            pr_warn!("checkpoint {} is shorter than its header", path);
            return Ok(false);
        }
        Err(err) => return Err(err),
    }
    let word = |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let long = |offset: usize| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());

    if word(0) != CHECKPOINT_MAGIC {
        pr_warn!("checkpoint {} has a foreign magic", path);
        return Ok(false);
    }
    if word(4) != CHECKPOINT_VERSION {
        pr_warn!(
            "checkpoint {} has version {} (expected {})",
            path,
            word(4),
            CHECKPOINT_VERSION
        );
        return Ok(false);
    }
    let alloc_segnum = long(8);
    let nr_segments = long(16);
    let pages_per_segment = long(24);
    let nr_entries = long(32);
    let stored_crc = word(CHECKPOINT_CRC_OFFSET);

    let entries = pgftl.map_entries();
    let segments = pgftl.dev().nr_segments();
    let pages = pgftl.dev().pages_per_segment();
    if nr_segments != segments as u64 || pages_per_segment != pages as u64 {
        pr_warn!(
            "checkpoint {} holds a {} x {} device (expected {} x {})",
            path,
            nr_segments,
            pages_per_segment,
            segments,
            pages
        );
        return Ok(false);
    }
    if nr_entries != entries as u64 {
        pr_warn!(
            "checkpoint {} holds {} entries (expected {})",
            path,
            nr_entries,
            entries
        );
        return Ok(false);
    }

    // Both sizes come from the device, never from the file, so the body read
    // below cannot be enlarged by the file it is checking.
    let entries_bytes = entries * size_of::<u32>();
    let frees_bytes = segments * size_of::<u32>();
    let expected = CHECKPOINT_HEADER_BYTES + entries_bytes + frees_bytes;
    if file.metadata()?.len() != expected as u64 {
        pr_warn!("checkpoint {} is not {} bytes long", path, expected);
        return Ok(false);
    }
    let mut body = vec![0u8; entries_bytes + frees_bytes];
    if let Err(err) = file.read_exact(&mut body) {
        pr_warn!("checkpoint {} cannot be read ({})", path, err);
        return Ok(false);
    }

    if checkpoint_crc(&header[..CHECKPOINT_CRC_OFFSET], &body) != stored_crc {
        pr_warn!("checkpoint {} checksum mismatch", path);
        return Ok(false);
    }
    if alloc_segnum >= nr_segments {
        pr_warn!(
            "checkpoint {} allocates segment {} of {}",
            path,
            alloc_segnum,
            nr_segments
        );
        return Ok(false);
    }
    let mut free_pages = Vec::with_capacity(segments);
    for segnum in 0..segments {
        let offset = entries_bytes + segnum * size_of::<u32>();
        let free = u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
        if free > pages as u32 {
            pr_warn!(
                "checkpoint {} frees {} pages of segment {} (max {})",
                path,
                free,
                segnum,
                pages
            );
            return Ok(false);
        }
        free_pages.push(free);
    }

    for lpn in 0..entries {
        let offset = lpn * size_of::<u32>();
        let entry = u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
        unsafe { *pgftl.trans_map.add(lpn) = entry };
    }
    pgftl.alloc_segnum = alloc_segnum;
    restore_segments(pgftl, &free_pages);

    Ok(true)
}

/// Install a loaded L2P table and the per-segment free page counts that came
/// with it.
///
/// `use_bits` is rebuilt from the free counts, not from the mapping: a page
/// whose mapping an overwrite invalidated is still programmed and its bit is
/// still set in the running FTL until the segment is erased
/// (`ftl/page/page-write.c:43-47`), so a mapping alone would present it as free
/// and the next allocation would program it a second time. The programmed pages
/// are the prefix below `pages_per_segment - free`, because the allocator hands
/// out the lowest zero bit and only an erase clears a bit.
///
/// The mapping walk then fills the GC `lpn_list`s and the valid page counters.
/// An entry is dropped, the way C drops nothing at all because it has no
/// recovery, when it points outside the segment array, when its page is not
/// programmed, or when another entry already claimed the same page. The last
/// one needs the set below: two entries on one page would count the page twice
/// in `nr_valid_pages` and then send the collector after a segment that does
/// not hold the pages it claims.
fn restore_segments(pgftl: &mut PageFtl, free_pages: &[u32]) {
    let pages_per_segment = pgftl.dev().pages_per_segment();
    let nr_segments = pgftl.dev().nr_segments();
    let words = pages_per_segment / 64 + 1;
    let mut claimed = HashSet::new();

    for (segnum, free) in free_pages.iter().enumerate() {
        let segment = unsafe { pgftl.segments.add(segnum) };
        unsafe { segment_data_init(pgftl, segment) };
        let programmed = pages_per_segment as u32 - free;
        unsafe {
            (*segment).nr_free_pages.store(*free as i32, Ordering::SeqCst);
            let bits = slice::from_raw_parts_mut((*segment).use_bits, words);
            for page in 0..programmed as u64 {
                set_bit(bits, page);
            }
        }
    }

    for lpn in 0..pgftl.map_entries() {
        let paddr = unsafe { *pgftl.trans_map.add(lpn) };
        if paddr == PADDR_EMPTY {
            continue;
        }
        let address = device_address::from(paddr);
        let segnum = address.block() as usize;
        let programmed = if segnum < nr_segments {
            pages_per_segment as u32 - free_pages[segnum]
        } else {
            0
        };
        let page = address.segment_page();
        if segnum >= nr_segments || page >= programmed {
            pr_warn!("checkpoint entry {} points outside a programmed page", lpn);
            unsafe { *pgftl.trans_map.add(lpn) = PADDR_EMPTY };
            continue;
        }
        if !claimed.insert(paddr) {
            pr_warn!("checkpoint entry {} repeats page {}", lpn, paddr);
            unsafe { *pgftl.trans_map.add(lpn) = PADDR_EMPTY };
            continue;
        }

        let segment = unsafe { &mut *pgftl.segments.add(segnum) };
        segment.nr_valid_pages.fetch_add(1, Ordering::SeqCst);
        segment.lpn_list = unsafe { ffi_list_prepend(segment.lpn_list, lpn as *mut c_void) };
    }
}

/// Apply the checkpoint `name` names, if it has one.
///
/// A failure is C's start state plus a warning, because a checkpoint the port
/// cannot read must not turn a valid device into a failed open.
fn load_checkpoint(pgftl: &mut PageFtl, name: *const c_char) {
    let Some(path) = checkpoint_path(name) else {
        return;
    };
    match checkpoint_load(pgftl, &path) {
        Ok(true) => pr_debug!("checkpoint {} loaded", path),
        Ok(false) => pr_debug!("no checkpoint at {}", path),
        Err(err) => pr_warn!(
            "checkpoint {} cannot be read ({}), starting from an empty mapping",
            path,
            err
        ),
    }
}

/// Write the checkpoint `open` recorded in `pgftl`, if it has one.
///
/// Nothing is written for an FTL that has no mapping table, which is the case
/// when `open` failed before the table was allocated and the error path calls
/// [`close`]. A failed write is logged and ignored: C's `page_ftl_close`
/// reports the device's result, and a directory that cannot be written must not
/// turn a successful close into a failure.
fn save_checkpoint(pgftl: &mut PageFtl) {
    if pgftl.trans_map.is_null() {
        return;
    }
    let Some(path) = pgftl.checkpoint.take() else {
        return;
    };
    match checkpoint_save(pgftl, &path) {
        Ok(()) => pr_debug!("checkpoint {} saved", path),
        Err(err) => pr_warn!("checkpoint {} cannot be written ({})", path, err),
    }
}

/// C's `page_ftl_open` (`ftl/page/page-core.c:240`): bring an FTL up on the
/// device its `dev` points at.
///
/// Returns 0 on success and a negated errno on failure. A failure releases
/// everything the call allocated by running [`close`], C's `exception` label.
///
/// This is the only open entry point: C builds the FTL through
/// `page_interface::ffi_page_ftl_module_init`, which sets `dev`, and reaches
/// [`open`] through the `flash_operations` table.
///
/// # Safety
/// `pgftl.dev` points at a live device or is `NULL`, and `name` is `NULL` or a
/// NUL terminated string.
pub unsafe fn open(pgftl: &mut PageFtl, name: *const c_char, flags: c_int) -> c_int {
    if pgftl.dev.is_null() {
        pr_err!("page FTL has no device");
        return -libc::EINVAL;
    }

    if flags & libc::O_CREAT == 0 {
        pr_warn!("current version needs to O_CREAT flag");
    }

    let mut err = unsafe { libc::pthread_mutex_init(&mut pgftl.mutex, ptr::null()) };
    if err != 0 {
        pr_err!("mutex initialize failed");
        return open_failed(pgftl, -err);
    }
    err = unsafe { libc::pthread_mutex_init(&mut pgftl.gc_mutex, ptr::null()) };
    if err != 0 {
        pr_err!("gc_mutex initialize failed");
        return open_failed(pgftl, -err);
    }

    let dev = pgftl.dev;
    let Some(device_open) = (unsafe { (*dev).d_op.as_ref() }).and_then(|ops| ops.open) else {
        pr_err!("device open failed");
        return open_failed(pgftl, -libc::EINVAL);
    };
    if unsafe { device_open(dev, name, flags) } != 0 {
        pr_err!("device open failed");
        return open_failed(pgftl, -libc::EINVAL);
    }

    let mut err = init_bus_lock(pgftl);
    if err != 0 {
        return open_failed(pgftl, err);
    }
    err = init_map(pgftl);
    if err != 0 {
        return open_failed(pgftl, err);
    }
    err = init_segment(pgftl);
    if err != 0 {
        return open_failed(pgftl, err);
    }
    pgftl.gc_list = ptr::null_mut();

    let nr_segments = pgftl.dev().nr_segments();
    let gc_seg_bits = alloc_zeroed(bits_to_bytes(nr_segments)) as *mut u64;
    if gc_seg_bits.is_null() {
        pr_err!("memory allocation failed");
        return open_failed(pgftl, -libc::ENOMEM);
    }
    pgftl.gc_seg_bits = gc_seg_bits;

    pgftl.o_flags = flags;
    load_checkpoint(pgftl, name);

    clear_gc_stop(pgftl as *mut PageFtl as usize);
    let status = unsafe {
        libc::pthread_create(
            &mut pgftl.gc_thread,
            ptr::null(),
            gc_thread_main,
            pgftl as *mut PageFtl as *mut c_void,
        )
    };
    if status != 0 {
        pr_err!("garbage collection thread creation failed");
        return open_failed(pgftl, -status);
    }

    pgftl.checkpoint = checkpoint_path(name);

    0
}

/// C's `open` exception path: release what the call allocated and report
/// `err`.
fn open_failed(pgftl: &mut PageFtl, err: c_int) -> c_int {
    let _ = unsafe { close(pgftl) };
    err
}

/// C's `page_ftl_free_segments` (`ftl/page/page-core.c:382`): release every
/// segment's bitmap and LPN list.
fn free_segments(pgftl: &PageFtl) {
    let segments = pgftl.segments;
    let nr_segments = pgftl.dev().nr_segments();

    for segnum in 0..nr_segments {
        let segment = unsafe { &mut *segments.add(segnum) };
        if !segment.use_bits.is_null() {
            unsafe { libc::free(segment.use_bits as *mut c_void) };
            segment.use_bits = ptr::null_mut();
        }
        if !segment.lpn_list.is_null() {
            unsafe { ffi_list_free(segment.lpn_list) };
            segment.lpn_list = ptr::null_mut();
        }
    }
}

/// C's `page_ftl_close` (`ftl/page/page-core.c:414`): save the checkpoint,
/// stop the GC thread, release every allocation and close the device.
///
/// Returns the device's result, and 0 when there is no device to close.
///
/// # Safety
/// `pgftl` came from [`open`] or is a zeroed structure, and no other caller
/// uses it while this runs.
pub unsafe fn close(pgftl: &mut PageFtl) -> c_int {
    let key = pgftl as *mut PageFtl as usize;
    request_gc_stop(key);

    // C joins whatever `gc_thread` holds, including the zeroed field `open`
    // left behind when it failed before creating the thread. The port joins
    // only a thread it created, and clears the field so a second close cannot
    // join twice.
    if pgftl.gc_thread != 0 {
        unsafe { libc::pthread_join(pgftl.gc_thread, ptr::null_mut()) };
        pgftl.gc_thread = 0;
    }
    clear_gc_stop(key);

    save_checkpoint(pgftl);

    unsafe {
        libc::pthread_mutex_destroy(&mut pgftl.mutex);
        libc::pthread_mutex_destroy(&mut pgftl.gc_mutex);
    }

    if !pgftl.segments.is_null() {
        free_segments(pgftl);
        unsafe { libc::free(pgftl.segments as *mut c_void) };
        pgftl.segments = ptr::null_mut();
    }
    if !pgftl.trans_map.is_null() {
        unsafe { libc::free(pgftl.trans_map as *mut c_void) };
        pgftl.trans_map = ptr::null_mut();
    }
    if !pgftl.gc_list.is_null() {
        unsafe { ffi_list_free(pgftl.gc_list) };
        pgftl.gc_list = ptr::null_mut();
    }
    if !pgftl.gc_seg_bits.is_null() {
        unsafe { libc::free(pgftl.gc_seg_bits as *mut c_void) };
        pgftl.gc_seg_bits = ptr::null_mut();
    }

    if !pgftl.dev.is_null() && !pgftl.bus_rwlock.is_null() {
        let nr_bus = unsafe { (*pgftl.dev).info.nr_bus };
        for bus in 0..nr_bus {
            unsafe { libc::pthread_rwlock_destroy(pgftl.bus_rwlock.add(bus)) };
        }
        unsafe { libc::free(pgftl.bus_rwlock as *mut c_void) };
        pgftl.bus_rwlock = ptr::null_mut();
    }

    if !pgftl.dev.is_null() && !unsafe { (*pgftl.dev).d_op }.is_null() {
        let dev = pgftl.dev;
        return match unsafe { (*(*dev).d_op).close } {
            Some(device_close) => unsafe { device_close(dev) },
            None => -libc::ENODEV,
        };
    }
    0
}

/// C-ABI entry point for [`submit_request`], mirroring C's
/// `page_ftl_submit_request`.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL and `request` is `NULL` or a live
/// request.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_submit_request(
    pgftl: *mut PageFtl,
    request: *mut device_request,
) -> isize {
    if pgftl.is_null() || request.is_null() {
        pr_err!("null detected");
        return -(libc::EINVAL as isize);
    }
    let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
    match submit_request(lock.get(), &mut *request) {
        Ok(ret) => ret,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as isize),
    }
}

/// C-ABI entry point for [`segment_data_init`], mirroring C's
/// `page_ftl_segment_data_init`.
///
/// # Safety
/// `pgftl` is `NULL` or a live page FTL and `segment` is `NULL` or points into
/// its segment array.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_segment_data_init(
    pgftl: *mut PageFtl,
    segment: *mut PageFtlSegment,
) -> c_int {
    if pgftl.is_null() || segment.is_null() {
        pr_err!("page FTL or segment is null detected");
        return -libc::EINVAL;
    }
    unsafe { segment_data_init(&*pgftl, segment) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Request, DEVICE_PAGE_SIZE};
    use crate::ftl::test_support::{failing_open, CoreFixture};
    use crate::list::ffi_list_length;
    use core::sync::atomic::AtomicUsize;

    /// Two segments of four pages: small enough that a write and an overwrite
    /// reach the collection queue.
    const NR_SEGMENTS: usize = 2;
    const PAGES_PER_SEGMENT: usize = 4;
    /// Payload length every dispatch test uses.
    const LEN: usize = 16;
    /// The flag pair C's callers pass, `page-interface.c:192`.
    const FLAGS: c_int = libc::O_CREAT | libc::O_RDWR;

    /// A unique device name in the temporary directory. The ramdisk ignores the
    /// name, so only the checkpoint file lands there.
    fn checkpoint_name(tag: &str) -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            "{}/ftl-page-core-{}-{}-{}",
            std::env::temp_dir().display(),
            std::process::id(),
            tag,
            NEXT.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// A request the dispatch can drive. `data` must outlive the call.
    fn request(flag: DeviceDirection, sector: usize, data: &mut [u8]) -> Request {
        let mut request = Request::new().expect("allocate a request");
        request.flag = flag.into();
        request.sector = sector;
        request.data_len = data.len();
        request.data = data.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        request
    }

    /// The bytes of a checkpoint holding `mapping` and the per-segment
    /// `free_pages`, checksummed the way [`checkpoint_save`] does.
    ///
    /// `nr_segments` and `pages_per_segment` are the file's claim about the
    /// device it came from, so a test can pass a geometry the device it is
    /// loaded into does not have.
    fn checkpoint_image(
        nr_segments: u64,
        pages_per_segment: u64,
        alloc_segnum: u64,
        mapping: &[u32],
        free_pages: &[u32],
    ) -> Vec<u8> {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&CHECKPOINT_MAGIC.to_le_bytes());
        buffer.extend_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
        buffer.extend_from_slice(&alloc_segnum.to_le_bytes());
        buffer.extend_from_slice(&nr_segments.to_le_bytes());
        buffer.extend_from_slice(&pages_per_segment.to_le_bytes());
        buffer.extend_from_slice(&(mapping.len() as u64).to_le_bytes());
        buffer.extend_from_slice(&0u32.to_le_bytes());
        for entry in mapping {
            buffer.extend_from_slice(&entry.to_le_bytes());
        }
        for free in free_pages {
            buffer.extend_from_slice(&free.to_le_bytes());
        }
        let crc = checkpoint_crc(
            &buffer[..CHECKPOINT_CRC_OFFSET],
            &buffer[CHECKPOINT_HEADER_BYTES..],
        );
        buffer[CHECKPOINT_CRC_OFFSET..CHECKPOINT_HEADER_BYTES].copy_from_slice(&crc.to_le_bytes());
        buffer
    }

    /// A mapping of `NR_SEGMENTS * PAGES_PER_SEGMENT + 1` [`PADDR_EMPTY`]
    /// entries, the shape a checkpoint for [`opened_fixture`]'s geometry has.
    fn empty_mapping() -> Vec<u32> {
        vec![PADDR_EMPTY; NR_SEGMENTS * PAGES_PER_SEGMENT + 1]
    }

    /// An opened FTL of [`NR_SEGMENTS`] segments with [`PAGES_PER_SEGMENT`]
    /// pages each.
    fn opened_fixture() -> CoreFixture {
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, ptr::null(), FLAGS) },
            0
        );
        f
    }

    /// Whether `segment`'s first `use_bits` word is clear.
    fn use_bits_clear(segment: *const PageFtlSegment) -> bool {
        unsafe { *(*segment).use_bits == 0 }
    }

    /// Put `lpn` on `segment`'s LPN list, the state the write path leaves a
    /// segment in once one of its pages is invalidated.
    ///
    /// # Safety
    /// `segment` points into a live segment array.
    unsafe fn queue_lpn(segment: *mut PageFtlSegment, lpn: usize) {
        unsafe { (*segment).lpn_list = ffi_list_prepend(ptr::null_mut(), lpn as *mut c_void) };
    }

    #[test]
    fn open_close_lifecycle_releases_every_resource() {
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let dev = f.dev;
        let key = f.pgftl.as_ref() as *const PageFtl as usize;

        assert_eq!(
            unsafe { open(&mut f.pgftl, ptr::null(), FLAGS) },
            0
        );

        // The device is open: its backend filled the store, the bad segment
        // bitmap and the geometry the FTL then read.
        assert!(!unsafe { (*dev).badseg_bitmap }.is_null());
        assert_eq!(unsafe { (*dev).info.package.nr_blocks }, NR_SEGMENTS);
        assert_eq!(
            unsafe { (*dev).info.package.block.nr_pages },
            PAGES_PER_SEGMENT
        );

        let pgftl = f.pgftl.as_mut();
        assert!(!pgftl.trans_map.is_null());
        assert_eq!(pgftl.map_entries(), NR_SEGMENTS * PAGES_PER_SEGMENT + 1);
        assert_eq!(unsafe { *pgftl.trans_map }, PADDR_EMPTY);
        assert_eq!(
            unsafe { *pgftl.trans_map.add(pgftl.map_entries() - 1) },
            PADDR_EMPTY
        );
        assert!(!pgftl.segments.is_null());
        assert!(!pgftl.bus_rwlock.is_null());
        assert!(!pgftl.gc_seg_bits.is_null());
        assert!(pgftl.gc_list.is_null());
        assert_eq!(pgftl.alloc_segnum, 0);
        assert_eq!(pgftl.o_flags, FLAGS);
        assert_ne!(pgftl.gc_thread, 0, "the GC thread is running");
        assert!(!gc_stop_requested(key));

        // Both mutexes are initialized and unlocked.
        assert_eq!(
            unsafe { libc::pthread_mutex_trylock(&mut pgftl.mutex as *mut libc::pthread_mutex_t) },
            0
        );
        unsafe { libc::pthread_mutex_unlock(&mut pgftl.mutex as *mut libc::pthread_mutex_t) };

        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        // Everything the open allocated is gone again and the thread is joined.
        let pgftl = f.pgftl.as_mut();
        assert!(pgftl.trans_map.is_null());
        assert!(pgftl.segments.is_null());
        assert!(pgftl.bus_rwlock.is_null());
        assert!(pgftl.gc_seg_bits.is_null());
        assert!(pgftl.gc_list.is_null());
        assert_eq!(pgftl.gc_thread, 0);

        // The device is closed too: the ramdisk released its bitmap.
        assert!(unsafe { (*dev).badseg_bitmap }.is_null());

        // A second lifecycle on the same structure, this time without
        // `O_CREAT`, which C warns about and accepts. The open reinitializes
        // everything the close released.
        assert_eq!(
            unsafe { open(&mut f.pgftl, ptr::null(), libc::O_RDWR) },
            0
        );
        let pgftl = f.pgftl.as_mut();
        assert_eq!(pgftl.o_flags, libc::O_RDWR);
        assert!(!pgftl.trans_map.is_null());
        assert!(!pgftl.segments.is_null());
        assert_ne!(pgftl.gc_thread, 0);
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        assert!(f.pgftl.trans_map.is_null());
    }

    #[test]
    fn gc_stop_requests_are_per_ftl() {
        // C's one global flag means one FTL's close stops every collector in
        // the process (`ftl/page/page-core.c:24`, `:422`). The port keys the
        // request by FTL, which is what lets two FTLs be shut down
        // independently, and the close of one releases its own key.
        let mut first = opened_fixture();
        let mut second = opened_fixture();
        let first_key = first.pgftl.as_ref() as *const PageFtl as usize;
        let second_key = second.pgftl.as_ref() as *const PageFtl as usize;
        assert_ne!(first_key, second_key);

        assert!(!gc_stop_requested(first_key));
        assert!(!gc_stop_requested(second_key));

        request_gc_stop(first_key);
        assert!(gc_stop_requested(first_key));
        assert!(!gc_stop_requested(second_key), "the request is keyed");

        // Both lifecycle ends join their own thread, and the close releases
        // the key it set.
        assert_eq!(unsafe { close(&mut first.pgftl) }, 0);
        assert!(!gc_stop_requested(first_key));
        assert_eq!(first.pgftl.gc_thread, 0);
        assert_eq!(unsafe { close(&mut second.pgftl) }, 0);
        assert!(!gc_stop_requested(second_key));
        assert_eq!(second.pgftl.gc_thread, 0);
    }

    #[test]
    fn segment_and_bitmap_allocation_matches_c() {
        let mut f = CoreFixture::new(NR_SEGMENTS + 1, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, ptr::null(), FLAGS) },
            0
        );

        let pgftl = f.pgftl.as_mut();
        let mut bitmaps = Vec::new();
        for segnum in 0..=NR_SEGMENTS {
            let segment = unsafe { &*pgftl.segments.add(segnum) };
            assert!(!segment.use_bits.is_null());
            assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32);
            assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 0);
            assert!(segment.lpn_list.is_null());
            assert!(use_bits_clear(segment), "the bitmap starts zeroed");
            bitmaps.push(segment.use_bits as usize);
        }
        bitmaps.sort_unstable();
        bitmaps.dedup();
        assert_eq!(bitmaps.len(), NR_SEGMENTS + 1, "one bitmap per segment");

        // The C ABI entry point resets a segment the way the erase path does.
        let segment = pgftl.segments;
        unsafe {
            (*segment).set_use_bit(3);
            (*segment).nr_free_pages.store(0, Ordering::SeqCst);
            (*segment).nr_valid_pages.store(PAGES_PER_SEGMENT as i32, Ordering::SeqCst);
            queue_lpn(segment, 1);

            assert_eq!(ffi_page_ftl_segment_data_init(pgftl, segment), 0);

            assert_eq!((*segment).free_pages(), PAGES_PER_SEGMENT as i32);
            assert_eq!((*segment).nr_valid_pages.load(Ordering::SeqCst), 0);
            assert!(use_bits_clear(segment));
            assert!((*segment).lpn_list.is_null());
        }
    }

    #[test]
    fn submit_request_dispatches_write_read_and_erase() {
        let mut f = opened_fixture();
        let pgftl = f.pgftl.as_mut();

        // A write reaches the write path, which maps the logical page and hands
        // it a page of the first segment.
        let mut payload = vec![0x5au8; LEN];
        let mut write_rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(pgftl, &mut write_rq).unwrap(), LEN as isize);
        assert_eq!(
            unsafe { *pgftl.trans_map },
            device_address::from_segment_page(0, 0).lpn
        );
        let segment = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 1);

        // A read of the same logical page comes back through the read path.
        let mut buf = vec![0u8; LEN];
        let mut read_rq = request(DeviceDirection::Read, 0, &mut buf);
        assert_eq!(submit_request(pgftl, &mut read_rq).unwrap(), LEN as isize);
        assert_eq!(buf, payload);

        // An erase with nothing queued collects nothing and reports 0.
        let mut erase_rq = request(DeviceDirection::Erase, 0, &mut []);
        assert_eq!(submit_request(pgftl, &mut erase_rq).unwrap(), 0);
        assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 1);

        // An undefined flag is C's `-EINVAL` (`page-core.c:372-374`), and it
        // crosses the ABI the same way.
        let mut bad_rq = request(DeviceDirection::Write, 0, &mut payload);
        bad_rq.flag = 3;
        assert_eq!(
            submit_request(pgftl, &mut bad_rq)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        unsafe {
            assert_eq!(
                ffi_page_ftl_submit_request(pgftl, bad_rq.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        // Both a write and a read cross the C ABI with the byte counts C's
        // callers compare against (`ftl/page/page-interface.c:113-124`).
        let mut abi_payload = vec![0x5bu8; LEN];
        let abi_rq = request(DeviceDirection::Write, DEVICE_PAGE_SIZE, &mut abi_payload);
        assert_eq!(
            unsafe { ffi_page_ftl_submit_request(pgftl, abi_rq.as_ptr()) },
            LEN as isize
        );
        let mut abi_buf = vec![0u8; LEN];
        let abi_rq = request(DeviceDirection::Read, DEVICE_PAGE_SIZE, &mut abi_buf);
        assert_eq!(
            unsafe { ffi_page_ftl_submit_request(pgftl, abi_rq.as_ptr()) },
            LEN as isize
        );
        assert_eq!(abi_buf, abi_payload);

        // The write landed on a fresh page of the same segment.
        assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32 - 2);
        assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn erase_dispatch_collects_a_queued_segment() {
        let mut f = opened_fixture();
        let pgftl = f.pgftl.as_mut();

        // Fill the first segment, then overwrite its first page: the new page
        // lands in the second segment and the invalidation queues the first one.
        for lpn in 0..PAGES_PER_SEGMENT {
            let mut payload = vec![0x40 + lpn as u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let mut payload = vec![0xa0u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);

        assert_eq!(unsafe { ffi_list_length(pgftl.gc_list) }, 1);

        // The erase dispatch routes to `page_ftl_do_gc`: the three remaining
        // pages of the victim move, then the segment is erased and reset.
        let mut erase_rq = request(DeviceDirection::Erase, 0, &mut []);
        assert_eq!(submit_request(pgftl, &mut erase_rq).unwrap(), 0);

        let victim = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(victim.free_pages(), PAGES_PER_SEGMENT as i32);
        assert_eq!(victim.nr_valid_pages.load(Ordering::SeqCst), 0);
        assert!(use_bits_clear(victim));
        assert!(victim.lpn_list.is_null());
        assert!(pgftl.gc_list.is_null());
        assert_eq!(unsafe { *pgftl.gc_seg_bits }, 0, "the gc bit is cleared");

        // Every logical page still resolves to its payload.
        let other = unsafe { &*pgftl.segments.add(1) };
        assert_eq!(other.free_pages(), 0);
        assert_eq!(other.nr_valid_pages.load(Ordering::SeqCst), 4);
        for lpn in 0..PAGES_PER_SEGMENT {
            let mut buf = vec![0u8; LEN];
            let mut rq = request(
                DeviceDirection::Read,
                lpn * DEVICE_PAGE_SIZE,
                &mut buf,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
            if lpn == 0 {
                assert_eq!(buf, vec![0xa0u8; LEN], "the overwritten page");
            } else {
                assert_eq!(buf, vec![0x40 + lpn as u8; LEN], "the relocated page");
            }
        }
    }

    #[test]
    fn gc_thread_iteration_runs_one_pass_or_stops() {
        // Twelve segments of four pages: the ratio bound C passes is
        // `nr_segments * 0.1`, so a device needs at least ten segments before a
        // pass collects anything at all.
        const SEGMENTS: usize = 12;
        const FILL: usize = SEGMENTS * PAGES_PER_SEGMENT - 8;

        let mut f = CoreFixture::new(SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, ptr::null(), FLAGS) },
            0
        );

        // The exit request stops the loop before the pass, so an FTL with a
        // fully free device never reaches the collection path.
        let pgftl = f.pgftl.as_mut();
        assert!(!gc_thread_iteration(pgftl, true));
        assert_eq!(unsafe { pgftl.free_pages() }, SEGMENTS * PAGES_PER_SEGMENT);

        // Above the threshold the pass is skipped
        // (`page-core.c:57-61`): every page is free.
        assert_eq!(gc_pass(pgftl).unwrap(), 0);
        assert!(pgftl.gc_list.is_null());

        // Fill all but eight pages, then overwrite one page of the first
        // segment: the new page lands in the free tail, the invalidation queues
        // the now full first segment, and seven free pages of forty eight are
        // under the threshold, so the pass runs.
        for lpn in 0..FILL {
            let mut payload = vec![0x11u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let mut payload = vec![0x22u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);

        assert_eq!(unsafe { pgftl.free_pages() }, 7);
        assert_eq!(unsafe { ffi_list_length(pgftl.gc_list) }, 1);

        assert!(gc_thread_iteration(pgftl, false));

        // The victim was drained, relocated into the free tail and reset.
        let victim = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(victim.free_pages(), PAGES_PER_SEGMENT as i32);
        assert_eq!(victim.nr_valid_pages.load(Ordering::SeqCst), 0);
        assert!(victim.lpn_list.is_null());
        assert!(pgftl.gc_list.is_null());
        assert_eq!(unsafe { *pgftl.gc_seg_bits }, 0, "the gc bit is cleared");
        // The erased segment's four pages are back, and the three relocated
        // pages took three of the free tail: 7 + 4 - 3.
        assert_eq!(unsafe { pgftl.free_pages() }, 8);

        // A failing pass stops the loop for the thread. The victim is queued by
        // hand with an LPN the mapping still holds, and both counters are
        // pinned under the threshold so the pass reaches the copy.
        let segment = unsafe { pgftl.segments.add(0) };
        unsafe {
            queue_lpn(segment, 1);
            (*segment).nr_valid_pages.store(1, Ordering::SeqCst);
        }
        pgftl.gc_list = unsafe { ffi_list_prepend(pgftl.gc_list, segment as *mut c_void) };
        set_bit(unsafe { slice::from_raw_parts_mut(pgftl.gc_seg_bits, 1) }, 0);
        for segnum in 0..SEGMENTS {
            let segment = unsafe { &*pgftl.segments.add(segnum) };
            segment.nr_free_pages.store(0, Ordering::SeqCst);
        }
        assert!(!unsafe {
            f.with_failing_read(|f| gc_thread_iteration(f.pgftl.as_mut(), false))
        });

        // The pass stopped where it failed: the victim was unlinked and its
        // free count pinned, its gc bit is still raised for the next pass, and
        // nothing was relocated.
        assert!(f.pgftl.gc_list.is_null());
        assert_eq!(
            unsafe { *f.pgftl.gc_seg_bits & 1 },
            1,
            "the gc bit stays raised"
        );
        assert_eq!(
            unsafe { &*f.pgftl.segments.add(0) }
                .nr_free_pages
                .load(Ordering::SeqCst),
            0,
            "the victim's free pages stay pinned"
        );
    }

    #[test]
    fn close_releases_a_queued_gc_list_and_the_segment_lists() {
        // A segment queued for collection when the FTL is closed: C frees the
        // list (`ftl/page/page-core.c:442-445`) and every segment's own LPN list
        // (`:431-435`, `:386-395`).
        let mut f = opened_fixture();
        let pgftl = f.pgftl.as_mut();
        for lpn in 0..PAGES_PER_SEGMENT {
            let mut payload = vec![0x40 + lpn as u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let mut payload = vec![0xa0u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);

        assert_eq!(unsafe { ffi_list_length(pgftl.gc_list) }, 1);
        assert!(!unsafe { &*pgftl.segments.add(1) }.lpn_list.is_null());

        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        assert!(f.pgftl.gc_list.is_null());
        assert!(f.pgftl.segments.is_null());
    }

    #[test]
    fn null_pointers_are_rejected_on_every_abi_entry() {
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let request = request(DeviceDirection::Write, 0, &mut []);
        let mut bare = PageFtl::zeroed();

        unsafe {
            assert_eq!(
                open(&mut bare, ptr::null(), FLAGS),
                -libc::EINVAL,
                "a page FTL without a device"
            );
            assert_eq!(ffi_page_ftl_submit_request(ptr::null_mut(), request.as_ptr()), -libc::EINVAL as isize);
            assert_eq!(
                ffi_page_ftl_submit_request(f.pgftl.as_mut(), ptr::null_mut()),
                -(libc::EINVAL as isize)
            );
            assert_eq!(
                ffi_page_ftl_segment_data_init(ptr::null_mut(), ptr::null_mut()),
                -libc::EINVAL
            );
            assert_eq!(
                ffi_page_ftl_segment_data_init(f.pgftl.as_mut(), ptr::null_mut()),
                -libc::EINVAL
            );

            // Closing an FTL that was never opened has nothing to release, no
            // thread to join and reports the device's result.
            assert_eq!(close(&mut f.pgftl), 0);

            // The same for an FTL without a device: there is no device to
            // close and nothing to release.
            assert_eq!(close(&mut bare), 0);
        }

        // The guards ran before anything was touched.
        assert!(f.pgftl.trans_map.is_null());
        assert!(f.pgftl.segments.is_null());
        assert!(f.pgftl.bus_rwlock.is_null());
        assert!(f.pgftl.gc_seg_bits.is_null());
        assert_eq!(f.pgftl.gc_thread, 0);
    }

    #[test]
    fn failed_device_open_maps_to_einval_and_leaves_nothing_allocated() {
        // The device's own open fails.
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let real = unsafe { &*(*f.dev).d_op };
        let failing = crate::device::device_operations {
            open: Some(failing_open),
            write: real.write,
            read: real.read,
            erase: real.erase,
            close: real.close,
        };
        let ret = unsafe {
            f.with_ops(&failing, |f| {
                open(&mut f.pgftl, ptr::null(), FLAGS)
            })
        };
        assert_eq!(ret, -libc::EINVAL, "C maps every device failure to EINVAL");
        assert!(f.pgftl.trans_map.is_null());
        assert!(f.pgftl.segments.is_null());
        assert!(f.pgftl.bus_rwlock.is_null());
        assert!(f.pgftl.gc_seg_bits.is_null());
        assert_eq!(f.pgftl.gc_thread, 0);

        // A device with no operations at all.
        let empty = crate::device::device_operations {
            open: None,
            write: None,
            read: None,
            erase: None,
            close: None,
        };
        let ret = unsafe {
            f.with_ops(&empty, |f| {
                open(&mut f.pgftl, ptr::null(), FLAGS)
            })
        };
        assert_eq!(ret, -libc::EINVAL);
        assert!(f.pgftl.trans_map.is_null());
        assert!(f.pgftl.segments.is_null());
        assert_eq!(f.pgftl.gc_thread, 0);
    }

    #[test]
    fn allocation_failure_is_reported_as_enomem() {
        // The L2P table is `(total_size / page_size + 1) * 4` bytes, so 2^44
        // segments of four pages ask for a 256TiB allocation. The table is the
        // first allocation that scales with the geometry, and malloc refuses
        // it, which is C's `-ENOMEM` (`ftl/page/page-core.c:217-220`).
        let mut f = CoreFixture::new(1usize << 44, PAGES_PER_SEGMENT);
        let ret = unsafe { open(&mut f.pgftl, ptr::null(), FLAGS) };
        assert_eq!(ret, -libc::ENOMEM);

        // The failed open released everything it had taken.
        assert!(f.pgftl.trans_map.is_null());
        assert!(f.pgftl.segments.is_null());
        assert!(f.pgftl.bus_rwlock.is_null());
        assert!(f.pgftl.gc_seg_bits.is_null());
        assert_eq!(f.pgftl.gc_thread, 0);
    }

    #[test]
    fn checkpoint_round_trip_recovers_the_mapping() {
        let name = checkpoint_name("round-trip");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");
        let _ = std::fs::remove_file(&file);

        // Two writes through the dispatch, then a close that saves the mapping.
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let pgftl = f.pgftl.as_mut();
        for lpn in 0..2 {
            let mut payload = vec![0x30 + lpn as u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let saved_map = [
            unsafe { *pgftl.trans_map },
            unsafe { *pgftl.trans_map.add(1) },
        ];
        let saved_segnum = pgftl.alloc_segnum;
        let saved = unsafe { &*pgftl.segments.add(0) };
        let (saved_free, saved_valid, saved_list) = (
            saved.free_pages(),
            saved.nr_valid_pages.load(Ordering::SeqCst),
            unsafe { ffi_list_length(saved.lpn_list) },
        );
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        assert!(std::path::Path::new(&file).exists(), "close wrote the file");

        // A fresh FTL on the same name starts from the checkpoint: the mapping
        // and the segment metadata carried with it come back.
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let pgftl = f.pgftl.as_mut();
        assert_eq!(unsafe { *pgftl.trans_map }, saved_map[0]);
        assert_eq!(unsafe { *pgftl.trans_map.add(1) }, saved_map[1]);
        assert_eq!(pgftl.alloc_segnum, saved_segnum);
        let recovered = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(recovered.free_pages(), saved_free);
        assert_eq!(recovered.nr_valid_pages.load(Ordering::SeqCst), saved_valid);
        assert_eq!(unsafe { ffi_list_length(recovered.lpn_list) }, saved_list);
        assert!(!use_bits_clear(recovered), "the used bits are back");

        // The recovered accounting is live: the next write takes a fresh page
        // instead of one the mapping already holds.
        let mut payload = vec![0x99u8; LEN];
        let mut rq = request(
            DeviceDirection::Write,
            2 * DEVICE_PAGE_SIZE,
            &mut payload,
        );
        assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        assert_eq!(
            unsafe { *pgftl.trans_map.add(2) },
            device_address::from_segment_page(0, 2).lpn
        );
        assert_eq!(recovered.free_pages(), PAGES_PER_SEGMENT as i32 - 3);
        assert_eq!(
            recovered.nr_valid_pages.load(Ordering::SeqCst),
            saved_valid + 1
        );

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_keeps_a_programmed_page_out_of_the_allocator() {
        // An overwrite leaves the page it replaced physically programmed but
        // unmapped: the write path clears the mapping and leaves the used bit
        // and `nr_free_pages` alone (`ftl/page/page-write.c:43-47`), and only
        // the erase reclaims the page. A checkpoint that rebuilt `use_bits` from
        // the mapping would see that page as free, and the next allocation would
        // program it a second time, which the write once ramdisk refuses and
        // real NAND corrupts.
        let name = checkpoint_name("programmed");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");
        let _ = std::fs::remove_file(&file);

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let pgftl = f.pgftl.as_mut();

        // Fill the first segment, then overwrite its first page: the new copy
        // lands in the second segment and the first one keeps its four used bits
        // with one of them no longer mapped.
        for lpn in 0..PAGES_PER_SEGMENT {
            let mut payload = vec![0x40 + lpn as u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let mut payload = vec![0xa0u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        assert_eq!(unsafe { *pgftl.trans_map }, device_address::from_segment_page(1, 0).lpn);

        // Fill the second segment so no page is free, which is what makes the
        // stale page the only candidate a wrong rebuild could offer.
        for lpn in PAGES_PER_SEGMENT..2 * PAGES_PER_SEGMENT - 1 {
            let mut payload = vec![0x60 + lpn as u8; LEN];
            let mut rq = request(
                DeviceDirection::Write,
                lpn * DEVICE_PAGE_SIZE,
                &mut payload,
            );
            assert_eq!(submit_request(pgftl, &mut rq).unwrap(), LEN as isize);
        }
        let first = unsafe { &*pgftl.segments.add(0) };
        let second = unsafe { &*pgftl.segments.add(1) };
        assert_eq!(first.free_pages(), 0);
        assert_eq!(first.nr_valid_pages.load(Ordering::SeqCst), 3);
        assert_eq!(unsafe { *first.use_bits }, 0b1111, "all four pages are used");
        assert_eq!(second.free_pages(), 0);
        assert_eq!(unsafe { *second.use_bits }, 0b1111);

        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let pgftl = f.pgftl.as_mut();

        // The used bits and the free counts come back as they were, and the
        // mapping still points at the copy in the second segment.
        let first = unsafe { &*pgftl.segments.add(0) };
        let second = unsafe { &*pgftl.segments.add(1) };
        assert_eq!(unsafe { *first.use_bits }, 0b1111, "the stale page stays used");
        assert_eq!(first.free_pages(), 0);
        assert_eq!(first.nr_valid_pages.load(Ordering::SeqCst), 3);
        assert_eq!(unsafe { *second.use_bits }, 0b1111);
        assert_eq!(second.free_pages(), 0);
        assert_eq!(second.nr_valid_pages.load(Ordering::SeqCst), 4);
        assert_eq!(
            unsafe { *pgftl.trans_map },
            device_address::from_segment_page(1, 0).lpn
        );

        // Every page of both segments is used, so the allocator refuses the
        // write instead of handing out the stale page. `get_free_page` reports
        // `PADDR_EMPTY`, and the write path turns that into `-EFAULT`
        // (`ftl/page/page-write.c:190-192`), while a rebuild from the mapping
        // alone would have offered the stale page and the device would have
        // refused the second program with `-EINVAL`.
        let mut payload = vec![0x99u8; LEN];
        let mut rq = request(DeviceDirection::Write, 7 * DEVICE_PAGE_SIZE, &mut payload);
        assert_eq!(
            submit_request(pgftl, &mut rq).unwrap_err().raw_os_error(),
            Some(libc::EFAULT)
        );

        // Nothing was handed out and nothing was reprogrammed.
        assert_eq!(unsafe { *first.use_bits }, 0b1111);
        assert_eq!(first.free_pages(), 0);
        assert_eq!(first.nr_valid_pages.load(Ordering::SeqCst), 3);
        assert_eq!(unsafe { *second.use_bits }, 0b1111);
        assert_eq!(second.free_pages(), 0);
        assert_eq!(unsafe { *pgftl.trans_map.add(7) }, PADDR_EMPTY);

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_drops_a_duplicate_physical_page() {
        // Two entries on one page would count the page twice in
        // `nr_valid_pages` and send the collector after pages the segment does
        // not hold. Only a hand written file can hold them, and the second one
        // is dropped the way an out of range entry is.
        let name = checkpoint_name("duplicate");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");

        let mut mapping = empty_mapping();
        let shared = device_address::from_segment_page(0, 1).lpn;
        mapping[1] = shared;
        mapping[2] = shared;
        let image = checkpoint_image(
            NR_SEGMENTS as u64,
            PAGES_PER_SEGMENT as u64,
            0,
            &mapping,
            &[2, PAGES_PER_SEGMENT as u32],
        );
        std::fs::write(&file, &image).unwrap();

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );

        let pgftl = f.pgftl.as_mut();
        assert_eq!(unsafe { *pgftl.trans_map.add(1) }, shared, "the first entry stays");
        assert_eq!(
            unsafe { *pgftl.trans_map.add(2) },
            PADDR_EMPTY,
            "the repeat is dropped"
        );
        let segment = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 1);
        assert_eq!(segment.free_pages(), 2, "the free count stays as it came");
        assert_eq!(unsafe { ffi_list_length(segment.lpn_list) }, 1);

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_rejects_a_file_from_another_geometry() {
        // Two devices with the same page count and therefore the same entry
        // count but a different split: 1 x 8 and 2 x 4. The entries of one
        // decode in range on the other and would land in the wrong segment, so
        // the geometry in the header has to refuse the file.
        let name = checkpoint_name("geometry");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");
        let _ = std::fs::remove_file(&file);

        // A checkpoint the 1 x 8 device wrote itself.
        let mut f = CoreFixture::new(1, 8);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let mut payload = vec![0x77u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(f.pgftl.as_mut(), &mut rq).unwrap(), LEN as isize);
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        assert!(std::path::Path::new(&file).exists());

        // A 2 x 4 device must not apply it. Its own entry count is the same, so
        // only the geometry check can refuse this file: the sizes differ too,
        // but the geometry is read from the header and checked first.
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let pgftl = f.pgftl.as_mut();
        for lpn in 0..pgftl.map_entries() {
            assert_eq!(unsafe { *pgftl.trans_map.add(lpn) }, PADDR_EMPTY);
        }
        assert_eq!(pgftl.alloc_segnum, 0);
        for segnum in 0..NR_SEGMENTS {
            let segment = unsafe { &*pgftl.segments.add(segnum) };
            assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32);
            assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 0);
        }
        std::mem::drop(f);

        // The same file with the size the 2 x 4 device expects: only the
        // geometry check stands between it and acceptance, since its magic,
        // version, entry count and checksum are all right. It carries a mapping
        // and a cursor so accepting it would be visible.
        let mut mapping = empty_mapping();
        mapping[1] = device_address::from_segment_page(0, 0).lpn;
        let image = checkpoint_image(1, 8, 0, &mapping, &[3, PAGES_PER_SEGMENT as u32]);
        assert_eq!(image.len(), CHECKPOINT_HEADER_BYTES + 9 * 4 + 2 * 4);
        std::fs::write(&file, &image).unwrap();

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { *f.pgftl.trans_map.add(1) }, PADDR_EMPTY);
        assert_eq!(f.pgftl.alloc_segnum, 0);
        assert_eq!(
            unsafe { &*f.pgftl.segments }.free_pages(),
            PAGES_PER_SEGMENT as i32
        );

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_validation_falls_back_to_an_empty_mapping() {
        let name = checkpoint_name("validation");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");

        // Each file below fails one check, and everything before that check is
        // right, so the file reaches the branch it is built for.
        let valid = || {
            checkpoint_image(
                NR_SEGMENTS as u64,
                PAGES_PER_SEGMENT as u64,
                1,
                &empty_mapping(),
                &[PAGES_PER_SEGMENT as u32, PAGES_PER_SEGMENT as u32],
            )
        };
        let mut wrong_version = valid();
        wrong_version[4..8].copy_from_slice(&(CHECKPOINT_VERSION + 1).to_le_bytes());
        let mut wrong_entries = valid();
        wrong_entries[32..40].copy_from_slice(&7u64.to_le_bytes());
        let mut short_body = valid();
        short_body.truncate(short_body.len() - size_of::<u32>());
        let mut oversized = valid();
        oversized.extend_from_slice(&[0u8; 4096]);

        let cases: Vec<Vec<u8>> = vec![
            // No file at all.
            Vec::new(),
            // A header of zeros has a foreign magic.
            vec![0u8; CHECKPOINT_HEADER_BYTES],
            // Shorter than a header, so nothing is read into the body.
            vec![0u8; 10],
            // Same page count, another segment and page split.
            checkpoint_image(1, 8, 0, &empty_mapping(), &[0, 0]),
            // An allocation cursor at the segment array's end.
            checkpoint_image(
                NR_SEGMENTS as u64,
                PAGES_PER_SEGMENT as u64,
                NR_SEGMENTS as u64,
                &empty_mapping(),
                &[PAGES_PER_SEGMENT as u32, PAGES_PER_SEGMENT as u32],
            ),
            // A free page count above the segment's page count.
            checkpoint_image(
                NR_SEGMENTS as u64,
                PAGES_PER_SEGMENT as u64,
                0,
                &empty_mapping(),
                &[PAGES_PER_SEGMENT as u32 + 1, PAGES_PER_SEGMENT as u32],
            ),
            wrong_version,
            wrong_entries,
            short_body,
            oversized,
        ];

        for (index, case) in cases.iter().enumerate() {
            if case.is_empty() {
                let _ = std::fs::remove_file(&file);
            } else {
                std::fs::write(&file, case).unwrap();
            }
            let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
            assert_eq!(
                unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
                0
            );
            let pgftl = f.pgftl.as_mut();
            for lpn in 0..pgftl.map_entries() {
                assert_eq!(
                    unsafe { *pgftl.trans_map.add(lpn) },
                    PADDR_EMPTY,
                    "case {} entry {}",
                    index,
                    lpn
                );
            }
            assert_eq!(pgftl.alloc_segnum, 0, "case {index}");
            assert_eq!(
                unsafe { &*pgftl.segments }.free_pages(),
                PAGES_PER_SEGMENT as i32,
                "case {index}"
            );
            std::mem::drop(f);
        }

        // A real checkpoint with one flipped byte in the body is refused by the
        // checksum, and a flipped byte in the header by the same field, which
        // covers the allocation cursor as well as the mapping.
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let mut payload = vec![0x77u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(f.pgftl.as_mut(), &mut rq).unwrap(), LEN as isize);
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        let whole = std::fs::read(&file).unwrap();

        for offset in [CHECKPOINT_HEADER_BYTES, 8] {
            let mut damaged = whole.clone();
            damaged[offset] ^= 0xff;
            std::fs::write(&file, &damaged).unwrap();

            let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
            assert_eq!(
                unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
                0
            );
            assert_eq!(unsafe { *f.pgftl.trans_map }, PADDR_EMPTY, "byte {offset}");
            assert_eq!(f.pgftl.alloc_segnum, 0, "byte {offset}");
        }

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_restores_in_range_entries_and_drops_the_rest() {
        // A hand written checkpoint with a correct checksum. Only a file
        // another producer made reaches this, and the port drops every entry it
        // cannot honour: one outside the segment array, one whose page is not
        // programmed, and one that repeats a page an earlier entry claimed.
        let name = checkpoint_name("out-of-range");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");

        // Segment 0 has three free pages, so one page of it is programmed, and
        // the entry that is applied takes it.
        let mut mapping = empty_mapping();
        let inside = device_address::from_segment_page(0, 0).lpn;
        let unprogrammed = device_address::from_segment_page(0, 3).lpn;
        let outside = device_address::from_segment_page(NR_SEGMENTS as u16 + 1, 0).lpn;
        mapping[1] = inside;
        mapping[2] = unprogrammed;
        mapping[3] = outside;
        mapping[4] = inside;

        let image = checkpoint_image(
            NR_SEGMENTS as u64,
            PAGES_PER_SEGMENT as u64,
            1,
            &mapping,
            &[PAGES_PER_SEGMENT as u32 - 1, PAGES_PER_SEGMENT as u32],
        );
        std::fs::write(&file, &image).unwrap();

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );

        let pgftl = f.pgftl.as_mut();
        assert_eq!(unsafe { *pgftl.trans_map.add(1) }, inside, "the mapping is applied");
        assert_eq!(
            unsafe { *pgftl.trans_map.add(2) },
            PADDR_EMPTY,
            "an entry on a page that is not programmed is dropped"
        );
        assert_eq!(
            unsafe { *pgftl.trans_map.add(3) },
            PADDR_EMPTY,
            "an entry outside the device is dropped"
        );
        assert_eq!(
            unsafe { *pgftl.trans_map.add(4) },
            PADDR_EMPTY,
            "an entry that repeats a page is dropped"
        );
        assert_eq!(unsafe { *pgftl.trans_map }, PADDR_EMPTY);
        assert_eq!(pgftl.alloc_segnum, 1, "the cursor is applied");

        // The accounting of the entry that was applied is rebuilt, and the
        // programmed page of segment 0 is marked used.
        let segment = unsafe { &*pgftl.segments.add(0) };
        assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 1);
        assert_eq!(unsafe { ffi_list_length(segment.lpn_list) }, 1);
        assert_eq!(unsafe { *segment.use_bits }, 0b0001);
        let empty = unsafe { &*pgftl.segments.add(1) };
        assert_eq!(empty.free_pages(), PAGES_PER_SEGMENT as i32);
        assert_eq!(empty.nr_valid_pages.load(Ordering::SeqCst), 0);
        assert!(empty.lpn_list.is_null());
        assert_eq!(unsafe { *empty.use_bits }, 0);

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_keeps_the_previous_file_when_the_next_save_fails() {
        // The replacement is atomic, so a save that cannot complete leaves the
        // last good file in place instead of truncating it into an empty
        // mapping.
        let name = checkpoint_name("atomic");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");
        let temporary = format!("{file}.tmp");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&temporary);

        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        let mut payload = vec![0x12u8; LEN];
        let mut rq = request(DeviceDirection::Write, 0, &mut payload);
        assert_eq!(submit_request(f.pgftl.as_mut(), &mut rq).unwrap(), LEN as isize);
        let saved_entry = unsafe { *f.pgftl.trans_map };
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        let saved = std::fs::read(&file).unwrap();

        // The next save cannot create its temporary, because that path is a
        // directory.
        std::fs::create_dir(&temporary).unwrap();
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { *f.pgftl.trans_map }, saved_entry, "the load worked");

        // Give this FTL state that differs from the saved file, so a save that
        // wrote the target directly instead of replacing it through a rename
        // would leave different bytes behind and fail the assertion below.
        let mut extra = vec![0x34u8; LEN];
        let mut rq = request(DeviceDirection::Write, DEVICE_PAGE_SIZE, &mut extra);
        assert_eq!(
            submit_request(f.pgftl.as_mut(), &mut rq).unwrap(),
            LEN as isize
        );
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        // The target still holds the last good checkpoint, byte for byte, and
        // it still loads.
        assert_eq!(std::fs::read(&file).unwrap(), saved);
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { *f.pgftl.trans_map }, saved_entry);
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        let _ = std::fs::remove_dir(&temporary);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checkpoint_failures_leave_open_and_close_successful() {
        // A checkpoint nothing can write: the directory does not exist, so the
        // load is skipped and the save fails without failing the close.
        let name = format!(
            "{}/ftl-page-core-{}/image",
            std::env::temp_dir().display(),
            std::process::id()
        );
        let name_c = std::ffi::CString::new(name).unwrap();
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        // A name that is not valid UTF-8 has no checkpoint path at all.
        let raw = std::ffi::CString::new(vec![0x80u8, 0xffu8]).unwrap();
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, raw.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);

        // An empty name names no checkpoint either, so the port does not write
        // a `.ckpt` beside the device it was handed.
        let empty = std::ffi::CString::new("").unwrap();
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, empty.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        assert!(!std::path::Path::new(".ckpt").exists());

        // A checkpoint path that exists but cannot be read, because it is a
        // directory: the load reports the read error and the save fails. Both
        // are warnings, so the open and the close still succeed.
        let name = checkpoint_name("directory");
        let name_c = std::ffi::CString::new(name.clone()).unwrap();
        let file = format!("{name}.ckpt");
        std::fs::create_dir(&file).unwrap();
        let mut f = CoreFixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        assert_eq!(
            unsafe { open(&mut f.pgftl, name_c.as_ptr(), FLAGS) },
            0
        );
        assert_eq!(unsafe { *f.pgftl.trans_map }, PADDR_EMPTY);
        assert_eq!(unsafe { close(&mut f.pgftl) }, 0);
        std::fs::remove_dir(&file).unwrap();
    }

    #[test]
    fn dispatch_rejects_a_logical_page_past_the_table() {
        // The dispatch reports the read and write paths' bound the same way:
        // a sector past the map never reaches the device.
        let mut f = opened_fixture();
        let pgftl = f.pgftl.as_mut();
        let page_size = pgftl.dev().page_size();
        let sector = pgftl.map_entries() * page_size;

        let mut payload = vec![0u8; LEN];
        let mut write_rq = request(DeviceDirection::Write, sector, &mut payload);
        assert_eq!(
            submit_request(pgftl, &mut write_rq)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );

        let mut buf = vec![0u8; LEN];
        let mut read_rq = request(DeviceDirection::Read, sector, &mut buf);
        assert_eq!(
            submit_request(pgftl, &mut read_rq)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );

        // The device never saw a request: the segments are untouched.
        for segnum in 0..NR_SEGMENTS {
            let segment = unsafe { &*pgftl.segments.add(segnum) };
            assert_eq!(segment.free_pages(), PAGES_PER_SEGMENT as i32);
            assert_eq!(segment.nr_valid_pages.load(Ordering::SeqCst), 0);
        }
    }
}
