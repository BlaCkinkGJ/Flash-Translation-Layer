//! Raspberry Pi device backend ported from `device/raspberry/raspberry.c`.
//!
//! [`Raspberry`] is the Pi's NAND backend. It talks to the controller through
//! libnand (`nand_init`, `nand_write`, `nand_read`, `nand_erase`, `nand_free`
//! and `nand_get_read_error_msg`) and keeps C's buffer ownership: the caller
//! owns the payload and its lifetime, the backend only passes `request->data`
//! to libnand and never allocates, copies or frees it. The one allocation the
//! backend owns is `dev->badseg_bitmap`, released by `close` like C's.
//!
//! Geometry is the raspberry build's `DEVICE_INFO` from the Makefile's
//! `USE_RASPBERRY_DEVICE=1` block: bus 1 bit, chip 1 bit, page 4 bits, block
//! 24 bits, 2048 byte pages, and C's `package->nr_blocks = 1024`. That is 1024
//! segments of 4 blocks of 16 pages, 128 MiB in total.
//!
//! C's `struct raspberry` is not mirrored field for field. `size` is written
//! twice and read only by the `pr_info` in `raspberry_open`, `o_flags` is
//! written and never read, and no other file touches either, so the size is
//! logged from the local that computes it and both fields are dropped. The
//! `pthread_spinlock_t lock` becomes a [`Mutex`]`<()>`: C never calls
//! `pthread_spin_init` on it, so C locks an uninitialized object, and Rust has
//! no uninitialized-mutex form. C's `__raspberry_dops` static vtable is gone
//! the way the ramdisk one is, [`DeviceOperations`] is the vtable and
//! `ffi_raspberry_device_init` installs it through [`crate::device`].
//!
//! libnand and wiringPi are bound by hand: the `extern "C"` block below is the
//! C prototype list `raspberry.c` calls, `nand.h` is not in this tree, and the
//! two [`link`] attributes are the Makefile's `-lnand -lwiringPi`. No wiringPi
//! function is called here, the source only needs the library for libnand's
//! own references, exactly as the C build does. The symbols live behind
//! [`NandApi`], a struct of function pointers with the same signatures, so the
//! unit tests can substitute the library and cover the geometry, the request
//! checks and the buffer and bitmap bookkeeping without the hardware. A
//! machine without the libraries fails `cargo test --features raspberry` at
//! link time (`ld: library 'nand' not found`), which is the failure the C
//! `USE_RASPBERRY_DEVICE=1` build has too, and `cargo check` and clippy still
//! pass because neither links. Install libnand and wiringPi (the same two the
//! Makefile lists) and the feature build and its tests link on a machine that
//! has them.
//!
//! Other deliberate deviations from the C original:
//!
//! - The six libnand calls go through [`NandApi`], a struct of function
//!   pointers filled from the `extern "C"` block, where C names the symbols
//!   directly. That is a deliberate divergence from the literal port: without
//!   the seam no test can run at all, because libnand is absent from every
//!   machine here. The signatures and the call order are still C's, and
//!   wiringPi is linked but never called, the source needs it only for
//!   libnand's own references.
//! - The geometry constants live in this module instead of arriving as `-D`
//!   for the whole build. The FTL and the other backends keep the default
//!   ramdisk geometry, so the raspberry bit counts are expressed where C uses
//!   them: `open` and C's `raspberry` address view.
//! - C's four `addr.raspberry_converter` field copies are a masked copy of
//!   `request->paddr` (both union views use this build's bit counts, and
//!   `addr.lpn = 0` precedes the copies), so the port masks the packed word
//!   once instead of copying four fields.
//! - `open` releases a previous bitmap instead of leaking it the way C does
//!   when it is called twice, which is a deliberate deviation from the literal
//!   port rather than C behaviour.
//! - The bitmap is `nr_segments.div_ceil(64)` words. C's
//!   `BITS_TO_UINT64_ALIGN` over-allocates by one `u64`, which no accessor
//!   reaches.
//! - `open` cannot fail with `-ENOMEM` on the backend structure, `Box` aborts
//!   on OOM as in the rest of the port. The bitmap `calloc` keeps its
//!   `-ENOMEM` path.
//! - `nand_free` runs from [`Drop`] for [`Raspberry`], so a second
//!   `ffi_raspberry_device_exit` neither closes nor frees twice, where C calls
//!   `nand_free()` on every exit.
//! - A block number past the end of the bitmap panics on the slice bounds
//!   check, where C's `set_bit` writes past the allocation. `erase` also skips
//!   the bitmap when `open` has not run, where C dereferences `NULL`.
//! - `read` drops C's `pr_debug` line that reinterprets `request->rq_private`
//!   as a `struct device_request` to print a mutex address.
//! - The operation level mirrors `ffi_raspberry_{open,write,read,erase}` are
//!   not exported, the `shim_*` wrappers in [`crate::device`] are the vtable
//!   entries, so an installed mirror would recurse through `d_op`. Callers
//!   reach the operations through `dev->d_op`, which is what the C entry
//!   points do.
//! - `ffi_raspberry_close`, `ffi_raspberry_device_init` and
//!   `ffi_raspberry_device_exit` accept `NULL` (`-EINVAL` / no-op) where C
//!   dereferences it, as the ramdisk entry points do.
//!
//! Every branch in this module except one is reached by the tests below. The
//! exception is the `calloc` failure in `open`, which reports `-ENOMEM`:
//! forcing an allocation failure needs `LD_PRELOAD` or a failing `calloc` in
//! the test build, and that is the check a later run needs before the line
//! can be called covered.

use core::ffi::{c_char, c_int, c_uint, c_void, CStr};
use core::mem::size_of;
use core::ptr;
use core::slice;
use std::borrow::Cow;
use std::sync::Mutex;

use crate::bits::{set_bit, BITS_PER_UINT64};
use crate::device::{
    device, device_request, DeviceDirection, DeviceOperations, PADDR_EMPTY,
};
use crate::{pr_err, pr_info, pr_warn};

/// The raspberry build's `DEVICE_NR_BUS_BITS`, from the Makefile's
/// `USE_RASPBERRY_DEVICE=1` block.
const NR_BUS_BITS: u32 = 1;

/// The raspberry build's `DEVICE_NR_CHIPS_BITS`.
const NR_CHIPS_BITS: u32 = 1;

/// The raspberry build's `DEVICE_NR_PAGES_BITS`.
const NR_PAGES_BITS: u32 = 4;

/// The raspberry build's `DEVICE_PAGE_SIZE`.
const PAGE_SIZE: usize = 2048;

/// The raspberry build's `DEVICE_NR_BLOCKS_BITS`.
const NR_BLOCKS_BITS: u32 = 24;

/// C's `package->nr_blocks = 1024` in `raspberry_open`, the one geometry field
/// the backend sets outright instead of deriving it from a bit count.
const NR_BLOCKS: usize = 1024;

/// Width of C's `addr.raspberry.page`, the bus, chip and page bits running
/// together in the low bits of the packed address.
const VIEW_BITS: u32 = NR_BUS_BITS + NR_CHIPS_BITS + NR_PAGES_BITS;

/// Mask of C's `addr.raspberry.page`.
const PAGE_MASK: u32 = (1 << VIEW_BITS) - 1;

/// Mask of C's `format` view, the bus, chip, page and block bits together.
/// `addr.lpn = 0` followed by the four `raspberry_converter` copies keeps only
/// these bits, so the two above them are cleared before the `raspberry` view
/// reads the word.
const FORMAT_MASK: u32 = (1 << (VIEW_BITS + NR_BLOCKS_BITS)) - 1;

// The library C links with `-lnand` in the `USE_RASPBERRY_DEVICE=1` build.
// The signatures are written out because `nand.h` lives in the external
// libnand project.
#[link(name = "nand")]
extern "C" {
    fn nand_init() -> c_int;
    fn nand_free();
    fn nand_write(buffer: *mut c_char, block: c_uint, page: c_uint) -> c_int;
    fn nand_read(buffer: *mut c_char, block: c_uint, page: c_uint) -> c_int;
    fn nand_erase(block: c_uint) -> c_int;
    fn nand_get_read_error_msg(error: c_int) -> *const c_char;
}

// libnand's own dependency, which the Makefile adds with `-lwiringPi` for the
// same reason. The port calls no wiringPi function, so the block is empty.
#[link(name = "wiringPi")]
extern "C" {}

/// C's `addr.raspberry.page`, the page number libnand is called with, read
/// from an address already put through [`FORMAT_MASK`].
fn raspberry_page(lpn: u32) -> u32 {
    lpn & PAGE_MASK
}

/// C's `addr.raspberry.block`, the block number libnand is called with, read
/// from an address already put through [`FORMAT_MASK`]. C's `format.block`
/// field is 24 bits in this build, and the erase path then truncates it to 16,
/// which [`DeviceOperations::erase`] reproduces.
fn raspberry_block(lpn: u32) -> u32 {
    lpn >> VIEW_BITS
}

/// Signature of libnand's `int nand_init(void)`.
type NandInit = unsafe extern "C" fn() -> c_int;

/// Signature of libnand's `void nand_free(void)`.
type NandFree = unsafe extern "C" fn();

/// Signature of libnand's `int nand_write(char *buffer, unsigned int block,
/// unsigned int page)`.
type NandWrite = unsafe extern "C" fn(*mut c_char, c_uint, c_uint) -> c_int;

/// Signature of libnand's `int nand_read(char *buffer, unsigned int block,
/// unsigned int page)`.
type NandRead = unsafe extern "C" fn(*mut c_char, c_uint, c_uint) -> c_int;

/// Signature of libnand's `int nand_erase(unsigned int block)`. C passes a
/// `uint16_t`, which this ABI passes the same way.
type NandErase = unsafe extern "C" fn(c_uint) -> c_int;

/// Signature of libnand's `const char *nand_get_read_error_msg(int error)`.
type NandReadErrorMsg = unsafe extern "C" fn(c_int) -> *const c_char;

/// The libnand entry points `device/raspberry/raspberry.c` calls, with the C
/// prototypes above written out by hand because no header is in this tree.
#[derive(Clone, Copy)]
struct NandApi {
    init: NandInit,
    free: NandFree,
    write: NandWrite,
    read: NandRead,
    erase: NandErase,
    read_error_msg: NandReadErrorMsg,
}

impl NandApi {
    /// The library entry points, the direct calls `raspberry.c` makes.
    fn system() -> Self {
        Self {
            init: nand_init,
            free: nand_free,
            write: nand_write,
            read: nand_read,
            erase: nand_erase,
            read_error_msg: nand_get_read_error_msg,
        }
    }

    /// `nand_get_read_error_msg` as a string. C hands the result to
    /// `pr_warn("...%s", ...)`, where glibc prints `(null)` for a `NULL`
    /// pointer, which this reproduces instead of dereferencing it.
    ///
    /// # Safety
    /// The library returns a NUL terminated string for the codes it knows.
    unsafe fn error_message(&self, error: c_int) -> Cow<'static, str> {
        let message = (self.read_error_msg)(error);
        if message.is_null() {
            Cow::Borrowed("(null)")
        } else {
            CStr::from_ptr(message).to_string_lossy()
        }
    }
}

/// C's `set_bit(dev->badseg_bitmap, segnum)`. A bit past the `nr_segments`
/// sized bitmap panics on the slice bounds check instead of writing past the
/// allocation the way C's macro does, and a `NULL` bitmap (no `open` yet) is
/// skipped where C dereferences it.
///
/// # Safety
/// `dev` is the device this backend was installed into.
unsafe fn mark_bad_segment(dev: *mut device, segnum: u32) {
    let bitmap = (*dev).badseg_bitmap;
    if bitmap.is_null() {
        return;
    }
    let words = (*dev).nr_segments().div_ceil(BITS_PER_UINT64);
    set_bit(slice::from_raw_parts_mut(bitmap, words), segnum as u64);
}

/// The raspberry backend, the Rust form of C's `struct raspberry`.
pub struct Raspberry {
    /// The libnand entry points, which C calls directly.
    nand: NandApi,
    /// C's `pthread_spinlock_t lock`, held across one libnand call at a time.
    lock: Mutex<()>,
}

impl Raspberry {
    /// The backend around the libnand entry points, the real ones from
    /// [`NandApi::system`] or a test's substitutes.
    fn new(nand: NandApi) -> Self {
        Self {
            nand,
            lock: Mutex::new(()),
        }
    }
}

impl Drop for Raspberry {
    /// C's `raspberry_device_exit` calls `nand_free()` after it releases the
    /// backend. Tying the call to the drop keeps that order (close, release,
    /// then `nand_free`), and it runs once per installed backend instead of
    /// once per exit.
    fn drop(&mut self) {
        unsafe { (self.nand.free)() }
    }
}

impl DeviceOperations for Raspberry {
    /// Allocate the flash geometry and the bad segment bitmap, then erase
    /// every block once and mark the ones that fail. Mirrors C's
    /// `raspberry_open`, including the "TODO: this part will be removed after
    /// implmenting recovery routine" pass over all 1024 blocks.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn open(
        &self,
        dev: *mut device,
        _name: *const c_char,
        _flags: c_int,
    ) -> c_int {
        (*dev).info.nr_bus = 1usize << NR_BUS_BITS;
        (*dev).info.nr_chips = 1usize << NR_CHIPS_BITS;
        (*dev).info.package.block.nr_pages = 1usize << NR_PAGES_BITS;
        (*dev).info.package.nr_blocks = NR_BLOCKS;
        (*dev).info.package.block.page.size = PAGE_SIZE;

        pr_info!("raspberry generated (size: {} bytes)", (*dev).total_size());

        // C leaks the previous bitmap when `open` runs twice. Release it the
        // way `close` does, so the two paths cannot disagree.
        if !(*dev).badseg_bitmap.is_null() {
            libc::free((*dev).badseg_bitmap as *mut c_void);
            (*dev).badseg_bitmap = ptr::null_mut();
        }

        let nr_segments = (*dev).nr_segments();
        let words = nr_segments.div_ceil(BITS_PER_UINT64);
        // C's `malloc(BITS_TO_UINT64_ALIGN(nr_segments))` plus `memset`.
        let bitmap = libc::calloc(words, size_of::<u64>()) as *mut u64;
        if bitmap.is_null() {
            pr_err!("memory allocation failed");
            return -libc::ENOMEM;
        }
        (*dev).badseg_bitmap = bitmap;

        // C's loop bound is `package->nr_blocks`, which `nr_segments` is.
        let bits = slice::from_raw_parts_mut(bitmap, words);
        for block in 0..nr_segments {
            let status = {
                let _guard = self.lock.lock().unwrap();
                (self.nand.erase)(block as c_uint)
            };
            if status != 0 {
                set_bit(bits, block as u64);
            }
        }

        0
    }

    /// Write one page through libnand. Mirrors C's `raspberry_write`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request whose `data` points at `data_len` readable bytes.
    unsafe fn write(
        &self,
        dev: *mut device,
        request: *mut device_request,
    ) -> isize {
        // C checks `request->paddr.lpn`, the raw word, and fills `addr`, its
        // own masked copy. `raspberry.c` builds it through the
        // `raspberry_converter` view, four field-by-field copies of
        // `request->paddr.format` into a word it zeroed first. Both views use
        // this build's bit counts, so the copies are one masked copy.
        let paddr = (*request).paddr.lpn;
        let page_size = (*dev).page_size();

        if (*request).data.is_null() {
            pr_err!("you do not pass the data pointer to NULL");
            return -(libc::ENODATA as isize);
        }
        if (*request).direction() != Some(DeviceDirection::Write) {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                DeviceDirection::Write as c_uint,
                (*request).flag
            );
            return -(libc::EINVAL as isize);
        }
        if paddr == PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }
        if (*request).data_len != page_size {
            pr_err!(
                "data write size is must be {} (current: {})",
                page_size,
                (*request).data_len
            );
            return -(libc::EINVAL as isize);
        }

        let lpn = paddr & FORMAT_MASK;
        let ret = {
            let _guard = self.lock.lock().unwrap();
            (self.nand.write)(
                (*request).data as *mut c_char,
                raspberry_block(lpn),
                raspberry_page(lpn),
            )
        };
        if ret != 0 {
            // C returns the library's error unchanged, after printing it.
            pr_err!("write error detected {}", ret);
            return ret as isize;
        }

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        (*request).data_len as isize
    }

    /// Read one page through libnand. Mirrors C's `raspberry_read`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request whose `data` points at `data_len` writable bytes.
    unsafe fn read(
        &self,
        dev: *mut device,
        request: *mut device_request,
    ) -> isize {
        // Same `addr` construction as `write`, and the same split between the
        // raw `request->paddr.lpn` the checks use and the masked copy libnand
        // is called with.
        let paddr = (*request).paddr.lpn;

        if (*request).data.is_null() {
            pr_err!("you do not pass the data pointer to NULL");
            return -(libc::ENODATA as isize);
        }
        if (*request).direction() != Some(DeviceDirection::Read) {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                DeviceDirection::Read as c_uint,
                (*request).flag
            );
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
        if paddr == PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }

        let lpn = paddr & FORMAT_MASK;
        let ret = {
            let _guard = self.lock.lock().unwrap();
            (self.nand.read)(
                (*request).data as *mut c_char,
                raspberry_block(lpn),
                raspberry_page(lpn),
            )
        };
        if ret != 0 {
            pr_warn!("read error detected {}", self.nand.error_message(ret));
            return ret as isize;
        }

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        (*request).data_len as isize
    }

    /// Erase one segment through libnand and mark it bad on failure. Mirrors
    /// C's `raspberry_erase`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn erase(
        &self,
        dev: *mut device,
        request: *mut device_request,
    ) -> c_int {
        if (*request).direction() != Some(DeviceDirection::Erase) {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                DeviceDirection::Erase as c_uint,
                (*request).flag
            );
            return -libc::EINVAL;
        }

        // C stores `request->paddr.format.block` in a `uint16_t`, so a block
        // number above 0xffff reaches libnand and the bitmap truncated. The
        // 24 bit field and the 16 bit truncation collapse into one mask.
        let segnum = raspberry_block((*request).paddr.lpn) & 0xffff;

        let ret = {
            let _guard = self.lock.lock().unwrap();
            (self.nand.erase)(segnum)
        };
        if ret != 0 {
            pr_warn!("erase fail detected {}", ret);
            mark_bad_segment(dev, segnum);
            return ret;
        }

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        0
    }

    /// Release the bad segment bitmap. Mirrors C's `raspberry_close`, minus the
    /// `raspberry->size = 0` that nothing reads and its `d_private == NULL`
    /// early return, which the vtable path cannot reach.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn close(&self, dev: *mut device) -> c_int {
        if !(*dev).badseg_bitmap.is_null() {
            libc::free((*dev).badseg_bitmap as *mut c_void);
            (*dev).badseg_bitmap = ptr::null_mut();
        }
        0
    }
}

/// Close the raspberry backend, mirroring C's `raspberry_close`.
///
/// # Safety
/// `dev` is `NULL` or a live device.
#[no_mangle]
pub unsafe extern "C" fn ffi_raspberry_close(dev: *mut device) -> c_int {
    if dev.is_null() {
        return -libc::EINVAL;
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.close) {
        Some(close) => close(dev),
        None => -libc::ENODEV,
    }
}

/// Initialize the raspberry module, mirroring C's `raspberry_device_init`: it
/// installs the backend and calls `nand_init`.
///
/// # Safety
/// `dev` is `NULL` or a live device with no backend installed yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_raspberry_device_init(
    dev: *mut device,
    _flags: u64,
) -> c_int {
    if dev.is_null() {
        return -libc::EINVAL;
    }
    init_with(dev, NandApi::system())
}

/// Install `nand` on `dev` and run `nand_init`, the body of
/// [`ffi_raspberry_device_init`]. This is C's order (fill the device, then
/// initialize the library) with its `exception:` label as the failure branch,
/// which runs the exit routine so the backend is released and `nand_free`
/// still runs.
///
/// # Safety
/// `dev` is a live device with no backend installed yet.
unsafe fn init_with(dev: *mut device, nand: NandApi) -> c_int {
    (*dev).install_backend(Raspberry::new(nand));
    (*dev).d_submodule_exit = Some(ffi_raspberry_device_exit);

    let ret = (nand.init)();
    if ret != 0 {
        ffi_raspberry_device_exit(dev);
        return ret;
    }
    0
}

/// Deallocate the raspberry module, mirroring C's `raspberry_device_exit`: run
/// the backend's close, release it, and let its [`Drop`] call `nand_free`.
///
/// # Safety
/// `dev` is `NULL` or a device whose backend is a [`Raspberry`] from
/// [`ffi_raspberry_device_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_raspberry_device_exit(dev: *mut device) -> c_int {
    if dev.is_null() || (*dev).d_private.is_null() {
        return 0;
    }
    let _ = ffi_raspberry_close(dev);
    (*dev).uninstall_backend::<Raspberry>();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{
        AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering,
    };

    use crate::bits::get_bit;
    use crate::device::{device_address, Request};

    /// Number of `u64` words `open` gives the bitmap, `1024 / 64`.
    const WORDS: usize = 16;

    /// Serializes the tests that share the fake's counters, which are process
    /// globals because an `extern "C"` function pointer cannot capture state.
    /// A poisoning panic (the bounds check test) must not fail the others.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    static INIT_RESULT: AtomicI32 = AtomicI32::new(0);
    static ERASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ERASE_LAST: AtomicU32 = AtomicU32::new(u32::MAX);
    /// First block whose erase fails, or `-1` for none.
    static ERASE_FAILS_FROM: AtomicI32 = AtomicI32::new(-1);
    static WRITE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static WRITE_RESULT: AtomicI32 = AtomicI32::new(0);
    static WRITE_ADDRESS: AtomicU64 = AtomicU64::new(u64::MAX);
    static WRITE_FIRST_BYTE: AtomicU32 = AtomicU32::new(0);
    static READ_CALLS: AtomicUsize = AtomicUsize::new(0);
    static READ_RESULT: AtomicI32 = AtomicI32::new(0);
    static READ_ADDRESS: AtomicU64 = AtomicU64::new(u64::MAX);
    static END_RQ_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FREE_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Put the fake back to its defaults. Only the counters the asserts read
    /// are reset, the fail thresholds are set per test.
    fn reset() {
        INIT_RESULT.store(0, Ordering::SeqCst);
        ERASE_CALLS.store(0, Ordering::SeqCst);
        ERASE_LAST.store(u32::MAX, Ordering::SeqCst);
        ERASE_FAILS_FROM.store(-1, Ordering::SeqCst);
        WRITE_CALLS.store(0, Ordering::SeqCst);
        WRITE_RESULT.store(0, Ordering::SeqCst);
        WRITE_ADDRESS.store(u64::MAX, Ordering::SeqCst);
        WRITE_FIRST_BYTE.store(0, Ordering::SeqCst);
        READ_CALLS.store(0, Ordering::SeqCst);
        READ_RESULT.store(0, Ordering::SeqCst);
        READ_ADDRESS.store(u64::MAX, Ordering::SeqCst);
        END_RQ_CALLS.store(0, Ordering::SeqCst);
        FREE_CALLS.store(0, Ordering::SeqCst);
    }

    /// The block and page libnand was called with, packed into one word.
    fn pack(block: u32, page: u32) -> u64 {
        ((block as u64) << 32) | page as u64
    }

    fn unpack(packed: u64) -> (u32, u32) {
        ((packed >> 32) as u32, packed as u32)
    }

    unsafe extern "C" fn fake_init() -> c_int {
        INIT_RESULT.load(Ordering::SeqCst)
    }

    unsafe extern "C" fn fake_free() {
        FREE_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_erase(block: c_uint) -> c_int {
        ERASE_CALLS.fetch_add(1, Ordering::SeqCst);
        ERASE_LAST.store(block, Ordering::SeqCst);
        let fails_from = ERASE_FAILS_FROM.load(Ordering::SeqCst);
        if fails_from >= 0 && block as i32 >= fails_from {
            libc::EIO
        } else {
            0
        }
    }

    unsafe extern "C" fn fake_write(
        buffer: *mut c_char,
        block: c_uint,
        page: c_uint,
    ) -> c_int {
        WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
        WRITE_ADDRESS.store(pack(block, page), Ordering::SeqCst);
        if !buffer.is_null() {
            WRITE_FIRST_BYTE.store(*buffer as u8 as u32, Ordering::SeqCst);
        }
        WRITE_RESULT.load(Ordering::SeqCst)
    }

    unsafe extern "C" fn fake_read(
        buffer: *mut c_char,
        block: c_uint,
        page: c_uint,
    ) -> c_int {
        READ_CALLS.fetch_add(1, Ordering::SeqCst);
        READ_ADDRESS.store(pack(block, page), Ordering::SeqCst);
        if !buffer.is_null() {
            *buffer = 0x5au8 as c_char;
        }
        READ_RESULT.load(Ordering::SeqCst)
    }

    unsafe extern "C" fn fake_read_error_msg(error: c_int) -> *const c_char {
        // A zero code stands in for a library that has no message, which C
        // would hand to `printf("%s")` as `NULL`.
        if error == 0 {
            ptr::null()
        } else {
            c"fake read error".as_ptr()
        }
    }

    unsafe extern "C" fn count_end_rq(_request: *mut device_request) {
        END_RQ_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn fake_api() -> NandApi {
        NandApi {
            init: fake_init,
            free: fake_free,
            write: fake_write,
            read: fake_read,
            erase: fake_erase,
            read_error_msg: fake_read_error_msg,
        }
    }

    fn zeroed_device() -> device {
        unsafe { MaybeUninit::<device>::zeroed().assume_init() }
    }

    /// A device with the backend open and the fake in place.
    fn open_device(raspberry: &Raspberry) -> device {
        let mut dev = zeroed_device();
        unsafe { assert_eq!(raspberry.open(&mut dev, ptr::null(), 0), 0) };
        dev
    }

    fn request(
        direction: DeviceDirection,
        lpn: u32,
        data: *mut c_void,
    ) -> Request {
        let mut request = Request::new().expect("allocate a request");
        request.flag = direction.into();
        request.paddr = device_address { lpn };
        request.data_len = PAGE_SIZE;
        request.data = data;
        request.end_rq = None;
        request
    }

    #[test]
    fn open_fills_the_raspberry_geometry_and_marks_bad_blocks() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        // Blocks 1000 to 1023 fail to erase, the rest succeed.
        ERASE_FAILS_FROM.store(1000, Ordering::SeqCst);

        let mut dev = zeroed_device();
        unsafe { assert_eq!(raspberry.open(&mut dev, ptr::null(), 0), 0) };

        assert_eq!(dev.info.nr_bus, 2);
        assert_eq!(dev.info.nr_chips, 2);
        assert_eq!(dev.info.package.nr_blocks, 1024);
        assert_eq!(dev.info.package.block.nr_pages, 16);
        assert_eq!(dev.page_size(), 2048);
        assert_eq!(dev.blocks_per_segment(), 4);
        assert_eq!(dev.pages_per_segment(), 64);
        // 1024 segments of 4 blocks of 16 pages of 2048 bytes.
        assert_eq!(dev.total_size(), 128 << 20);
        assert_eq!(dev.total_pages(), 65536);

        // C's recovery TODO erases every block once.
        assert_eq!(ERASE_CALLS.load(Ordering::SeqCst), 1024);
        assert_eq!(ERASE_LAST.load(Ordering::SeqCst), 1023);

        let bitmap = unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) };
        assert!(!get_bit(bitmap, 0));
        assert!(!get_bit(bitmap, 999));
        assert!(get_bit(bitmap, 1000));
        assert!(get_bit(bitmap, 1023));

        unsafe { assert_eq!(raspberry.close(&mut dev), 0) };
        assert!(dev.badseg_bitmap.is_null());
    }

    #[test]
    fn write_passes_the_buffer_and_the_raspberry_address_to_libnand() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);

        let mut page = vec![0u8; PAGE_SIZE];
        page[0] = 0x77;
        let mut request = request(
            DeviceDirection::Write,
            (3 << VIEW_BITS) | 7,
            page.as_mut_ptr() as *mut c_void,
        );

        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                PAGE_SIZE as isize
            );
        }
        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 1);
        // Block 3, page 7 through C's `raspberry` view.
        assert_eq!(unpack(WRITE_ADDRESS.load(Ordering::SeqCst)), (3, 7));
        // The caller's buffer pointer reaches libnand unchanged.
        assert_eq!(WRITE_FIRST_BYTE.load(Ordering::SeqCst), 0x77);

        // A library error is returned as it is, not negated.
        WRITE_RESULT.store(libc::EIO, Ordering::SeqCst);
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                libc::EIO as isize
            );
        }

        // C's `addr.lpn = 0` drops the two bits above `format`, so an address
        // that sets them reaches libnand as block 0, page 0. `PADDR_EMPTY` is
        // rejected before this point, so this is the only way there.
        WRITE_RESULT.store(0, Ordering::SeqCst);
        request.paddr = device_address { lpn: 0xc000_0000 };
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                PAGE_SIZE as isize
            );
        }
        assert_eq!(unpack(WRITE_ADDRESS.load(Ordering::SeqCst)), (0, 0));

        // A successful write signals the request, a failed one does not.
        request.end_rq = Some(count_end_rq);
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 0);
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                PAGE_SIZE as isize
            );
        }
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 1);

        WRITE_RESULT.store(libc::EIO, Ordering::SeqCst);
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                libc::EIO as isize
            );
        }
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_second_open_releases_the_previous_bitmap() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        // The first open marks the last 24 blocks bad.
        ERASE_FAILS_FROM.store(1000, Ordering::SeqCst);
        let mut dev = open_device(&raspberry);
        assert!(get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            1000
        ));

        // C overwrites `dev->badseg_bitmap` here and leaks the old one. The
        // port releases it and allocates a fresh, zeroed bitmap, so every
        // erase succeeds this time and the markings are gone.
        ERASE_FAILS_FROM.store(-1, Ordering::SeqCst);
        unsafe {
            assert_eq!(raspberry.open(&mut dev, ptr::null(), 0), 0);
        }
        assert!(!dev.badseg_bitmap.is_null());
        assert!(!get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            1000
        ));

        unsafe {
            assert_eq!(raspberry.close(&mut dev), 0);
        }
        assert!(dev.badseg_bitmap.is_null());
    }

    #[test]
    fn write_rejects_bad_requests_without_calling_libnand() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);
        let mut page = vec![0u8; PAGE_SIZE];

        // NULL data pointer.
        let mut request = request(DeviceDirection::Write, 0, ptr::null_mut());
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                -(libc::ENODATA as isize)
            );
        }

        // Wrong direction.
        request.data = page.as_mut_ptr() as *mut c_void;
        request.flag = DeviceDirection::Read.into();
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        // Unassigned physical address.
        request.flag = DeviceDirection::Write.into();
        request.paddr = device_address { lpn: PADDR_EMPTY };
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        // Short write.
        request.paddr = device_address { lpn: 0 };
        request.data_len = PAGE_SIZE - 1;
        unsafe {
            assert_eq!(
                raspberry.write(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn read_reports_the_libnand_error_and_runs_end_rq_on_success() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);

        let mut page = vec![0u8; PAGE_SIZE];
        let mut request = request(
            DeviceDirection::Read,
            (5 << VIEW_BITS) | 9,
            page.as_mut_ptr() as *mut c_void,
        );
        request.end_rq = Some(count_end_rq);

        READ_RESULT.store(libc::EIO, Ordering::SeqCst);
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                libc::EIO as isize
            );
        }
        // A failed read does not signal the request.
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 0);

        READ_RESULT.store(0, Ordering::SeqCst);
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                PAGE_SIZE as isize
            );
        }
        assert_eq!(READ_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(unpack(READ_ADDRESS.load(Ordering::SeqCst)), (5, 9));
        // The library wrote through the caller's pointer.
        assert_eq!(page[0], 0x5a);
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 1);

        // The read error message, both the library's string and a NULL.
        unsafe {
            assert_eq!(
                raspberry.nand.error_message(libc::EIO),
                "fake read error"
            );
            assert_eq!(raspberry.nand.error_message(0), "(null)");
        }
    }

    #[test]
    fn read_rejects_bad_requests_without_calling_libnand() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);
        let mut page = vec![0u8; PAGE_SIZE];

        let mut request = request(DeviceDirection::Read, 0, ptr::null_mut());
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                -(libc::ENODATA as isize)
            );
        }

        request.data = page.as_mut_ptr() as *mut c_void;
        request.flag = DeviceDirection::Write.into();
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        request.flag = DeviceDirection::Read.into();
        request.data_len = PAGE_SIZE + 1;
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        request.data_len = PAGE_SIZE;
        request.paddr = device_address { lpn: PADDR_EMPTY };
        unsafe {
            assert_eq!(
                raspberry.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
        }

        assert_eq!(READ_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn erase_truncates_the_block_and_marks_the_bitmap() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);

        // The open pass erased every block successfully.
        assert!(!get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            0
        ));

        ERASE_FAILS_FROM.store(0, Ordering::SeqCst);
        let mut request =
            request(DeviceDirection::Erase, 4 << VIEW_BITS, ptr::null_mut());
        unsafe {
            assert_eq!(raspberry.erase(&mut dev, request.as_ptr()), libc::EIO);
        }
        assert_eq!(ERASE_LAST.load(Ordering::SeqCst), 4);
        assert!(get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            4
        ));

        // C stores the block in a `uint16_t`, so 0x1_0000 arrives as 0.
        request.paddr = device_address { lpn: 0x1_0000 << VIEW_BITS };
        unsafe {
            assert_eq!(raspberry.erase(&mut dev, request.as_ptr()), libc::EIO);
        }
        assert_eq!(ERASE_LAST.load(Ordering::SeqCst), 0);
        assert!(get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            0
        ));

        // A non-erase request is refused before libnand is called.
        request.flag = DeviceDirection::Write.into();
        let calls = ERASE_CALLS.load(Ordering::SeqCst);
        unsafe {
            assert_eq!(
                raspberry.erase(&mut dev, request.as_ptr()),
                -libc::EINVAL
            );
        }
        assert_eq!(ERASE_CALLS.load(Ordering::SeqCst), calls);

        // A successful erase returns 0, signals the request and leaves the
        // bitmap alone. Block 4 keeps the bad bit the failed erase set, which
        // only a fresh `open` clears, exactly as in C.
        ERASE_FAILS_FROM.store(-1, Ordering::SeqCst);
        request.flag = DeviceDirection::Erase.into();
        request.paddr = device_address { lpn: 4 << VIEW_BITS };
        request.end_rq = Some(count_end_rq);
        unsafe {
            assert_eq!(raspberry.erase(&mut dev, request.as_ptr()), 0);
        }
        assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 1);
        assert!(get_bit(
            unsafe { slice::from_raw_parts(dev.badseg_bitmap, WORDS) },
            4
        ));
    }

    #[test]
    fn erase_without_open_skips_the_bitmap() {
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        ERASE_FAILS_FROM.store(0, Ordering::SeqCst);
        let mut dev = zeroed_device();
        let request = request(DeviceDirection::Erase, 0, ptr::null_mut());

        // `badseg_bitmap` is NULL and `nr_segments` is 0, where C would
        // dereference the NULL bitmap.
        unsafe {
            assert_eq!(raspberry.erase(&mut dev, request.as_ptr()), libc::EIO);
        }
        assert!(dev.badseg_bitmap.is_null());
    }

    #[test]
    #[should_panic]
    fn a_block_past_the_bitmap_panics_instead_of_corrupting() {
        // C's `set_bit(dev->badseg_bitmap, segnum)` writes past the 16 word
        // bitmap for a block above 1023. The Rust port bounds-checks.
        let _guard = lock_tests();
        reset();
        let raspberry = Raspberry::new(fake_api());
        let mut dev = open_device(&raspberry);
        ERASE_FAILS_FROM.store(0, Ordering::SeqCst);
        let request =
            request(DeviceDirection::Erase, 1024 << VIEW_BITS, ptr::null_mut());

        unsafe { raspberry.erase(&mut dev, request.as_ptr()) };
    }

    #[test]
    fn close_releases_the_bitmap_and_drop_calls_nand_free() {
        let _guard = lock_tests();
        reset();
        let mut dev = zeroed_device();
        let raspberry = Raspberry::new(fake_api());
        unsafe { assert_eq!(raspberry.open(&mut dev, ptr::null(), 0), 0) };

        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 0);
        unsafe {
            assert_eq!(raspberry.close(&mut dev), 0);
            // A second close has nothing to release.
            assert_eq!(raspberry.close(&mut dev), 0);
        }
        assert!(dev.badseg_bitmap.is_null());

        drop(raspberry);
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn init_installs_the_backend_and_runs_nand_init() {
        let _guard = lock_tests();
        reset();
        let mut dev = zeroed_device();
        unsafe {
            assert_eq!(init_with(&mut dev, fake_api()), 0);
            assert!(!dev.d_op.is_null());
            assert!(!dev.d_private.is_null());
            assert!((*dev.d_op).open.is_some());

            // The exit routine reaches the backend through the vtable.
            assert_eq!(ffi_raspberry_device_exit(&mut dev), 0);
        }
        assert!(dev.d_private.is_null());
        assert!(dev.d_op.is_null());
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_nand_init_runs_the_exit_routine() {
        let _guard = lock_tests();
        reset();
        INIT_RESULT.store(-libc::ENODEV, Ordering::SeqCst);
        let mut dev = zeroed_device();
        unsafe {
            assert_eq!(init_with(&mut dev, fake_api()), -libc::ENODEV);
        }
        // C's `exception:` path released the backend, and `nand_free` ran.
        assert!(dev.d_private.is_null());
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ffi_entry_points_accept_a_null_device() {
        unsafe {
            assert_eq!(
                ffi_raspberry_device_init(ptr::null_mut(), 0),
                -libc::EINVAL
            );
            assert_eq!(ffi_raspberry_device_exit(ptr::null_mut()), 0);
            assert_eq!(ffi_raspberry_close(ptr::null_mut()), -libc::EINVAL);

            // A device with no backend has no close to dispatch to.
            let mut dev = zeroed_device();
            assert_eq!(ffi_raspberry_close(&mut dev), -libc::ENODEV);
        }
    }

    #[test]
    fn the_raspberry_view_is_the_low_six_bits_plus_the_block() {
        // C's union in this build: bus 1 bit, chip 1 bit, page 4 bits, block
        // 24 bits, so `addr.raspberry.page` is the low 6 bits.
        assert_eq!(raspberry_page(0x3f), 0x3f);
        assert_eq!(raspberry_block(0x3f), 0);
        assert_eq!(raspberry_page(0x40), 0);
        assert_eq!(raspberry_block(0x40), 1);
        assert_eq!(raspberry_page(PADDR_EMPTY), PAGE_MASK);
        assert_eq!(raspberry_block(PADDR_EMPTY), (PADDR_EMPTY >> VIEW_BITS));
        // The erase path keeps the low 16 bits of the block field.
        assert_eq!(raspberry_block(PADDR_EMPTY) & 0xffff, 0xffff);
    }
}
