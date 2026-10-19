//! Device abstraction layer ported from `include/device.h` + `device/device.c`.
//!
//! The C ABI is preserved: every struct is a `#[repr(C)]` mirror of its C
//! counterpart, and `device` / `device_request` keep the exact field offsets C
//! reads and writes (`dev->d_op`, `dev->info`, `dev->badseg_bitmap`,
//! `dev->d_private`, `dev->d_submodule_exit`, `request->mutex`,
//! `request->cond`, ...). Those offsets are the contract, pinned on both sides
//! by `layout_matches_c` here and the `DEVICE_STATIC_ASSERT`s in
//! `include/device.h`.
//!
//! `device_operations` stays the C-visible vtable that a backend installs into
//! `dev->d_op`. Rust backends implement [`DeviceOperations`] and
//! [`device::install_backend`] stores the backend plus the vtable built from it
//! in one slot, so C keeps dispatching through `dev->d_op` unchanged and the
//! vtable can never disagree with the type behind it. The C-named structs keep
//! their C names (a `#[repr(C)]` type is not subject to
//! `non_camel_case_types`). Type aliases are camel-cased as in `src/lru.rs`.
//!
//! Two layers live here. The C ABI layer (the `#[repr(C)]` mirrors, the `ffi_*`
//! entry points and the vtable) is what C calls. The native
//! layer ([`device_address`] helpers, [`DeviceDirection`], [`Request`] and the
//! accessors on [`device_request`] and [`device`]) is what Rust callers use, so
//! the FTL port (Tasks 8-13) does not hand-roll raw pointers, `errno` or
//! atomics.
//!
//! Symbols carry the `ffi_` prefix because they are the C ABI this module
//! exports. The C original (`device/device.c`) went with the last C source in
//! the cutover, so this module is the only implementation: `SUBMODULE_INIT`
//! installs every backend, and `ffi_device_module_init` /
//! `ffi_device_module_exit` are the live pair every caller uses.
//!
//! Deliberate deviations from the C original, which is deleted, so the bullets
//! below describe what it did:
//!
//! - `ffi_device_module_init` rejects an out-of-range `modnum` with `-EINVAL`
//!   and a module with no backend with `-ENODEV`. C indexes
//!   `submodule_init[modnum]` unchecked and calls a `NULL` entry, so both are
//!   wild calls there.
//! - `ffi_device_module_init` and `ffi_device_free_request` accept `NULL`
//!   (`-EINVAL` / no-op). C dereferences it, and `device_module_exit` asserts.
//! - A failing `pthread_mutex_init` / `pthread_cond_init` releases the
//!   partially built structure. C leaks the request (mutex failure) or the
//!   mutex (cond failure), and ignores the mutex failure in
//!   `device_module_init` entirely.
//! - `ffi_device_module_init` cannot fail with `-ENOMEM`: `Box` aborts on OOM,
//!   as in `src/list.rs`.
//! - Geometry lives on [`device`] as safe methods instead of the header's
//!   `static inline`s. The C inlines in `include/device.h` stay, so no C-ABI
//!   symbol is exported for them.
//! - The bit counts and `DEVICE_PAGE_SIZE` mirror the Makefile's
//!   `DEVICE_INFO`. The non-default backends compile C with different
//!   `-DDEVICE_NR_*_BITS`, and the backend that has landed selects the same
//!   counts behind its cargo feature: the zoned build's `3/3/5/21` behind
//!   `zone`. The raspberry build's `1/1/4/24` with a 2048 byte page lands with
//!   its hardware port, and the bluedbm build's `3/3/7/19` with its own.
//! - C's `raspberry` address view is not ported: it only means something under
//!   the raspberry build's bit counts, and the Pi NAND path is out of scope, so
//!   it lands with the raspberry hardware port instead of carrying a constant
//!   that is wrong for its own target. See [`device_address`].
//! - `ffi_device_module_init` dispatches through a backend table where the
//!   ramdisk entry is wired (Task 6), the zone entry carries the ported zoned
//!   backend (#108) and the two remaining slots are wired behind their Cargo
//!   features (Task 7). C's table indexed the backend pointers unchecked, and
//!   the C entry points that read it are deleted.

pub mod ramdisk;

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::slice;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicI32, Ordering};

use libc::{pthread_cond_t, pthread_mutex_t};

use crate::pr_err;

/// C's `PADDR_EMPTY`: the "physical address not assigned" sentinel.
pub const PADDR_EMPTY: u32 = u32::MAX;

/// C's `DEVICE_DEFAULT_REQUEST`: the only request allocation flag defined.
pub const DEVICE_DEFAULT_REQUEST: u64 = 0;

/// `DEVICE_NR_BUS_BITS` / `DEVICE_NR_CHIPS_BITS` / `DEVICE_NR_PAGES_BITS` /
/// `DEVICE_NR_BLOCKS_BITS`, mirroring the Makefile's `DEVICE_INFO`.
///
/// The zoned build compiles C with `-DDEVICE_NR_*_BITS=3/3/5/21`, and
/// `USE_ZONE_DEVICE=1` also turns on the `zone` cargo feature, so the same
/// feature selects those counts here and both sides decode a
/// [`device_address`] the same way. `DEVICE_PAGE_SIZE` is the same in both
/// builds. The zoned and the default counts put the block field at bit 11 and
/// give a segment 2048 pages, so only the split of the low 11 bits differs.
#[cfg(feature = "zone")]
pub const DEVICE_NR_BUS_BITS: u32 = 3;
#[cfg(not(feature = "zone"))]
pub const DEVICE_NR_BUS_BITS: u32 = 2;

/// See [`DEVICE_NR_BUS_BITS`].
#[cfg(feature = "zone")]
pub const DEVICE_NR_CHIPS_BITS: u32 = 3;
#[cfg(not(feature = "zone"))]
pub const DEVICE_NR_CHIPS_BITS: u32 = 2;

/// See [`DEVICE_NR_BUS_BITS`].
#[cfg(feature = "zone")]
pub const DEVICE_NR_PAGES_BITS: u32 = 5;
#[cfg(not(feature = "zone"))]
pub const DEVICE_NR_PAGES_BITS: u32 = 7;

/// See [`DEVICE_NR_BUS_BITS`].
#[cfg(feature = "zone")]
pub const DEVICE_NR_BLOCKS_BITS: u32 = 21;
#[cfg(not(feature = "zone"))]
pub const DEVICE_NR_BLOCKS_BITS: u32 = 19;

/// C's `DEVICE_PAGE_SIZE`, the NAND page size of the default build and of the
/// zoned build, which does not override it.
pub const DEVICE_PAGE_SIZE: usize = 8192;

const BUS_SHIFT: u32 = 0;
const CHIP_SHIFT: u32 = DEVICE_NR_BUS_BITS;
const PAGE_SHIFT: u32 = DEVICE_NR_BUS_BITS + DEVICE_NR_CHIPS_BITS;
const BLOCK_SHIFT: u32 = PAGE_SHIFT + DEVICE_NR_PAGES_BITS;

const BUS_MASK: u32 = (1u32 << DEVICE_NR_BUS_BITS) - 1;
const CHIP_MASK: u32 = (1u32 << DEVICE_NR_CHIPS_BITS) - 1;
const PAGE_MASK: u32 = (1u32 << DEVICE_NR_PAGES_BITS) - 1;
const BLOCK_MASK: u32 = (1u32 << DEVICE_NR_BLOCKS_BITS) - 1;

/// C's `enum { RAMDISK_MODULE = 0, ... }`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceModule {
    Ramdisk = 0,
    Bluedbm = 1,
    Zone = 2,
    Raspberry = 3,
}

/// Number of entries in C's `submodule_init[]`, one per [`DeviceModule`].
pub const DEVICE_MODULE_COUNT: usize = 4;

impl DeviceModule {
    /// Map the `modnum` the C API passes to a module, or `None` when it is out
    /// of range.
    pub const fn from_modnum(modnum: u64) -> Option<Self> {
        match modnum {
            0 => Some(DeviceModule::Ramdisk),
            1 => Some(DeviceModule::Bluedbm),
            2 => Some(DeviceModule::Zone),
            3 => Some(DeviceModule::Raspberry),
            _ => None,
        }
    }
}

/// C's `enum { DEVICE_WRITE = 0, DEVICE_READ, DEVICE_ERASE }`, the value of
/// [`device_request::flag`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceDirection {
    Write = 0,
    Read = 1,
    Erase = 2,
}

impl DeviceDirection {
    /// Map the `flag` word of a request to a direction, or `None` for a value C
    /// does not define.
    pub const fn from_flag(flag: c_uint) -> Option<Self> {
        match flag {
            0 => Some(DeviceDirection::Write),
            1 => Some(DeviceDirection::Read),
            2 => Some(DeviceDirection::Erase),
            _ => None,
        }
    }
}

impl From<DeviceDirection> for c_uint {
    fn from(direction: DeviceDirection) -> Self {
        direction as c_uint
    }
}

/// Mirrors `struct device_address` in `include/device.h`.
///
/// C declares it as a union of bitfield views over one `uint32_t`. Rust keeps
/// the `lpn` word and reproduces the views with shifts, so the bit order is
/// explicit instead of compiler-defined. C's bitfield order on a little-endian
/// target puts the first declared field in the least significant bits, which
/// is what the shifts below encode.
///
/// C's third view, `raspberry`, folds the whole bus/chip/page run into one
/// `page` field for the Pi NAND API (`device/raspberry/raspberry.c` reads
/// `addr.raspberry.page` and `.block`). It is only meaningful under the
/// raspberry build's bit counts, which this port does not express, so it lands
/// with the raspberry hardware port (the Task 7 work shipped stubs only, so
/// nothing landed there yet). C's `raspberry_converter` view is a
/// field-by-field copy of `format` and needs no helper here.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct device_address {
    pub lpn: u32,
}

impl device_address {
    /// Pack a physical address the way C's `format` view does.
    pub const fn new(bus: u32, chip: u32, block: u32, page: u32) -> Self {
        Self {
            lpn: ((bus & BUS_MASK) << BUS_SHIFT)
                | ((chip & CHIP_MASK) << CHIP_SHIFT)
                | ((page & PAGE_MASK) << PAGE_SHIFT)
                | ((block & BLOCK_MASK) << BLOCK_SHIFT),
        }
    }

    /// Pack a segment-local page index the way C's `paddr.format.block =
    /// (uint16_t)segnum; paddr.lpn |= page;` does.
    ///
    /// `page` is a linear index across the low (bus + chip + page) bits, not a
    /// page within a block, so this is not `new(0, 0, segment, page)`. The
    /// segment number is 16 bits because C's cast truncates it to 16.
    pub const fn from_segment_page(segment: u16, page: u32) -> Self {
        Self {
            lpn: ((segment as u32) << BLOCK_SHIFT) | page,
        }
    }

    /// Bus (channel) of this address.
    pub const fn bus(&self) -> u32 {
        (self.lpn >> BUS_SHIFT) & BUS_MASK
    }

    /// Chip (way) of this address.
    pub const fn chip(&self) -> u32 {
        (self.lpn >> CHIP_SHIFT) & CHIP_MASK
    }

    /// Page within the block.
    pub const fn page(&self) -> u32 {
        (self.lpn >> PAGE_SHIFT) & PAGE_MASK
    }

    /// Block (segment) of this address.
    pub const fn block(&self) -> u32 {
        (self.lpn >> BLOCK_SHIFT) & BLOCK_MASK
    }

    /// Page index inside the segment, the inverse of
    /// [`Self::from_segment_page`].
    ///
    /// The low (bus + chip + page) bits are the segment-local page number that
    /// `paddr.lpn |= page` builds in C's `page_ftl_get_free_page` and that a
    /// segment's `use_bits` bitmap indexes.
    pub const fn segment_page(&self) -> u32 {
        self.lpn & ((1 << BLOCK_SHIFT) - 1)
    }
}

impl From<u32> for device_address {
    fn from(lpn: u32) -> Self {
        Self { lpn }
    }
}

impl From<device_address> for u32 {
    fn from(address: device_address) -> Self {
        address.lpn
    }
}

impl fmt::Display for device_address {
    /// Print the decoded fields, which is what logs and test failures want.
    /// `Debug` keeps showing the raw word.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "bus {} chip {} block {} page {} (lpn {:#x})",
            self.bus(),
            self.chip(),
            self.block(),
            self.page(),
            self.lpn
        )
    }
}

/// Callback run when the device finishes a request. Mirrors
/// `device_end_req_fn` in `include/device.h` (renamed for Rust's
/// `non_camel_case_types`. The ABI is the same nullable function pointer).
pub type DeviceEndReqFn = Option<unsafe extern "C" fn(*mut device_request)>;

/// Mirrors `struct device_request` in `include/device.h`.
///
/// `mutex` / `cond` guard `is_finish`. The FTL allocates a request per I/O and
/// waits on `cond` until the backend's `end_rq` callback signals it. C reads
/// `flag`, `paddr`, `data`, `end_rq` and locks `mutex` directly, so every field
/// keeps its C offset (see `layout_matches_c`).
#[repr(C)]
pub struct device_request {
    pub flag: c_uint,
    pub data_len: usize,
    pub sector: usize,
    pub paddr: device_address,
    pub data: *mut c_void,
    pub end_rq: DeviceEndReqFn,
    pub is_finish: c_int,
    pub mutex: pthread_mutex_t,
    pub cond: pthread_cond_t,
    pub rq_private: *mut c_void,
}

impl device_request {
    /// Direction of this request, or `None` for a value C does not define.
    /// A zeroed request reads as `Write`, because C's `DEVICE_WRITE` is 0.
    pub fn direction(&self) -> Option<DeviceDirection> {
        DeviceDirection::from_flag(self.flag)
    }

    /// `is_finish` as an atomic.
    ///
    /// C touches this word with `__atomic_*` and the allocator uses the same
    /// `AtomicI32`, so no plain access can race with it.
    fn finished_flag(&self) -> &AtomicI32 {
        unsafe { AtomicI32::from_ptr(ptr::addr_of!(self.is_finish) as *mut c_int) }
    }

    /// Whether the backend finished this request.
    ///
    /// Uses a sequentially consistent load, the read side of C's `__atomic_*`
    /// in `page_ftl_read_end_rq`, so a waiting thread can poll it.
    pub fn is_finish(&self) -> bool {
        self.finished_flag().load(Ordering::SeqCst) != 0
    }

    /// Mark the request finished, the store side of C's `end_rq` callbacks.
    pub fn mark_finished(&self) {
        self.finished_flag().store(1, Ordering::SeqCst);
    }

    /// View `data` as the payload buffer of `data_len` bytes, or `None` when C
    /// passed a `NULL` data pointer.
    ///
    /// # Safety
    /// `data` points at `data_len` writable bytes that stay valid for as long
    /// as the returned slice is used.
    pub unsafe fn buffer_mut(&mut self) -> Option<&mut [u8]> {
        if self.data.is_null() {
            return None;
        }
        Some(slice::from_raw_parts_mut(self.data as *mut u8, self.data_len))
    }
}

/// Mirrors `struct device_page`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct device_page {
    pub size: usize,
}

/// Mirrors `struct device_block`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct device_block {
    pub page: device_page,
    pub nr_pages: usize,
}

/// Mirrors `struct device_package`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct device_package {
    pub block: device_block,
    pub nr_blocks: usize,
}

/// Mirrors `struct device_info`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct device_info {
    pub package: device_package,
    pub nr_bus: usize,
    pub nr_chips: usize,
}

/// Mirrors `struct device`.
///
/// A backend fills `d_op`, `d_private` and `d_submodule_exit` in its init
/// routine. The geometry lives in `info` and is filled by the backend's `open`.
#[repr(C)]
pub struct device {
    pub mutex: pthread_mutex_t,
    pub d_op: *const device_operations,
    pub info: device_info,
    pub badseg_bitmap: *mut u64,
    pub d_private: *mut c_void,
    pub d_submodule_exit: Option<unsafe extern "C" fn(*mut device) -> c_int>,
}

impl device {
    /// Number of segments in the flash board (C's `device_get_nr_segments`).
    pub fn nr_segments(&self) -> usize {
        self.info.package.nr_blocks
    }

    /// Number of blocks in a segment (C's `device_get_blocks_per_segment`).
    pub fn blocks_per_segment(&self) -> usize {
        self.info.nr_bus * self.info.nr_chips
    }

    /// Number of pages in a segment (C's `device_get_pages_per_segment`).
    pub fn pages_per_segment(&self) -> usize {
        self.blocks_per_segment() * self.info.package.block.nr_pages
    }

    /// NAND page size in bytes (C's `device_get_page_size`).
    pub fn page_size(&self) -> usize {
        self.info.package.block.page.size
    }

    /// Total size of the flash board in bytes (C's `device_get_total_size`).
    pub fn total_size(&self) -> usize {
        self.nr_segments() * self.pages_per_segment() * self.page_size()
    }

    /// Total number of pages in the flash board (C's `device_get_total_pages`).
    ///
    /// Panics when the page size is zero. C divides by zero there.
    pub fn total_pages(&self) -> usize {
        self.total_size() / self.page_size()
    }
}

/// Mirrors `struct device_operations`, the vtable a backend installs into
/// [`device::d_op`]. C calls these five pointers directly.
#[repr(C)]
pub struct device_operations {
    pub open: Option<unsafe extern "C" fn(*mut device, *const c_char, c_int) -> c_int>,
    pub write: Option<unsafe extern "C" fn(*mut device, *mut device_request) -> isize>,
    pub read: Option<unsafe extern "C" fn(*mut device, *mut device_request) -> isize>,
    pub erase: Option<unsafe extern "C" fn(*mut device, *mut device_request) -> c_int>,
    pub close: Option<unsafe extern "C" fn(*mut device) -> c_int>,
}

/// Rust-side device backend interface, the trait-based form of the
/// `device_operations` vtable.
///
/// [`device::install_backend`] stores the backend and the vtable built from it
/// in one slot, and points [`device::d_op`] at that vtable. C calling
/// `dev->d_op->read(...)` therefore reaches this implementation, and the type
/// the shims recover can never disagree with the vtable they are called
/// through.
///
/// # Safety
///
/// Every method is called by C with the device the backend was installed into.
/// Implementations must tolerate being called from any thread and must not
/// move themselves.
pub trait DeviceOperations {
    /// Open the backend (C's `device_operations.open`).
    ///
    /// # Safety
    /// `dev` is the device this backend was initialized into.
    unsafe fn open(&self, dev: *mut device, name: *const c_char, flags: c_int) -> c_int;

    /// Write a request (C's `device_operations.write`).
    ///
    /// # Safety
    /// `dev` is the device this backend was initialized into and `request` is
    /// a live request.
    unsafe fn write(&self, dev: *mut device, request: *mut device_request) -> isize;

    /// Read a request (C's `device_operations.read`).
    ///
    /// # Safety
    /// `dev` is the device this backend was initialized into and `request` is
    /// a live request.
    unsafe fn read(&self, dev: *mut device, request: *mut device_request) -> isize;

    /// Erase a request (C's `device_operations.erase`).
    ///
    /// # Safety
    /// `dev` is the device this backend was initialized into and `request` is
    /// a live request.
    unsafe fn erase(&self, dev: *mut device, request: *mut device_request) -> c_int;

    /// Close the backend (C's `device_operations.close`).
    ///
    /// # Safety
    /// `dev` is the device this backend was initialized into.
    unsafe fn close(&self, dev: *mut device) -> c_int;
}

/// A Rust backend together with the C-visible vtable built from it.
///
/// [`device::d_private`] points at the slot, so the vtable [`device::d_op`]
/// points at and the type the shims recover are built from the same `B` and
/// cannot disagree.
struct BackendSlot<B: DeviceOperations> {
    ops: device_operations,
    backend: B,
}

unsafe extern "C" fn shim_open<B: DeviceOperations>(
    dev: *mut device,
    name: *const c_char,
    flags: c_int,
) -> c_int {
    let slot = &*((*dev).d_private as *const BackendSlot<B>);
    slot.backend.open(dev, name, flags)
}

unsafe extern "C" fn shim_write<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    let slot = &*((*dev).d_private as *const BackendSlot<B>);
    slot.backend.write(dev, request)
}

unsafe extern "C" fn shim_read<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    let slot = &*((*dev).d_private as *const BackendSlot<B>);
    slot.backend.read(dev, request)
}

unsafe extern "C" fn shim_erase<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> c_int {
    let slot = &*((*dev).d_private as *const BackendSlot<B>);
    slot.backend.erase(dev, request)
}

unsafe extern "C" fn shim_close<B: DeviceOperations>(dev: *mut device) -> c_int {
    let slot = &*((*dev).d_private as *const BackendSlot<B>);
    slot.backend.close(dev)
}

impl device_operations {
    /// Build the C-visible vtable from a Rust backend.
    fn from_backend<B: DeviceOperations>() -> Self {
        Self {
            open: Some(shim_open::<B>),
            write: Some(shim_write::<B>),
            read: Some(shim_read::<B>),
            erase: Some(shim_erase::<B>),
            close: Some(shim_close::<B>),
        }
    }
}

impl device {
    /// Install a Rust backend on this device: store it and point
    /// [`device::d_op`] at the vtable built from it.
    ///
    /// This replaces C's per-backend `static` vtable plus the `d_private`
    /// bookkeeping that had to agree with it.
    ///
    /// # Safety
    /// `self` is a live device with no backend installed yet.
    pub unsafe fn install_backend<B: DeviceOperations>(&mut self, backend: B) {
        let slot = Box::into_raw(Box::new(BackendSlot {
            ops: device_operations::from_backend::<B>(),
            backend,
        }));
        self.d_op = ptr::addr_of!((*slot).ops);
        self.d_private = slot as *mut c_void;
    }

    /// Release the backend installed by [`device::install_backend`] and clear
    /// `d_private` / `d_op`. A `NULL` `d_private` is a no-op.
    ///
    /// # Safety
    /// `self` had `install_backend::<B>` called on it with the same `B`, and no
    /// operation is in flight.
    pub unsafe fn uninstall_backend<B: DeviceOperations>(&mut self) {
        if self.d_private.is_null() {
            return;
        }
        drop(Box::from_raw(self.d_private as *mut BackendSlot<B>));
        self.d_private = ptr::null_mut();
        self.d_op = ptr::null();
    }
}

/// Init routine of a device backend, C's `submodule_init[]` element type.
pub type SubmoduleInitFn = unsafe extern "C" fn(*mut device, u64) -> c_int;

#[cfg(feature = "bluedbm")]
mod bluedbm;
#[cfg(feature = "raspberry")]
mod raspberry;
#[cfg(feature = "zone")]
mod zone;

#[cfg(feature = "bluedbm")]
const BLUEDBM_INIT: Option<SubmoduleInitFn> = Some(bluedbm::ffi_bluedbm_device_init);
#[cfg(not(feature = "bluedbm"))]
const BLUEDBM_INIT: Option<SubmoduleInitFn> = None;

#[cfg(feature = "zone")]
const ZONE_INIT: Option<SubmoduleInitFn> = Some(zone::ffi_zone_device_init);
#[cfg(not(feature = "zone"))]
const ZONE_INIT: Option<SubmoduleInitFn> = None;

#[cfg(feature = "raspberry")]
const RASPBERRY_INIT: Option<SubmoduleInitFn> = Some(raspberry::ffi_raspberry_device_init);
#[cfg(not(feature = "raspberry"))]
const RASPBERRY_INIT: Option<SubmoduleInitFn> = None;

/// C's `submodule_init[]`, indexed by [`DeviceModule`].
///
/// Task 6 wired the ramdisk entry, #108 replaced the zone stub with the ported
/// zoned backend, and Task 7 wired the bluedbm and raspberry stub slots. Each
/// non-default entry is enabled by its Cargo feature. It is the only backend
/// table now that the C one is deleted.
static SUBMODULE_INIT: [Option<SubmoduleInitFn>; DEVICE_MODULE_COUNT] = [
    Some(ramdisk::ffi_ramdisk_device_init),
    BLUEDBM_INIT,
    ZONE_INIT,
    RASPBERRY_INIT,
];

/// Set `errno`, the way C's failure paths do before returning a negative value.
///
/// C stores the pthread return code in `errno` so callers can read it after
/// `NULL`. `libc` exposes the location per target.
#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn set_errno(value: c_int) {
    *libc::__errno_location() = value;
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe fn set_errno(value: c_int) {
    *libc::__error() = value;
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
unsafe fn set_errno(_value: c_int) {}

/// Allocate a device request, mirroring C's `device_alloc_request`.
///
/// `flags` is ignored like C does. [`DEVICE_DEFAULT_REQUEST`] is the only flag
/// defined. Returns `NULL` and sets `errno` when the request's synchronization
/// primitives cannot be initialized.
///
/// # Safety
/// The returned pointer must be released with [`ffi_device_free_request`].
#[no_mangle]
pub unsafe extern "C" fn ffi_device_alloc_request(flags: u64) -> *mut device_request {
    let _ = flags;

    let mut request: Box<device_request> =
        Box::new(MaybeUninit::<device_request>::zeroed().assume_init());

    let ret = libc::pthread_mutex_init(&mut request.mutex, ptr::null());
    if ret != 0 {
        pr_err!("pthread mutex initialize failed");
        set_errno(ret);
        return ptr::null_mut();
    }

    let ret = libc::pthread_cond_init(&mut request.cond, ptr::null());
    if ret != 0 {
        pr_err!("pthread conditional variable initialize failed");
        // C leaks the request and the mutex here.
        let _ = libc::pthread_mutex_destroy(&mut request.mutex);
        set_errno(ret);
        return ptr::null_mut();
    }

    AtomicI32::from_ptr(ptr::addr_of_mut!(request.is_finish)).store(0, Ordering::SeqCst);

    Box::into_raw(request)
}

/// Release a request allocated by [`ffi_device_alloc_request`], mirroring C's
/// `device_free_request`. A `NULL` request is a no-op (C dereferences it).
///
/// # Safety
/// `request` is `NULL` or came from [`ffi_device_alloc_request`] and was not
/// released yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_device_free_request(request: *mut device_request) {
    if request.is_null() {
        return;
    }

    let mut request = Box::from_raw(request);
    let _ = libc::pthread_cond_destroy(&mut request.cond);
    let _ = libc::pthread_mutex_destroy(&mut request.mutex);
}

/// RAII owner of a request allocated by [`ffi_device_alloc_request`].
///
/// Native callers get the request's lifetime tied to this value instead of
/// calling [`ffi_device_free_request`] by hand. It is deliberately neither
/// `Send` nor `Sync`: the raw pointer is handed to a backend, whose `end_rq`
/// callback runs on the backend's thread.
pub struct Request {
    ptr: *mut device_request,
}

impl Request {
    /// Allocate a request, mirroring C's `device_alloc_request` with an
    /// `io::Error` built from `errno` instead of a bare `NULL`.
    pub fn new() -> io::Result<Self> {
        let ptr = unsafe { ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST) };
        if ptr.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr })
    }

    /// The raw request pointer, for handing to C or to a backend.
    pub fn as_ptr(&self) -> *mut device_request {
        self.ptr
    }
}

impl Deref for Request {
    type Target = device_request;

    fn deref(&self) -> &device_request {
        unsafe { &*self.ptr }
    }
}

impl DerefMut for Request {
    fn deref_mut(&mut self) -> &mut device_request {
        unsafe { &mut *self.ptr }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        unsafe { ffi_device_free_request(self.ptr) }
    }
}

/// Tear a device down: run the backend's exit routine, destroy the mutex and
/// release the structure. Mirrors the body of C's `device_module_exit`.
///
/// # Safety
/// `dev` is `NULL` or a live device.
unsafe fn destroy_device(dev: *mut device) -> c_int {
    if dev.is_null() {
        return 0;
    }

    if let Some(submodule_exit) = (*dev).d_submodule_exit {
        submodule_exit(dev);
        (*dev).d_submodule_exit = None;
    }
    let _ = libc::pthread_mutex_destroy(&mut (*dev).mutex);
    drop(Box::from_raw(dev));
    0
}

/// Dispatch through `table`, the body of [`ffi_device_module_init`].
///
/// The table is a parameter so the dispatch can be exercised against a mock
/// backend while the real table is still empty (see the module docs).
///
/// # Safety
/// `out` is `NULL` or a writable pointer. The table's entries follow the
/// backend init contract of C's `submodule_init[]`.
unsafe fn module_init_with(
    table: &[Option<SubmoduleInitFn>],
    modnum: u64,
    out: *mut *mut device,
    flags: u64,
) -> c_int {
    if out.is_null() {
        pr_err!("device output pointer is NULL");
        return -libc::EINVAL;
    }

    let Some(module) = DeviceModule::from_modnum(modnum) else {
        // C indexes submodule_init[modnum] unchecked.
        pr_err!("unknown device module (modnum: {})", modnum);
        return -libc::EINVAL;
    };

    let Some(submodule_init) = table.get(module as usize).copied().flatten() else {
        // Also covers a backend list shorter than the module list, the shape
        // Tasks 6/7 build up entry by entry.
        pr_err!("no backend for this device module (modnum: {})", modnum);
        return -libc::ENODEV;
    };

    let mut dev: Box<device> = Box::new(MaybeUninit::<device>::zeroed().assume_init());

    let ret = libc::pthread_mutex_init(&mut dev.mutex, ptr::null());
    if ret != 0 {
        // C ignores this failure and keeps an uninitialized mutex.
        pr_err!("pthread mutex initialize failed");
        set_errno(ret);
        return -ret;
    }

    let ret = submodule_init(&mut *dev, flags);
    if ret != 0 {
        pr_err!("initialize the submodule failed (modnum: {})", modnum);
        destroy_device(&mut *dev);
        return ret;
    }

    dev.badseg_bitmap = ptr::null_mut();
    *out = Box::into_raw(dev);
    0
}

/// Initialize a device module, mirroring C's `device_module_init`.
///
/// Returns `0` on success and a negative value on failure. On failure `*dev` is
/// left untouched.
///
/// # Safety
/// `dev` is `NULL` or points to a writable `*mut device`.
#[no_mangle]
pub unsafe extern "C" fn ffi_device_module_init(
    modnum: u64,
    dev: *mut *mut device,
    flags: u64,
) -> c_int {
    module_init_with(&SUBMODULE_INIT, modnum, dev, flags)
}

/// Deallocate a device module, mirroring C's `device_module_exit`.
///
/// # Safety
/// `dev` is `NULL` or a live device from [`ffi_device_module_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_device_module_exit(dev: *mut device) -> c_int {
    destroy_device(dev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    /// Width of a pointer, spelled once for the layout assertions.
    const PTR: usize = size_of::<*const c_void>();

    fn zeroed_device() -> device {
        unsafe { MaybeUninit::<device>::zeroed().assume_init() }
    }

    fn zeroed_request() -> device_request {
        unsafe { MaybeUninit::<device_request>::zeroed().assume_init() }
    }

    #[test]
    fn address_packing_matches_c_bitfields() {
        // The default build is bus 2 bits, chip 2 bits, page 7 bits, block 19
        // bits, and the zoned build is 3/3/5/21. Both put the block at bit 11.
        let addr = device_address::new(1, 2, 3, 4);
        assert_eq!(
            addr.lpn,
            1 | (2 << DEVICE_NR_BUS_BITS)
                | (4 << (DEVICE_NR_BUS_BITS + DEVICE_NR_CHIPS_BITS))
                | (3 << BLOCK_SHIFT)
        );
        #[cfg(not(feature = "zone"))]
        assert_eq!(addr.lpn, 6217);
        #[cfg(feature = "zone")]
        assert_eq!(addr.lpn, 6417);
        assert_eq!(addr.bus(), 1);
        assert_eq!(addr.chip(), 2);
        assert_eq!(addr.page(), 4);
        assert_eq!(addr.block(), 3);

        // Every bit of the packed word is addressable, and the fields truncate
        // to their declared width like C's bitfields do.
        let full = device_address::new(BUS_MASK, CHIP_MASK, BLOCK_MASK, PAGE_MASK);
        assert_eq!(full.block(), BLOCK_MASK);

        // The default geometry packs 30 of the 32 bits, so the "empty"
        // sentinel is not a packable address. The zoned geometry packs all 32,
        // so there the sentinel is a reachable address, which is C's behaviour
        // under `-DDEVICE_NR_*_BITS=3/3/5/21` too.
        #[cfg(not(feature = "zone"))]
        {
            assert_eq!(full.lpn, (1u32 << 30) - 1);
            assert_ne!(PADDR_EMPTY, full.lpn);
        }
        #[cfg(feature = "zone")]
        {
            assert_eq!(full.lpn, u32::MAX);
            assert_eq!(full.lpn, PADDR_EMPTY);
        }
        assert_eq!(device_address { lpn: PADDR_EMPTY }.lpn, u32::MAX);

        assert_eq!(device_address::default().lpn, 0);

        // `from_segment_page` places the segment at `BLOCK_SHIFT` and ORs a
        // linear page index below it, which is what C's `format.block` plus
        // `|= page` yields. It is not `new(0, 0, segment, page)`.
        assert_eq!(
            device_address::from_segment_page(1, 0),
            device_address::new(0, 0, 1, 0)
        );
        assert_eq!(device_address::from_segment_page(0, 2).lpn, 2);
        assert_eq!(device_address::from_segment_page(2, 5).lpn, (2 << 11) | 5);
        // The segment keeps 16 bits, the width C's cast truncates to.
        assert_eq!(
            device_address::from_segment_page(u16::MAX, 0).block(),
            u16::MAX as u32
        );
    }

    #[test]
    fn geometry_helpers_match_c() {
        let mut dev = zeroed_device();
        dev.info.nr_bus = 1 << DEVICE_NR_BUS_BITS;
        dev.info.nr_chips = 1 << DEVICE_NR_CHIPS_BITS;
        dev.info.package.nr_blocks = 64;
        dev.info.package.block.nr_pages = 1 << DEVICE_NR_PAGES_BITS;
        dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

        assert_eq!(dev.nr_segments(), 64);
        assert_eq!(
            dev.blocks_per_segment(),
            1 << (DEVICE_NR_BUS_BITS + DEVICE_NR_CHIPS_BITS)
        );
        // Both geometries give a segment 2^11 pages: 4 * 4 * 128 in the default
        // build, 8 * 8 * 32 in the zoned one.
        assert_eq!(dev.pages_per_segment(), 1 << 11);
        assert_eq!(dev.page_size(), DEVICE_PAGE_SIZE);
        // The Makefile's default build is the 1GiB ramdisk, and the zoned build
        // reaches the same 1GiB with 64 segments of 2048 pages.
        assert_eq!(dev.total_size(), 1 << 30);
        assert_eq!(dev.total_pages(), (1 << 30) / DEVICE_PAGE_SIZE);
    }

    #[test]
    fn layout_matches_c() {
        // struct device: mutex, d_op, info (5 words), badseg_bitmap, d_private,
        // d_submodule_exit.
        assert_eq!(offset_of!(device, mutex), 0);
        assert_eq!(
            offset_of!(device, d_op),
            size_of::<pthread_mutex_t>().next_multiple_of(PTR)
        );
        assert_eq!(offset_of!(device, info), offset_of!(device, d_op) + PTR);
        assert_eq!(
            offset_of!(device, badseg_bitmap),
            offset_of!(device, info) + 5 * size_of::<usize>()
        );
        assert_eq!(
            offset_of!(device, d_private),
            offset_of!(device, badseg_bitmap) + PTR
        );
        assert_eq!(
            offset_of!(device, d_submodule_exit),
            offset_of!(device, d_private) + PTR
        );
        assert_eq!(
            size_of::<device>(),
            offset_of!(device, d_submodule_exit) + PTR
        );

        // struct device_request: flag, data_len, sector, paddr, data, end_rq,
        // is_finish, mutex, cond, rq_private.
        assert_eq!(offset_of!(device_request, flag), 0);
        assert_eq!(
            offset_of!(device_request, data_len),
            size_of::<c_uint>().next_multiple_of(size_of::<usize>())
        );
        assert_eq!(
            offset_of!(device_request, sector),
            offset_of!(device_request, data_len) + size_of::<usize>()
        );
        assert_eq!(
            offset_of!(device_request, paddr),
            offset_of!(device_request, sector) + size_of::<usize>()
        );
        assert_eq!(
            offset_of!(device_request, data),
            (offset_of!(device_request, paddr) + size_of::<device_address>())
                .next_multiple_of(PTR)
        );
        assert_eq!(
            offset_of!(device_request, end_rq),
            offset_of!(device_request, data) + PTR
        );
        assert_eq!(
            offset_of!(device_request, is_finish),
            offset_of!(device_request, end_rq) + PTR
        );
        assert_eq!(
            offset_of!(device_request, mutex),
            (offset_of!(device_request, is_finish) + size_of::<c_int>())
                .next_multiple_of(align_of::<pthread_mutex_t>())
        );
        assert_eq!(
            offset_of!(device_request, cond),
            offset_of!(device_request, mutex) + size_of::<pthread_mutex_t>()
        );
        assert_eq!(
            offset_of!(device_request, rq_private),
            (offset_of!(device_request, cond) + size_of::<pthread_cond_t>())
                .next_multiple_of(PTR)
        );
        assert_eq!(
            size_of::<device_request>(),
            offset_of!(device_request, rq_private) + PTR
        );

        // struct device_address is C's bitfield union seen as one uint32_t.
        assert_eq!(size_of::<device_address>(), size_of::<u32>());

        // The geometry structs are plain size_t chains.
        assert_eq!(size_of::<device_page>(), size_of::<usize>());
        assert_eq!(size_of::<device_block>(), 2 * size_of::<usize>());
        assert_eq!(size_of::<device_package>(), 3 * size_of::<usize>());
        assert_eq!(size_of::<device_info>(), 5 * size_of::<usize>());
        assert_eq!(offset_of!(device_info, nr_bus), 3 * size_of::<usize>());

        // The vtable is five function pointers.
        assert_eq!(size_of::<device_operations>(), 5 * PTR);
    }

    #[test]
    fn request_alloc_and_free_mirror_c() {
        unsafe {
            let request = ffi_device_alloc_request(DEVICE_DEFAULT_REQUEST);
            assert!(!request.is_null());
            assert_eq!((*request).is_finish, 0);
            assert!((*request).data.is_null());
            assert_eq!((*request).flag, 0);

            // The mutex C locks around is_finish is a working pthread mutex.
            assert_eq!(libc::pthread_mutex_lock(&mut (*request).mutex), 0);
            assert_eq!(libc::pthread_mutex_unlock(&mut (*request).mutex), 0);

            ffi_device_free_request(request);
            ffi_device_free_request(ptr::null_mut());
        }
    }

    #[test]
    fn native_request_surface() {
        unsafe {
            let mut request = Request::new().expect("allocate a request");
            assert!(!request.is_finish());
            // C's DEVICE_WRITE is 0, so a fresh request reads as Write.
            assert_eq!(request.direction(), Some(DeviceDirection::Write));
            assert!(request.buffer_mut().is_none());

            request.flag = DeviceDirection::Read.into();
            assert_eq!(request.direction(), Some(DeviceDirection::Read));
            request.flag = 3;
            assert_eq!(request.direction(), None);

            // The native buffer view is the payload C would copy.
            let mut payload = [0u8; 32];
            request.data = payload.as_mut_ptr() as *mut c_void;
            request.data_len = payload.len();
            let buffer = request.buffer_mut().expect("data is set");
            assert_eq!(buffer.len(), 32);
            buffer[0] = 0xab;
            assert_eq!(payload[0], 0xab);
            request.data = ptr::null_mut();

            request.mark_finished();
            assert!(request.is_finish());
            assert!(!request.as_ptr().is_null());
        }
    }

    #[test]
    fn native_address_and_direction_surface() {
        let addr = device_address::new(1, 2, 3, 4);
        assert_eq!(u32::from(addr), addr.lpn);
        assert_eq!(device_address::from(addr.lpn), addr);
        #[cfg(not(feature = "zone"))]
        assert_eq!(addr.to_string(), "bus 1 chip 2 block 3 page 4 (lpn 0x1849)");
        #[cfg(feature = "zone")]
        assert_eq!(addr.to_string(), "bus 1 chip 2 block 3 page 4 (lpn 0x1911)");

        assert_eq!(DeviceDirection::from_flag(0), Some(DeviceDirection::Write));
        assert_eq!(DeviceDirection::from_flag(2), Some(DeviceDirection::Erase));
        assert_eq!(DeviceDirection::from_flag(3), None);
        assert_eq!(c_uint::from(DeviceDirection::Read), 1);
    }

    /// Backend used to drive the trait, the vtable bridge and the dispatch.
    struct MockBackend;

    static MOCK_OPENS: AtomicI32 = AtomicI32::new(0);
    static MOCK_EXITS: AtomicI32 = AtomicI32::new(0);

    impl DeviceOperations for MockBackend {
        unsafe fn open(&self, _dev: *mut device, _name: *const c_char, _flags: c_int) -> c_int {
            MOCK_OPENS.fetch_add(1, Ordering::SeqCst);
            0
        }

        unsafe fn write(&self, _dev: *mut device, request: *mut device_request) -> isize {
            if (*request).flag != DeviceDirection::Write as c_uint {
                return -libc::EINVAL as isize;
            }
            (*request).data_len as isize
        }

        unsafe fn read(&self, _dev: *mut device, request: *mut device_request) -> isize {
            (*request).data_len as isize
        }

        unsafe fn erase(&self, _dev: *mut device, _request: *mut device_request) -> c_int {
            0
        }

        unsafe fn close(&self, _dev: *mut device) -> c_int {
            0
        }
    }

    unsafe extern "C" fn mock_init(dev: *mut device, _flags: u64) -> c_int {
        (*dev).install_backend(MockBackend);
        (*dev).d_submodule_exit = Some(mock_exit);
        0
    }

    unsafe extern "C" fn mock_exit(dev: *mut device) -> c_int {
        MOCK_EXITS.fetch_add(1, Ordering::SeqCst);
        (*dev).uninstall_backend::<MockBackend>();
        0
    }

    #[test]
    fn module_init_dispatches_through_the_trait() {
        unsafe {
            let table = [Some(mock_init as SubmoduleInitFn), None, None, None];
            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                module_init_with(&table, DeviceModule::Ramdisk as u64, &mut dev, 0),
                0
            );
            assert!(!dev.is_null());
            assert!(!(*dev).d_op.is_null());
            assert!((*dev).badseg_bitmap.is_null());

            // C reaches the Rust implementation through the vtable.
            let ops = &*(*dev).d_op;
            let opens = MOCK_OPENS.load(Ordering::SeqCst);
            assert_eq!((ops.open.unwrap())(dev, ptr::null(), 0), 0);
            assert_eq!(MOCK_OPENS.load(Ordering::SeqCst), opens + 1);

            let mut request = zeroed_request();
            request.flag = DeviceDirection::Write as c_uint;
            request.data_len = 4096;
            assert_eq!((ops.write.unwrap())(dev, &mut request), 4096);

            request.flag = DeviceDirection::Read as c_uint;
            assert_eq!(
                (ops.write.unwrap())(dev, &mut request),
                -libc::EINVAL as isize
            );
            assert_eq!((ops.read.unwrap())(dev, &mut request), 4096);
            assert_eq!((ops.erase.unwrap())(dev, &mut request), 0);
            assert_eq!((ops.close.unwrap())(dev), 0);

            let exits = MOCK_EXITS.load(Ordering::SeqCst);
            assert_eq!(ffi_device_module_exit(dev), 0);
            assert_eq!(MOCK_EXITS.load(Ordering::SeqCst), exits + 1);
        }
    }

    #[test]
    fn module_init_error_paths() {
        unsafe {
            let table = [None; DEVICE_MODULE_COUNT];
            let mut dev: *mut device = ptr::null_mut();

            // A valid module without a backend: -ENODEV, nothing allocated.
            assert_eq!(
                module_init_with(&table, DeviceModule::Ramdisk as u64, &mut dev, 0),
                -libc::ENODEV
            );
            assert!(dev.is_null());

            // Out of range modnum: C reads past submodule_init[] here.
            assert_eq!(
                module_init_with(&table, DEVICE_MODULE_COUNT as u64, &mut dev, 0),
                -libc::EINVAL
            );
            assert_eq!(
                module_init_with(&table, u64::MAX, &mut dev, 0),
                -libc::EINVAL
            );

            // NULL output pointer: C dereferences it.
            assert_eq!(
                module_init_with(&table, DeviceModule::Ramdisk as u64, ptr::null_mut(), 0),
                -libc::EINVAL
            );

            assert_eq!(ffi_device_module_exit(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn install_and_uninstall_backend_keep_the_vtable_and_the_slot_in_step() {
        unsafe {
            let mut dev = zeroed_device();
            dev.install_backend(MockBackend);
            assert!(!dev.d_op.is_null());
            assert!(!dev.d_private.is_null());

            // The vtable lives in the same allocation as the backend, so the
            // pointer C calls through and the type the shim recovers cannot
            // disagree.
            let ops = &*dev.d_op;
            let opens = MOCK_OPENS.load(Ordering::SeqCst);
            assert_eq!((ops.open.unwrap())(&mut dev, ptr::null(), 0), 0);
            assert_eq!(MOCK_OPENS.load(Ordering::SeqCst), opens + 1);

            dev.uninstall_backend::<MockBackend>();
            assert!(dev.d_private.is_null());
            assert!(dev.d_op.is_null());

            // A second uninstall is a no-op, not a double free.
            dev.uninstall_backend::<MockBackend>();
        }
    }

    #[test]
    fn module_init_rejects_a_table_that_is_too_short() {
        unsafe {
            // Tasks 6/7 grow the backend list one entry at a time, so a list
            // shorter than the module list must report -ENODEV instead of
            // indexing past its end.
            let table = [Some(mock_init as SubmoduleInitFn)];

            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                module_init_with(&table, DeviceModule::Ramdisk as u64, &mut dev, 0),
                0
            );
            assert!(!dev.is_null());
            assert_eq!(ffi_device_module_exit(dev), 0);

            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                module_init_with(&table, DeviceModule::Zone as u64, &mut dev, 0),
                -libc::ENODEV
            );
            assert!(dev.is_null());
        }
    }

    #[test]
    fn exported_module_init_reports_each_backend() {
        // Task 6 wired the ramdisk entry, #108 wired the zoned entry to the
        // ported backend, and Task 7 wires the remaining stub slots behind
        // their Cargo features, so only the slots still empty report -ENODEV.
        unsafe {
            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                0
            );
            assert!(!dev.is_null());
            assert!(!(*dev).d_op.is_null());
            assert_eq!(ffi_device_module_exit(dev), 0);

            for (module, wired) in [
                (DeviceModule::Bluedbm, cfg!(feature = "bluedbm")),
                (DeviceModule::Zone, cfg!(feature = "zone")),
                (DeviceModule::Raspberry, cfg!(feature = "raspberry")),
            ] {
                let mut dev: *mut device = ptr::null_mut();
                if !wired {
                    assert_eq!(
                        ffi_device_module_init(module as u64, &mut dev, 0),
                        -libc::ENODEV
                    );
                    assert!(dev.is_null());
                    continue;
                }

                // The wired slot holds that backend, whose vtable C would
                // call.
                assert_eq!(
                    ffi_device_module_init(module as u64, &mut dev, 0),
                    0,
                    "{module:?} should install its backend"
                );
                assert!(!dev.is_null());
                // The operations are not driven here, each backend's own
                // module covers them. Driving `open` would touch hardware for
                // a ported backend and the stub would answer -ENOSYS.
                assert!((*(*dev).d_op).open.is_some());
                assert_eq!(ffi_device_module_exit(dev), 0);
            }

            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DEVICE_MODULE_COUNT as u64, &mut dev, 0),
                -libc::EINVAL
            );
        }
    }
}
