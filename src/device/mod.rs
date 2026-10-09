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
//! `dev->d_op`; Rust backends implement [`DeviceOperations`] and
//! [`device_operations::from_backend`] turns that implementation into the
//! vtable, so C keeps dispatching through `dev->d_op` unchanged. The
//! C-named structs keep their C names (a `#[repr(C)]` type is not subject to
//! `non_camel_case_types`); type aliases are camel-cased as in `src/lru.rs`.
//!
//! Symbols carry the `ffi_` prefix so they can coexist with `device/device.c`
//! while both implementations are linked; the cutover is Task 14.
//!
//! Deliberate deviations from the C original:
//!
//! - `ffi_device_module_init` rejects an out-of-range `modnum` with `-EINVAL`
//!   and a module with no backend with `-ENODEV`. C indexes
//!   `submodule_init[modnum]` unchecked and calls a `NULL` entry, so both are
//!   wild calls there.
//! - `ffi_device_module_init` and `ffi_device_free_request` accept `NULL`
//!   (`-EINVAL` / no-op); C dereferences it, and `device_module_exit` asserts.
//! - A failing `pthread_mutex_init` / `pthread_cond_init` releases the
//!   partially built structure. C leaks the request (mutex failure) or the
//!   mutex (cond failure), and ignores the mutex failure in
//!   `device_module_init` entirely.
//! - `ffi_device_module_init` cannot fail with `-ENOMEM`: `Box` aborts on OOM,
//!   as in `src/list.rs`.
//! - Geometry lives on [`device`] as safe methods instead of the header's
//!   `static inline`s. The C inlines stay until the cutover, so no C-ABI
//!   symbol is exported for them yet.
//! - The bit counts and `DEVICE_PAGE_SIZE` are the default (ramdisk)
//!   `DEVICE_INFO` from the Makefile. The non-default backends compile C with
//!   different `-DDEVICE_NR_*_BITS` (zoned `3/3/5/21`, raspberry `1/1/4/24`
//!   with a 2048 byte page); their Rust ports (Task 7) must bring the matching
//!   configuration with them.
//! - `ffi_device_module_init` dispatches through an all-`None` backend table:
//!   the Rust backends arrive with Tasks 6 (ramdisk) and 7 (stubs), and C's
//!   `device/device.c` keeps its own table and stays the live path for C
//!   callers until the cutover, so nothing regresses meanwhile.

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::mem::MaybeUninit;
use core::ptr;
use std::sync::atomic::{AtomicI32, Ordering};

use libc::{pthread_cond_t, pthread_mutex_t};

use crate::pr_err;

/// C's `PADDR_EMPTY`: the "physical address not assigned" sentinel.
pub const PADDR_EMPTY: u32 = u32::MAX;

/// C's `DEVICE_DEFAULT_REQUEST`: the only request allocation flag defined.
pub const DEVICE_DEFAULT_REQUEST: u64 = 0;

/// Default (ramdisk) geometry, mirroring the Makefile's `DEVICE_INFO`.
pub const DEVICE_NR_BUS_BITS: u32 = 2;
pub const DEVICE_NR_CHIPS_BITS: u32 = 2;
pub const DEVICE_NR_PAGES_BITS: u32 = 7;
pub const DEVICE_NR_BLOCKS_BITS: u32 = 19;

/// C's `DEVICE_PAGE_SIZE`, the NAND page size of the default build.
pub const DEVICE_PAGE_SIZE: usize = 8192;

const BUS_SHIFT: u32 = 0;
const CHIP_SHIFT: u32 = DEVICE_NR_BUS_BITS;
const PAGE_SHIFT: u32 = DEVICE_NR_BUS_BITS + DEVICE_NR_CHIPS_BITS;
const BLOCK_SHIFT: u32 = PAGE_SHIFT + DEVICE_NR_PAGES_BITS;

const BUS_MASK: u32 = (1u32 << DEVICE_NR_BUS_BITS) - 1;
const CHIP_MASK: u32 = (1u32 << DEVICE_NR_CHIPS_BITS) - 1;
const PAGE_MASK: u32 = (1u32 << DEVICE_NR_PAGES_BITS) - 1;
const BLOCK_MASK: u32 = (1u32 << DEVICE_NR_BLOCKS_BITS) - 1;

/// Width of the `page` field in C's `raspberry` address view.
const RASPBERRY_PAGE_BITS: u32 =
    DEVICE_NR_BUS_BITS + DEVICE_NR_CHIPS_BITS + DEVICE_NR_PAGES_BITS;

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

/// Mirrors `struct device_address` in `include/device.h`.
///
/// C declares it as a union of bitfield views over one `uint32_t`; Rust keeps
/// the `lpn` word and reproduces the views with shifts, so the bit order is
/// explicit instead of compiler-defined. C's bitfield order on a little-endian
/// target puts the first declared field in the least significant bits, which
/// is what the shifts below encode.
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

    /// C's `raspberry` view: the page field covers bus, chip and page.
    pub const fn raspberry_page(&self) -> u32 {
        self.lpn & ((1u32 << RASPBERRY_PAGE_BITS) - 1)
    }

    /// C's `raspberry` view: the block field covers everything above the page.
    pub const fn raspberry_block(&self) -> u32 {
        self.lpn >> RASPBERRY_PAGE_BITS
    }
}

/// Callback run when the device finishes a request. Mirrors
/// `device_end_req_fn` in `include/device.h` (renamed for Rust's
/// `non_camel_case_types`; the ABI is the same nullable function pointer).
pub type DeviceEndReqFn = Option<unsafe extern "C" fn(*mut device_request)>;

/// Mirrors `struct device_request` in `include/device.h`.
///
/// `mutex` / `cond` guard `is_finish`; the FTL allocates a request per I/O and
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
/// routine; the geometry lives in `info` and is filled by the backend's `open`.
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
    pub fn get_nr_segments(&self) -> usize {
        self.info.package.nr_blocks
    }

    /// Number of blocks in a segment (C's `device_get_blocks_per_segment`).
    pub fn get_blocks_per_segment(&self) -> usize {
        self.info.nr_bus * self.info.nr_chips
    }

    /// Number of pages in a segment (C's `device_get_pages_per_segment`).
    pub fn get_pages_per_segment(&self) -> usize {
        self.get_blocks_per_segment() * self.info.package.block.nr_pages
    }

    /// NAND page size in bytes (C's `device_get_page_size`).
    pub fn get_page_size(&self) -> usize {
        self.info.package.block.page.size
    }

    /// Total size of the flash board in bytes (C's `device_get_total_size`).
    pub fn get_total_size(&self) -> usize {
        self.get_nr_segments() * self.get_pages_per_segment() * self.get_page_size()
    }

    /// Total number of pages in the flash board (C's `device_get_total_pages`).
    ///
    /// Panics when the page size is zero; C divides by zero there.
    pub fn get_total_pages(&self) -> usize {
        self.get_total_size() / self.get_page_size()
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

/// Rust-side device backend interface; the trait-based form of the
/// `device_operations` vtable.
///
/// A backend stores itself in [`device::d_private`] and installs
/// [`device_operations::from_backend::<Self>`] into [`device::d_op`]. The shims
/// recover the backend from `d_private`, so C calling `dev->d_op->read(...)`
/// reaches this implementation.
///
/// # Safety
///
/// Every method is called by C with the device the backend was initialized
/// into; implementations must tolerate being called from any thread and must
/// not move themselves.
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

unsafe extern "C" fn shim_open<B: DeviceOperations>(
    dev: *mut device,
    name: *const c_char,
    flags: c_int,
) -> c_int {
    let backend = &*((*dev).d_private as *const B);
    backend.open(dev, name, flags)
}

unsafe extern "C" fn shim_write<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    let backend = &*((*dev).d_private as *const B);
    backend.write(dev, request)
}

unsafe extern "C" fn shim_read<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    let backend = &*((*dev).d_private as *const B);
    backend.read(dev, request)
}

unsafe extern "C" fn shim_erase<B: DeviceOperations>(
    dev: *mut device,
    request: *mut device_request,
) -> c_int {
    let backend = &*((*dev).d_private as *const B);
    backend.erase(dev, request)
}

unsafe extern "C" fn shim_close<B: DeviceOperations>(dev: *mut device) -> c_int {
    let backend = &*((*dev).d_private as *const B);
    backend.close(dev)
}

impl device_operations {
    /// Build the C-visible vtable from a Rust backend.
    ///
    /// `B` must be the type stored in [`device::d_private`], because that is
    /// where the shims recover the implementation from.
    pub const fn from_backend<B: DeviceOperations>() -> Self {
        Self {
            open: Some(shim_open::<B>),
            write: Some(shim_write::<B>),
            read: Some(shim_read::<B>),
            erase: Some(shim_erase::<B>),
            close: Some(shim_close::<B>),
        }
    }
}

/// Init routine of a device backend, C's `submodule_init[]` element type.
pub type SubmoduleInitFn = unsafe extern "C" fn(*mut device, u64) -> c_int;

/// C's `submodule_init[]`, indexed by [`DeviceModule`].
///
/// Every slot is `None` in this task: the Rust backends arrive with Tasks 6
/// (ramdisk) and 7 (stubs). C's `device/device.c` keeps its own table and stays
/// the live path for C callers until the Task 14 cutover, so no C caller
/// regresses while the Rust table is still empty.
static SUBMODULE_INIT: [Option<SubmoduleInitFn>; DEVICE_MODULE_COUNT] =
    [None; DEVICE_MODULE_COUNT];

/// Set `errno`, the way C's failure paths do before returning a negative value.
///
/// C stores the pthread return code in `errno` so callers can read it after
/// `NULL`; `libc` exposes the location per target.
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
/// `flags` is ignored like C does; [`DEVICE_DEFAULT_REQUEST`] is the only flag
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
/// `out` is `NULL` or a writable pointer; the table's entries follow the
/// backend init contract of C's `submodule_init[]`.
unsafe fn module_init_with(
    table: &[Option<SubmoduleInitFn>; DEVICE_MODULE_COUNT],
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

    let Some(submodule_init) = table[module as usize] else {
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
/// Returns `0` on success and a negative value on failure; on failure `*dev` is
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
        // bus 2 bits, chip 2 bits, page 7 bits, block 19 bits.
        let addr = device_address::new(1, 2, 3, 4);
        assert_eq!(addr.lpn, 1 | (2 << 2) | (4 << 4) | (3 << 11));
        assert_eq!(addr.lpn, 6217);
        assert_eq!(addr.bus(), 1);
        assert_eq!(addr.chip(), 2);
        assert_eq!(addr.page(), 4);
        assert_eq!(addr.block(), 3);

        // Every bit of the packed word is addressable, and the fields truncate
        // to their declared width like C's bitfields do.
        let full = device_address::new(BUS_MASK, CHIP_MASK, BLOCK_MASK, PAGE_MASK);
        assert_eq!(full.lpn, (1u32 << 30) - 1);
        assert_eq!(full.block(), BLOCK_MASK);

        // The "empty" sentinel is not a packable address: the default geometry
        // uses 30 of the 32 bits.
        assert_ne!(PADDR_EMPTY, full.lpn);
        assert_eq!(device_address { lpn: PADDR_EMPTY }.lpn, u32::MAX);

        // C's raspberry view: page = bus | chip | page, block = the rest. With
        // the default geometry that page field is 11 bits wide (the raspberry
        // build narrows it to 6: bus 1 + chip 1 + page 4).
        assert_eq!(
            addr.raspberry_page(),
            addr.lpn & ((1 << RASPBERRY_PAGE_BITS) - 1)
        );
        assert_eq!(addr.raspberry_page(), 1 | (2 << 2) | (4 << 4));
        assert_eq!(addr.raspberry_page(), 73);
        assert_eq!(addr.raspberry_block(), addr.lpn >> RASPBERRY_PAGE_BITS);
        assert_eq!(addr.raspberry_block(), 3);

        assert_eq!(device_address::default().lpn, 0);
    }

    #[test]
    fn geometry_helpers_match_c() {
        let mut dev = zeroed_device();
        dev.info.nr_bus = 1 << DEVICE_NR_BUS_BITS;
        dev.info.nr_chips = 1 << DEVICE_NR_CHIPS_BITS;
        dev.info.package.nr_blocks = 64;
        dev.info.package.block.nr_pages = 1 << DEVICE_NR_PAGES_BITS;
        dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

        assert_eq!(dev.get_nr_segments(), 64);
        assert_eq!(dev.get_blocks_per_segment(), 16);
        assert_eq!(dev.get_pages_per_segment(), 16 * 128);
        assert_eq!(dev.get_page_size(), DEVICE_PAGE_SIZE);
        // The Makefile's default build is the 1GiB ramdisk.
        assert_eq!(dev.get_total_size(), 1 << 30);
        assert_eq!(dev.get_total_pages(), (1 << 30) / DEVICE_PAGE_SIZE);
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

    static MOCK_OPS: device_operations = device_operations::from_backend::<MockBackend>();

    unsafe extern "C" fn mock_init(dev: *mut device, _flags: u64) -> c_int {
        (*dev).d_op = &MOCK_OPS;
        (*dev).d_private = Box::into_raw(Box::new(MockBackend)) as *mut c_void;
        (*dev).d_submodule_exit = Some(mock_exit);
        0
    }

    unsafe extern "C" fn mock_exit(dev: *mut device) -> c_int {
        MOCK_EXITS.fetch_add(1, Ordering::SeqCst);
        drop(Box::from_raw((*dev).d_private as *mut MockBackend));
        (*dev).d_private = ptr::null_mut();
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
            assert_eq!((ops.open.unwrap())(dev, ptr::null(), 0), 0);
            assert_eq!(MOCK_OPENS.load(Ordering::SeqCst), 1);

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
    fn exported_module_init_reports_unported_backends() {
        // The exported table is still empty: Tasks 6 and 7 fill it, and this
        // assertion is what tells them the seam moved.
        unsafe {
            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                -libc::ENODEV
            );
            assert!(dev.is_null());
            assert_eq!(
                ffi_device_module_init(DEVICE_MODULE_COUNT as u64, &mut dev, 0),
                -libc::EINVAL
            );
        }
    }
}
