//! BlueDBM backend, the Rust port of `device/bluedbm/bluedbm.c`.
//!
//! [`Bluedbm`] drives a Flash Board Tester through libmemio, kept one to one
//! with the C: the same geometry, the same order of operations, the same DMA
//! buffer ownership and free ordering, and the same return values and error
//! codes. The C ABI layer keeps the C entry points with the `ffi_` prefix
//! (`ffi_bluedbm_device_init` / `ffi_bluedbm_device_exit` /
//! `ffi_bluedbm_close`). `device/device.c` installs the backend through the
//! init and exit pair, and `ffi_bluedbm_close` is the C close entry, reachable
//! as before and dispatched through `dev->d_op` like the ramdisk's.
//! `device/bluedbm/bluedbm.c` is gone.
//!
//! libmemio is bound by hand below with `extern "C"` plus
//! `#[link(name = "memio")]`, as the issue asks. The library is not installed
//! in this repository's build environment, so the three cargo commands that
//! link it cannot be observed here. See the module tests and the issue notes.
//!
//! C to Rust table (C line, C symbol, Rust item):
//!
//! | C | symbol | Rust |
//! |---|--------|------|
//! | 22 | `g_badseg_counter` | [`G_BADSEG_COUNTER`] |
//! | 23 | `g_erase_counter` | [`G_ERASE_COUNTER`] |
//! | 34 | `bluedbm_erase_end_request` | [`erase_end_request`] |
//! | 52 | `bluedbm_clear` | [`Bluedbm::clear`] |
//! | 87 | `bluedbm_wait_erase_finish` | [`wait_erase_finish`] |
//! | 115 | `bluedbm_open` | [`Bluedbm::open`] + [`Bluedbm::open_storage`] |
//! | 199 | `bluedbm_end_rw_request` | [`end_rw_request`] |
//! | 238 | `bluedbm_write` | [`Bluedbm::write`] |
//! | 327 | `bluedbm_read` | [`Bluedbm::read`] |
//! | 405 | `bluedbm_erase` | [`Bluedbm::erase`] |
//! | 499 | `bluedbm_close` | [`Bluedbm::close`] |
//! | 522 | `__bluedbm_dops` | the `DeviceOperations` vtable |
//! | 536 | `bluedbm_device_init` | [`ffi_bluedbm_device_init`] |
//! | 565 | `bluedbm_device_exit` | [`ffi_bluedbm_device_exit`] |
//! | 13 | `bluedbm_dma_t` | [`BluedbmDma`] |
//! | 20 | `struct bluedbm` | [`Bluedbm`] |
//!
//! Deliberate deviations from the C original, each idiomatic rather than
//! literal:
//!
//! - The geometry arithmetic uses the BlueDBM build's bit counts (`3/3/7/19`,
//!   the Makefile's `USE_BLUEDBM_DEVICE=1` `DEVICE_INFO`), held in
//!   [`BLUEDBM_NR_BUS_BITS`] and friends, not the default counts in
//!   [`crate::device`]. C compiles `bluedbm.c` with the same `-D` flags, so
//!   `addr.format.block` in C occupies bits 13 and up. The crate's
//!   [`crate::device::device_address`] helpers are built from the default
//!   (ramdisk) counts, so this module packs and unpacks the segment field with
//!   the BlueDBM shift instead of those helpers.
//! - `struct bluedbm`'s fields become `Cell`s behind `&self`, because
//!   [`DeviceOperations`] hands out `&self` and `open` / `close` mutate the
//!   handle in place, exactly as C mutates `dev->d_private`. `o_flags` stays
//!   (C reads it in the `O_CREAT` check). `size` is dropped: C stores
//!   `device_get_total_size` in it and never reads it back, and neither does
//!   the port.
//! - C reaches `struct bluedbm` by casting `dev->d_private`. The port holds it
//!   as `self`, the backend the vtable was built from, so the type behind the
//!   vtable can never disagree with it.
//! - The two process-global counters stay process-global, because
//!   `memio_trim`'s `end_req` callback carries no context to reach a
//!   per-device structure through. They are `AtomicPtr<AtomicI32>` where C has
//!   `gint *`, and the `g_atomic_int_*` calls become the matching
//!   [`AtomicI32`] operations.
//! - `bluedbm_open` is split at the `memio_open` call: [`Bluedbm::open`] fills
//!   the geometry and opens the handle, [`Bluedbm::open_storage`] takes the
//!   handle and allocates the bitmap and the two counters. The split lets the
//!   unit tests drive the allocation and the `O_CREAT` check with a non-null
//!   handle and no libmemio, without changing the order of operations.
//! - The `malloc` of the small DMA and request structures cannot fail in Rust:
//!   `Box` aborts on OOM, as in [`crate::device::ramdisk`]. C's `-errno`
//!   (write) and `-ENOMEM` (read) returns for that allocation go with it.
//! - On a re-open, C overwrites `dev->badseg_bitmap` and the two counter
//!   globals and leaks the old buffers. The port releases them first, the way
//!   [`Bluedbm::close`] does, so the two paths cannot disagree.
//! - When `memio_write` / `memio_read` returns a negative value, C returns it
//!   and leaks the DMA and request structures. The port releases both before
//!   returning, because the structures are `Box`es it still owns there.
//! - [`Bluedbm::erase`] calls `request->end_rq` before the trim, as C does,
//!   and C's `(uint32_t)` cast of `device_get_pages_per_segment` is kept for
//!   the `erase_size` product.

use core::cell::Cell;
use core::ffi::{c_char, c_int, c_void};
use core::mem::size_of;
use core::ptr;
use core::slice;
use core::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

use crate::bits::{set_bit, BITS_PER_UINT64};
use crate::device::{device, device_request, DeviceOperations, DEVICE_PAGE_SIZE};
use crate::{pr_err, pr_warn};

/// C's `BLUEDBM_NR_BLOCKS`: the number of blocks (segments) in the board.
const BLUEDBM_NR_BLOCKS: usize = 4096;

/// The Makefile's `USE_BLUEDBM_DEVICE=1` `DEVICE_INFO` bit counts. C compiles
/// `bluedbm.c` and the FTL with these, so the segment field of a
/// `device_address` sits at bit 13, not at the default build's bit 11.
const BLUEDBM_NR_BUS_BITS: u32 = 3;
const BLUEDBM_NR_CHIPS_BITS: u32 = 3;
const BLUEDBM_NR_PAGES_BITS: u32 = 7;
const BLUEDBM_NR_BLOCKS_BITS: u32 = 19;

/// Width of the bus/chip/page run below the segment field (C's
/// `DEVICE_NR_PAGES_BITS + DEVICE_NR_CHIPS_BITS + DEVICE_NR_BUS_BITS`).
const BLUEDBM_BLOCK_SHIFT: u32 =
    BLUEDBM_NR_BUS_BITS + BLUEDBM_NR_CHIPS_BITS + BLUEDBM_NR_PAGES_BITS;

/// Mask of the segment field in a `device_address`.
const BLUEDBM_BLOCK_MASK: u32 = (1u32 << BLUEDBM_NR_BLOCKS_BITS) - 1;

/// C's `REQTYPE_IO_READ` / `REQTYPE_IO_WRITE` from `bdbm_drv.h`, the value
/// `async_bdbm_req.type` carries.
const REQTYPE_IO_READ: u32 = 0x000001;
const REQTYPE_IO_WRITE: u32 = 0x000004;

/// C's `enum _dma_type` from `libmemio.h`.
const DMA_WRITE_BUF: c_int = 1;
const DMA_READ_BUF: c_int = 2;

/// `memio_t` from `libmemio.h`, opaque here. C only ever holds a pointer to it
/// (the header defines it in C++ terms with a `std::queue` member), and so does
/// the port. Its layout is never read, so an opaque zero-sized type is enough.
#[repr(C)]
struct memio_t {
    _private: [u8; 0],
}

/// Mirrors `bluedbm_dma_t` in `include/bluedbm.h`.
///
/// `data` is the DMA buffer `memio_alloc_dma` hands back, `tag` its tag for
/// `memio_free_dma`, and `d_private` the user `device_request`.
#[repr(C)]
struct BluedbmDma {
    tag: u32,
    data: *mut c_char,
    d_private: *mut c_void,
}

/// Signature of `memio_trim`'s `end_req` callback, C's
/// `void (*end_req)(uint64_t, uint8_t)`.
type EraseEndReqFn = unsafe extern "C" fn(segnum: u64, is_bad: u8);

/// Signature of `async_bdbm_req.end_req`, C's
/// `void (*end_req)(struct async_bdbm_req *)`.
type RwEndReqFn = unsafe extern "C" fn(rw_req: *mut AsyncBdbmReq);

/// Mirrors `async_bdbm_req` in `libmemio.h`.
///
/// The C++ header declares it as plain C data, so this is its exact layout:
/// `type`, `end_req`, `private_data`. `type` is renamed `type_` because `type`
/// is a Rust keyword. The names are not part of the ABI, only the order is.
#[repr(C)]
struct AsyncBdbmReq {
    type_: u32,
    end_req: Option<RwEndReqFn>,
    private_data: *mut c_void,
}

// libmemio, bound by hand. The C ABI names are the ones `bluedbm.c` calls.
//
// The upstream `libmemio.cpp` declares none of these `extern "C"` and its
// header is C++ (`std::queue`, `bool`), so if an installed build mangles the
// names this `extern "C"` block will not link and a narrow C++ wrapper
// compiled with `g++` would be needed instead. The library is not installed
// here, so that could not be observed. See the module docs.
#[link(name = "memio")]
extern "C" {
    fn memio_open() -> *mut memio_t;
    fn memio_close(mio: *mut memio_t);
    fn memio_read(
        mio: *mut memio_t,
        lba: u32,
        len: u64,
        data: *mut u8,
        is_async: c_int,
        req: *mut c_void,
        dma_tag: c_int,
    ) -> c_int;
    fn memio_write(
        mio: *mut memio_t,
        lba: u32,
        len: u64,
        data: *mut u8,
        is_async: c_int,
        req: *mut c_void,
        dma_tag: c_int,
    ) -> c_int;
    fn memio_trim(mio: *mut memio_t, lba: u32, len: u64, end_req: EraseEndReqFn) -> c_int;
    fn memio_alloc_dma(type_: c_int, buf: *mut *mut c_char) -> c_int;
    fn memio_free_dma(type_: c_int, dma_tag: c_int);
}

/// C's `g_erase_counter`: one atomic counter per segment, the number of blocks
/// erased since the counter was last cleared.
static G_ERASE_COUNTER: AtomicPtr<AtomicI32> = AtomicPtr::new(ptr::null_mut());

/// C's `g_badseg_counter`: one atomic counter per segment, non-zero once the
/// segment is reported bad.
static G_BADSEG_COUNTER: AtomicPtr<AtomicI32> = AtomicPtr::new(ptr::null_mut());

/// BlueDBM backend, the Rust form of C's `struct bluedbm`.
///
/// The fields are `Cell`s because [`DeviceOperations`] hands out `&self` and
/// `open` / `close` mutate the handle in place. There is no lock, matching C.
pub struct Bluedbm {
    mio: Cell<*mut memio_t>,
    o_flags: Cell<c_int>,
}

// The backend contract in [`DeviceOperations`] says an implementation is
// callable from any thread and the caller must not issue overlapping requests,
// which is the deal C's `bluedbm` makes. `Cell` is what would otherwise make
// this `!Sync`.
unsafe impl Sync for Bluedbm {}

/// Fill the flash geometry, the first block of C's `bluedbm_open`: the bus and
/// chip counts, the block count, the pages per block and the page size.
///
/// Factored out so the unit tests check the geometry without opening memio.
///
/// # Safety
/// `dev` is the device this backend was installed into.
unsafe fn configure_geometry(dev: *mut device) {
    (*dev).info.nr_bus = 1usize << BLUEDBM_NR_BUS_BITS;
    (*dev).info.nr_chips = 1usize << BLUEDBM_NR_CHIPS_BITS;
    (*dev).info.package.nr_blocks = BLUEDBM_NR_BLOCKS;
    (*dev).info.package.block.nr_pages = 1usize << BLUEDBM_NR_PAGES_BITS;
    (*dev).info.package.block.page.size = DEVICE_PAGE_SIZE;
}

impl Bluedbm {
    /// A backend with no memio handle, the state C's `memset(bdbm, 0, ...)`
    /// leaves behind.
    fn new() -> Self {
        Self {
            mio: Cell::new(ptr::null_mut()),
            o_flags: Cell::new(0),
        }
    }

    /// The body of C's `bluedbm_open` after `memio_open` succeeds: record the
    /// handle and the flags, allocate the bad segment bitmap and the two
    /// counters, and clear the board when `O_CREAT` is set.
    ///
    /// Split out of [`DeviceOperations::open`] so the unit tests can drive it
    /// with a non-null handle and no `O_CREAT`, without libmemio. The order of
    /// operations matches C.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn open_storage(
        &self,
        dev: *mut device,
        mio: *mut memio_t,
        flags: c_int,
    ) -> c_int {
        // C stores `device_get_total_size` in `bdbm->size` and never reads it.
        self.o_flags.set(flags);
        self.mio.set(mio);

        let nr_segments = (*dev).nr_segments();

        // C leaves the previous bitmap in place and leaks it. Release it the
        // way `close` does.
        if !(*dev).badseg_bitmap.is_null() {
            libc::free((*dev).badseg_bitmap as *mut c_void);
            (*dev).badseg_bitmap = ptr::null_mut();
        }
        let badseg = libc::calloc(
            nr_segments.div_ceil(BITS_PER_UINT64),
            size_of::<u64>(),
        ) as *mut u64;
        if badseg.is_null() {
            pr_err!("memory allocation failed");
            return -libc::ENOMEM;
        }
        (*dev).badseg_bitmap = badseg;

        free_counter(&G_ERASE_COUNTER);
        let erase_counter =
            libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32;
        if erase_counter.is_null() {
            pr_err!("memory allocation failed");
            return -libc::ENOMEM;
        }
        G_ERASE_COUNTER.store(erase_counter, Ordering::SeqCst);

        free_counter(&G_BADSEG_COUNTER);
        let badseg_counter =
            libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32;
        if badseg_counter.is_null() {
            pr_err!("memory allocation failed");
            return -libc::ENOMEM;
        }
        G_BADSEG_COUNTER.store(badseg_counter, Ordering::SeqCst);

        if self.o_flags.get() & libc::O_CREAT != 0 {
            let _ = self.clear(dev);
            libc::sleep(1);
            wait_erase_finish(dev, 0, nr_segments);
        }

        0
    }

    /// Clear every segment in the board, mirroring C's `bluedbm_clear`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn clear(&self, dev: *mut device) -> c_int {
        let mio = self.mio.get();
        if mio.is_null() {
            pr_err!("mio must be specified.");
            return -libc::EINVAL;
        }

        let pages_per_segment = (*dev).pages_per_segment();
        let erase_size = pages_per_segment * (*dev).page_size();
        // C's `addr.format.block = segnum` with the BlueDBM bit counts, so the
        // segment sits at bit 13.
        for segnum in 0..(*dev).nr_segments() {
            let lpn = (segnum as u32 & BLUEDBM_BLOCK_MASK) << BLUEDBM_BLOCK_SHIFT;
            memio_trim(mio, lpn, erase_size as u64, erase_end_request);
        }
        0
    }
}

impl DeviceOperations for Bluedbm {
    /// Open the board, mirroring C's `bluedbm_open`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn open(&self, dev: *mut device, _name: *const c_char, flags: c_int) -> c_int {
        configure_geometry(dev);

        let mio = memio_open();
        if mio.is_null() {
            pr_err!("memio open failed");
            let ret = -libc::EFAULT;
            let _ = self.close(dev);
            return ret;
        }

        self.open_storage(dev, mio, flags)
    }

    /// Write one page, mirroring C's `bluedbm_write`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn write(&self, dev: *mut device, request: *mut device_request) -> isize {
        let mio = self.mio.get();
        if mio.is_null() {
            pr_err!("memio global structure doesn't exist");
            return -(libc::EFAULT as isize);
        }
        if (*request).data.is_null() {
            pr_err!("you do not pass the data pointer to NULL");
            return -(libc::ENODATA as isize);
        }
        if (*request).paddr.lpn == crate::device::PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }
        if (*request).flag != crate::device::DeviceDirection::Write as u32 {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                crate::device::DeviceDirection::Write as u32,
                (*request).flag
            );
            return -(libc::EINVAL as isize);
        }
        let page_size = (*dev).page_size();
        if (*request).data_len != page_size {
            // C passes `(request->data_len, page_size)` to this format string,
            // so the two print under the wrong labels. Kept as C has it.
            pr_err!(
                "data write size is must be {} (current: {})",
                (*request).data_len,
                page_size
            );
            return -(libc::EINVAL as isize);
        }

        let lpn = (*request).paddr.lpn;

        let dma = Box::into_raw(Box::new(BluedbmDma {
            tag: 0,
            data: ptr::null_mut(),
            d_private: ptr::null_mut(),
        }));
        (*dma).tag =
            memio_alloc_dma(DMA_WRITE_BUF, ptr::addr_of_mut!((*dma).data)) as u32;
        (*dma).d_private = request as *mut c_void;
        ptr::copy_nonoverlapping(
            (*request).data as *const u8,
            (*dma).data.cast::<u8>(),
            page_size,
        );

        let write_rq = Box::into_raw(Box::new(AsyncBdbmReq {
            type_: REQTYPE_IO_WRITE,
            end_req: Some(end_rw_request),
            private_data: dma as *mut c_void,
        }));

        let ret = memio_write(
            mio,
            lpn,
            page_size as u64,
            (*dma).data.cast::<u8>(),
            0,
            write_rq as *mut c_void,
            (*dma).tag as c_int,
        );
        if ret < 0 {
            // C leaks both here. The port still owns them, so release them.
            drop(Box::from_raw(write_rq));
            drop(Box::from_raw(dma));
        }
        ret as isize
    }

    /// Read one page, mirroring C's `bluedbm_read`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn read(&self, dev: *mut device, request: *mut device_request) -> isize {
        let mio = self.mio.get();
        if mio.is_null() {
            pr_err!("memio global structure doesn't exist");
            return -(libc::EFAULT as isize);
        }
        if (*request).data.is_null() {
            pr_err!("you do not pass the data pointer to NULL");
            return -(libc::ENODATA as isize);
        }
        if (*request).flag != crate::device::DeviceDirection::Read as u32 {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                crate::device::DeviceDirection::Read as u32,
                (*request).flag
            );
            return -(libc::EINVAL as isize);
        }
        if (*request).paddr.lpn == crate::device::PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }
        let page_size = (*dev).page_size();
        if (*request).data_len != page_size {
            pr_err!(
                "data read size is must be {} (current: {})",
                page_size,
                (*request).data_len
            );
            return -(libc::EINVAL as isize);
        }

        let lpn = (*request).paddr.lpn;

        let dma = Box::into_raw(Box::new(BluedbmDma {
            tag: 0,
            data: ptr::null_mut(),
            d_private: ptr::null_mut(),
        }));
        (*dma).tag = memio_alloc_dma(DMA_READ_BUF, ptr::addr_of_mut!((*dma).data)) as u32;
        (*dma).d_private = request as *mut c_void;

        let read_rq = Box::into_raw(Box::new(AsyncBdbmReq {
            type_: REQTYPE_IO_READ,
            end_req: Some(end_rw_request),
            private_data: dma as *mut c_void,
        }));

        let ret = memio_read(
            mio,
            lpn,
            page_size as u64,
            (*dma).data.cast::<u8>(),
            0,
            read_rq as *mut c_void,
            (*dma).tag as c_int,
        );
        if ret < 0 {
            // C leaks both here. The port still owns them, so release them.
            drop(Box::from_raw(read_rq));
            drop(Box::from_raw(dma));
        }
        ret as isize
    }

    /// Erase a segment, mirroring C's `bluedbm_erase`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn erase(&self, dev: *mut device, request: *mut device_request) -> c_int {
        let mio = self.mio.get();
        if mio.is_null() {
            pr_err!("memio global structure doesn't exist");
            return -libc::EFAULT;
        }
        if (*request).flag != crate::device::DeviceDirection::Erase as u32 {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                crate::device::DeviceDirection::Erase as u32,
                (*request).flag
            );
            return -libc::EINVAL;
        }
        if (*request).paddr.lpn == crate::device::PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -libc::EINVAL;
        }

        let page_size = (*dev).page_size();
        // C's `(uint32_t)device_get_pages_per_segment`.
        let pages_per_segment = (*dev).pages_per_segment() as u32;
        // C's `segnum = addr.format.block` with the BlueDBM bit counts.
        let segnum = ((*request).paddr.lpn >> BLUEDBM_BLOCK_SHIFT) & BLUEDBM_BLOCK_MASK;

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        let erase_size = pages_per_segment as usize * page_size;
        let lpn = segnum << BLUEDBM_BLOCK_SHIFT;
        memio_trim(mio, lpn, erase_size as u64, erase_end_request);
        wait_erase_finish(dev, segnum as usize, 1);
        0
    }

    /// Close the board, mirroring C's `bluedbm_close`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn close(&self, dev: *mut device) -> c_int {
        if !self.mio.get().is_null() {
            memio_close(self.mio.get());
            self.mio.set(ptr::null_mut());
        }
        if !(*dev).badseg_bitmap.is_null() {
            libc::free((*dev).badseg_bitmap as *mut c_void);
            (*dev).badseg_bitmap = ptr::null_mut();
        }
        free_counter(&G_ERASE_COUNTER);
        free_counter(&G_BADSEG_COUNTER);
        0
    }
}

/// The `memio_trim` erase callback, mirroring C's `bluedbm_erase_end_request`.
///
/// It counts one erase for `segnum` and, when `is_bad`, one bad detection. The
/// counters are process-global because the callback has no context.
unsafe extern "C" fn erase_end_request(segnum: u64, is_bad: u8) {
    if G_BADSEG_COUNTER.load(Ordering::SeqCst).is_null()
        || G_ERASE_COUNTER.load(Ordering::SeqCst).is_null()
    {
        pr_err!("NULL pointer detected");
        return;
    }
    counter_inc(&G_ERASE_COUNTER, segnum as usize);
    if is_bad != 0 {
        counter_inc(&G_BADSEG_COUNTER, segnum as usize);
    }
}

/// Wait for the erase of `nr_segments` segments starting at `segnum` to finish,
/// mirroring C's `bluedbm_wait_erase_finish`. Marks a segment bad when its bad
/// counter is non-zero, and clears the erase counter once every block in the
/// segment has been erased. Sleeps one second between polls, as C does.
///
/// # Safety
/// `dev` is the device this backend was installed into and both counter
/// globals point at their allocation.
unsafe fn wait_erase_finish(dev: *mut device, segnum: usize, nr_segments: usize) {
    let end_segment = segnum + nr_segments;
    let blocks_per_segment = (*dev).blocks_per_segment();
    let mut segnum = segnum;
    while segnum < end_segment {
        if counter_get(&G_BADSEG_COUNTER, segnum) != 0 {
            let len = (*dev).nr_segments().div_ceil(BITS_PER_UINT64);
            let bitmap = slice::from_raw_parts_mut((*dev).badseg_bitmap, len);
            set_bit(bitmap, segnum as u64);
            segnum += 1;
            continue;
        }

        if counter_get(&G_ERASE_COUNTER, segnum) as usize == blocks_per_segment {
            counter_set(&G_ERASE_COUNTER, segnum, 0);
            segnum += 1;
            continue;
        }
        libc::sleep(1);
    }
}

/// The read/write completion callback, mirroring C's `bluedbm_end_rw_request`.
///
/// A write frees its DMA write buffer, a read copies the DMA buffer back into
/// the user request first, then both free the DMA and request structures and
/// run the user's `end_rq`.
unsafe extern "C" fn end_rw_request(rw_req: *mut AsyncBdbmReq) {
    if rw_req.is_null() {
        pr_warn!("NULL rw_req detected");
        return;
    }

    let dma = (*rw_req).private_data as *mut BluedbmDma;
    if dma.is_null() {
        pr_warn!("NULL request detected (rw_req: {:p})", rw_req);
        drop(Box::from_raw(rw_req));
        return;
    }
    let user_rq = (*dma).d_private as *mut device_request;
    assert!(!user_rq.is_null());

    match (*rw_req).type_ {
        REQTYPE_IO_WRITE => memio_free_dma(DMA_WRITE_BUF, (*dma).tag as c_int),
        REQTYPE_IO_READ => {
            ptr::copy_nonoverlapping(
                (*dma).data as *const u8,
                (*user_rq).data as *mut u8,
                (*user_rq).data_len,
            );
            memio_free_dma(DMA_READ_BUF, (*dma).tag as c_int);
        }
        _ => pr_warn!("unknown request type detected (flag: {})", (*rw_req).type_),
    }
    drop(Box::from_raw(dma));
    drop(Box::from_raw(rw_req));

    if let Some(end_rq) = (*user_rq).end_rq {
        end_rq(user_rq);
    }
}

/// Read the atomic counter for `segnum`, C's `g_atomic_int_get`.
///
/// # Safety
/// `counter` points at its allocation, which holds at least `segnum + 1`
/// counters.
unsafe fn counter_get(counter: &AtomicPtr<AtomicI32>, segnum: usize) -> c_int {
    (*counter.load(Ordering::SeqCst).add(segnum)).load(Ordering::SeqCst)
}

/// Store the atomic counter for `segnum`, C's `g_atomic_int_set`.
///
/// # Safety
/// `counter` points at its allocation, which holds at least `segnum + 1`
/// counters.
unsafe fn counter_set(counter: &AtomicPtr<AtomicI32>, segnum: usize, value: c_int) {
    (*counter.load(Ordering::SeqCst).add(segnum)).store(value, Ordering::SeqCst);
}

/// Increment the atomic counter for `segnum`, C's `g_atomic_int_inc`.
///
/// # Safety
/// `counter` points at its allocation, which holds at least `segnum + 1`
/// counters.
unsafe fn counter_inc(counter: &AtomicPtr<AtomicI32>, segnum: usize) {
    (*counter.load(Ordering::SeqCst).add(segnum)).fetch_add(1, Ordering::SeqCst);
}

/// Release a counter global the way C's `free` does in `bluedbm_close`, and
/// clear it. A `NULL` pointer is a no-op, as in C.
///
/// # Safety
/// The pointer, when non-null, came from `calloc` and no callback is using it.
unsafe fn free_counter(counter: &AtomicPtr<AtomicI32>) {
    let ptr = counter.load(Ordering::SeqCst);
    if !ptr.is_null() {
        libc::free(ptr as *mut c_void);
        counter.store(ptr::null_mut(), Ordering::SeqCst);
    }
}

/// Close the board, mirroring C's `bluedbm_close` reached through `dev->d_op`.
///
/// # Safety
/// `dev` is `NULL` or a live device.
#[no_mangle]
pub unsafe extern "C" fn ffi_bluedbm_close(dev: *mut device) -> c_int {
    if dev.is_null() {
        return -libc::EINVAL;
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.close) {
        Some(close) => close(dev),
        None => -libc::ENODEV,
    }
}

/// Initialize the BlueDBM backend, mirroring C's `bluedbm_device_init`. C
/// allocates the `struct bluedbm`, zeroes it, and installs the vtable. The
/// init ignores `flags` and does not open the board, and neither does this.
///
/// # Safety
/// `dev` is `NULL` or a live device with no backend installed yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_bluedbm_device_init(dev: *mut device, flags: u64) -> c_int {
    let _ = flags;
    if dev.is_null() {
        return -libc::EINVAL;
    }
    (*dev).install_backend(Bluedbm::new());
    (*dev).d_submodule_exit = Some(ffi_bluedbm_device_exit);
    0
}

/// Deallocate the BlueDBM backend, mirroring C's `bluedbm_device_exit`.
///
/// # Safety
/// `dev` is `NULL` or a device whose backend is a [`Bluedbm`] from
/// [`ffi_bluedbm_device_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_bluedbm_device_exit(dev: *mut device) -> c_int {
    if dev.is_null() || (*dev).d_private.is_null() {
        return 0;
    }
    let _ = ffi_bluedbm_close(dev);
    (*dev).uninstall_backend::<Bluedbm>();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::get_bit;
    use crate::device::{
        ffi_device_module_exit, ffi_device_module_init, device_address, DeviceDirection,
        DeviceModule, Request, PADDR_EMPTY,
    };
    use core::mem::MaybeUninit;
    use core::ptr::NonNull;
    use std::sync::Mutex;

    /// The two counter globals are process-wide, so every test that touches
    /// them holds this lock and the default parallel test run stays sane.
    static GLOBALS: Mutex<()> = Mutex::new(());

    fn zeroed_device() -> device {
        unsafe { MaybeUninit::<device>::zeroed().assume_init() }
    }

    /// A backend with a non-null but never dereferenced memio handle, so the
    /// argument checks of write / read / erase can be reached without the
    /// library.
    fn backend_with_handle() -> Bluedbm {
        Bluedbm {
            mio: Cell::new(NonNull::<memio_t>::dangling().as_ptr()),
            o_flags: Cell::new(0),
        }
    }

    #[test]
    fn geometry_matches_the_bluedbm_build() {
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };

        // The Makefile's bluedbm geometry: 3/3/7/19 bits, 4096 segments.
        assert_eq!(dev.info.nr_bus, 8);
        assert_eq!(dev.info.nr_chips, 8);
        assert_eq!(dev.info.package.block.nr_pages, 128);
        assert_eq!(dev.page_size(), 8192);
        assert_eq!(dev.nr_segments(), BLUEDBM_NR_BLOCKS);
        assert_eq!(dev.blocks_per_segment(), 64);
        assert_eq!(dev.pages_per_segment(), 8192);
        assert_eq!(dev.total_size(), 256usize << 30);

        // The segment field sits at bit 13, the BlueDBM bus/chip/page run.
        assert_eq!(BLUEDBM_BLOCK_SHIFT, 13);
        let address = device_address::from(
            (2u32 & BLUEDBM_BLOCK_MASK) << BLUEDBM_BLOCK_SHIFT,
        );
        assert_eq!(
            (address.lpn >> BLUEDBM_BLOCK_SHIFT) & BLUEDBM_BLOCK_MASK,
            2
        );
    }

    #[test]
    fn write_rejects_bad_requests_in_c_order() {
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };
        let backend = backend_with_handle();
        let page_size = dev.page_size();
        let mut request = Request::new().expect("allocate a request");
        let mut page = [0u8; 1];

        unsafe {
            // NULL data pointer.
            request.data = ptr::null_mut();
            request.data_len = page_size;
            request.flag = DeviceDirection::Write.into();
            request.paddr = device_address::from(0u32);
            assert_eq!(
                backend.write(&mut dev, request.as_ptr()),
                -(libc::ENODATA as isize)
            );

            // Unassigned physical address.
            request.data = page.as_mut_ptr() as *mut c_void;
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(
                backend.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // Wrong direction.
            request.paddr = device_address::from(0u32);
            request.flag = DeviceDirection::Read.into();
            assert_eq!(
                backend.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // Short write.
            request.flag = DeviceDirection::Write.into();
            request.data_len = page_size - 1;
            assert_eq!(
                backend.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // A null hande fails before any argument check.
            let bare = Bluedbm::new();
            assert_eq!(
                bare.write(&mut dev, request.as_ptr()),
                -(libc::EFAULT as isize)
            );
        }
    }

    #[test]
    fn read_rejects_bad_requests_in_c_order() {
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };
        let backend = backend_with_handle();
        let page_size = dev.page_size();
        let mut request = Request::new().expect("allocate a request");
        let mut page = [0u8; 1];

        unsafe {
            request.data = ptr::null_mut();
            request.data_len = page_size;
            request.flag = DeviceDirection::Read.into();
            request.paddr = device_address::from(0u32);
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::ENODATA as isize)
            );

            request.data = page.as_mut_ptr() as *mut c_void;
            request.flag = DeviceDirection::Write.into();
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            request.flag = DeviceDirection::Read.into();
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            request.paddr = device_address::from(0u32);
            request.data_len = page_size - 1;
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // A null handle fails before any argument check.
            let bare = Bluedbm::new();
            assert_eq!(
                bare.read(&mut dev, request.as_ptr()),
                -(libc::EFAULT as isize)
            );
        }
    }

    #[test]
    fn erase_rejects_bad_requests_and_fails_without_a_handle() {
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };
        let backend = backend_with_handle();
        let mut request = Request::new().expect("allocate a request");

        unsafe {
            request.paddr = device_address::from(0u32);
            request.flag = DeviceDirection::Write.into();
            assert_eq!(backend.erase(&mut dev, request.as_ptr()), -libc::EINVAL);

            request.flag = DeviceDirection::Erase.into();
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(backend.erase(&mut dev, request.as_ptr()), -libc::EINVAL);

            let bare = Bluedbm::new();
            assert_eq!(bare.erase(&mut dev, request.as_ptr()), -libc::EFAULT);
            assert_eq!(bare.clear(&mut dev), -libc::EINVAL);
        }
    }

    #[test]
    fn open_storage_allocates_the_bitmap_and_the_counters() {
        let _globals = GLOBALS.lock().unwrap();
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };
        let backend = Bluedbm::new();
        let handle = NonNull::<memio_t>::dangling().as_ptr();

        unsafe {
            // No `O_CREAT`, so nothing reaches memio_trim.
            assert_eq!(backend.open_storage(&mut dev, handle, 0), 0);
            assert_eq!(backend.mio.get(), handle);
            assert!(!dev.badseg_bitmap.is_null());
            assert!(!G_ERASE_COUNTER.load(Ordering::SeqCst).is_null());
            assert!(!G_BADSEG_COUNTER.load(Ordering::SeqCst).is_null());

            // A second open releases the first buffers instead of leaking.
            let first_bitmap = dev.badseg_bitmap;
            assert_eq!(backend.open_storage(&mut dev, handle, 0), 0);
            assert_ne!(dev.badseg_bitmap, first_bitmap);

            // Clear the handle so close never hands the dangling pointer to
            // memio_close, then close releases the bitmap and the counters.
            backend.mio.set(ptr::null_mut());
            assert_eq!(backend.close(&mut dev), 0);
            assert!(dev.badseg_bitmap.is_null());
            assert!(G_ERASE_COUNTER.load(Ordering::SeqCst).is_null());
            assert!(G_BADSEG_COUNTER.load(Ordering::SeqCst).is_null());
        }
    }

    #[test]
    fn erase_end_request_counts_erases_and_bad_segments() {
        let _globals = GLOBALS.lock().unwrap();
        unsafe {
            let nr_segments = 4usize;
            free_counter(&G_ERASE_COUNTER);
            free_counter(&G_BADSEG_COUNTER);
            G_ERASE_COUNTER.store(
                libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32,
                Ordering::SeqCst,
            );
            G_BADSEG_COUNTER.store(
                libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32,
                Ordering::SeqCst,
            );

            erase_end_request(2, 0);
            erase_end_request(2, 1);
            assert_eq!(counter_get(&G_ERASE_COUNTER, 2), 2);
            assert_eq!(counter_get(&G_BADSEG_COUNTER, 2), 1);
            assert_eq!(counter_get(&G_BADSEG_COUNTER, 0), 0);

            // With no counters the callback logs and returns.
            free_counter(&G_ERASE_COUNTER);
            free_counter(&G_BADSEG_COUNTER);
            erase_end_request(0, 1);
        }
    }

    #[test]
    fn wait_erase_finish_marks_bad_and_clears_erased_segments() {
        let _globals = GLOBALS.lock().unwrap();
        let mut dev = zeroed_device();
        unsafe { configure_geometry(&mut dev) };
        let nr_segments = dev.nr_segments();

        unsafe {
            free_counter(&G_ERASE_COUNTER);
            free_counter(&G_BADSEG_COUNTER);
            G_ERASE_COUNTER.store(
                libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32,
                Ordering::SeqCst,
            );
            G_BADSEG_COUNTER.store(
                libc::calloc(nr_segments, size_of::<c_int>()) as *mut AtomicI32,
                Ordering::SeqCst,
            );
            dev.badseg_bitmap = libc::calloc(
                nr_segments.div_ceil(BITS_PER_UINT64),
                size_of::<u64>(),
            ) as *mut u64;

            // A bad segment sets its bit without polling.
            counter_inc(&G_BADSEG_COUNTER, 2);
            wait_erase_finish(&mut dev, 2, 1);
            let bitmap = slice::from_raw_parts(dev.badseg_bitmap, 1);
            assert!(get_bit(bitmap, 2));

            // A fully erased segment has its counter cleared.
            counter_set(
                &G_ERASE_COUNTER,
                3,
                dev.blocks_per_segment() as c_int,
            );
            wait_erase_finish(&mut dev, 3, 1);
            assert_eq!(counter_get(&G_ERASE_COUNTER, 3), 0);

            libc::free(dev.badseg_bitmap as *mut c_void);
            free_counter(&G_ERASE_COUNTER);
            free_counter(&G_BADSEG_COUNTER);
        }
    }

    #[test]
    fn end_rw_request_tolerates_null_and_a_null_dma() {
        unsafe {
            end_rw_request(ptr::null_mut());

            let rw_req = Box::into_raw(Box::new(AsyncBdbmReq {
                type_: REQTYPE_IO_WRITE,
                end_req: None,
                private_data: ptr::null_mut(),
            }));
            // `end_rw_request` frees the request on this path.
            end_rw_request(rw_req);
        }
    }

    #[test]
    fn init_close_exit_round_trip() {
        let _globals = GLOBALS.lock().unwrap();
        unsafe {
            assert_eq!(ffi_bluedbm_device_init(ptr::null_mut(), 0), -libc::EINVAL);
            assert_eq!(ffi_bluedbm_device_exit(ptr::null_mut()), 0);
            assert_eq!(ffi_bluedbm_close(ptr::null_mut()), -libc::EINVAL);

            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DeviceModule::Bluedbm as u64, &mut dev, 0),
                0
            );
            assert!((*dev).d_submodule_exit.is_some());
            assert!(!(*dev).d_op.is_null());
            assert!(!(*dev).d_private.is_null());

            // No geometry, no handle, nothing to free.
            assert_eq!(ffi_bluedbm_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);

            // A device with no backend installed has no close to dispatch to.
            let mut bare = zeroed_device();
            assert_eq!(ffi_bluedbm_close(&mut bare), -libc::ENODEV);
        }
    }
}
