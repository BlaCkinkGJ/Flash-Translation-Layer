//! Module dispatch ported from `include/module.h` + `interface/module.c`.
//!
//! `module_init` is the library's public entry point: it builds the flash
//! handle, then hands it to the submodule selected by `modnum`. In this repo
//! the flag word doubles as the device module number, because
//! `page_ftl_module_init` reads the NAND backend out of `flags` (`RAMDISK_MODULE`
//! in `integration-test.c`). The flash layer does not look at it.
//!
//! The submodule table has C's single entry (`PAGE_FTL_MODULE = 0`). C fills it
//! with `page_ftl_module_init`, which is the bridge Task 13b writes after this
//! task lands, so the entry stays empty here and every dispatch reports
//! `-ENODEV` until then. C's `interface/module.c` keeps its own table and stays
//! the live path for C callers until the Task 14 cutover.
//!
//! Symbols carry the `ffi_` prefix so they can coexist with `interface/module.c`
//! while both implementations are linked.
//!
//! Deliberate deviations from the C original:
//!
//! - An out of range `modnum` returns `-EINVAL`. C indexes `submodule_init[]`
//!   unchecked, so it reads a pointer off the table.
//! - A module number with no submodule returns `-ENODEV`. C calls the `NULL`
//!   entry.
//! - A `NULL` flash output pointer returns `-EINVAL` and a `NULL` flash handle
//!   to [`ffi_module_exit`] returns `-EINVAL`. C dereferences both.
//! - A failing submodule init releases the flash handle and clears the output
//!   pointer. C returns the error and leaks the handle.
//! - The `modnum` range check happens before the flash handle is allocated, so
//!   a bad module number costs no allocation.

use core::ffi::c_int;
use core::ptr;

use crate::flash::{ffi_flash_module_exit, ffi_flash_module_init, flash_device};
use crate::{pr_err, pr_info};

/// C's `PAGE_FTL_MODULE`, the only module number the header defines.
pub const PAGE_FTL_MODULE: c_int = 0;

/// Number of entries in C's `submodule_init[]`.
pub const MODULE_COUNT: usize = 1;

/// Element type of C's `submodule_init[]`.
pub type SubmoduleInitFn = unsafe extern "C" fn(*mut flash_device, u64) -> c_int;

/// C's `submodule_init[]`, indexed by the `modnum` argument of
/// [`ffi_module_init`]. The page FTL entry arrives with Task 13b.
static SUBMODULE_INIT: [Option<SubmoduleInitFn>; MODULE_COUNT] = [None];

/// Dispatch through `table`, the body of [`ffi_module_init`].
///
/// The table is a parameter so the dispatch can be exercised against a mock
/// submodule while the real table is still empty.
///
/// # Safety
/// `flash` is `NULL` or a writable pointer, and the table's entries follow the
/// submodule init contract of C's `submodule_init[]`.
unsafe fn module_init_with(
    table: &[Option<SubmoduleInitFn>],
    modnum: c_int,
    flash: *mut *mut flash_device,
    flags: u64,
) -> c_int {
    if flash.is_null() {
        pr_err!("flash output pointer is NULL");
        return -libc::EINVAL;
    }

    if modnum < 0 || modnum.unsigned_abs() as usize >= table.len() {
        // C indexes submodule_init[modnum] unchecked.
        pr_err!("unknown module (modnum: {})", modnum);
        return -libc::EINVAL;
    }

    let ret = ffi_flash_module_init(flash, flags);
    if ret != 0 {
        pr_err!("flash initialize failed");
        return ret;
    }
    pr_info!("flash initialize success");

    let Some(submodule_init) = table[modnum as usize] else {
        pr_err!("no submodule for this module (modnum: {})", modnum);
        destroy_flash(flash);
        return -libc::ENODEV;
    };

    let ret = submodule_init(*flash, flags);
    if ret != 0 {
        pr_err!("submodule initialize failed");
        destroy_flash(flash);
        return ret;
    }
    pr_info!("submodule initialize success");

    0
}

/// Release the flash handle a failed dispatch built.
///
/// # Safety
/// `flash` points to a handle from [`ffi_flash_module_init`] with no submodule
/// state installed.
unsafe fn destroy_flash(flash: *mut *mut flash_device) {
    ffi_flash_module_exit(*flash);
    *flash = ptr::null_mut();
}

/// Initialize a module, mirroring C's `module_init`.
///
/// Returns `0` on success and a negative value on failure, in which case the
/// flash handle is released and `*flash` is `NULL`.
///
/// # Safety
/// `flash` is `NULL` or points to a writable `*mut flash_device`.
#[no_mangle]
pub unsafe extern "C" fn ffi_module_init(
    modnum: c_int,
    flash: *mut *mut flash_device,
    flags: u64,
) -> c_int {
    module_init_with(&SUBMODULE_INIT, modnum, flash, flags)
}

/// Free the module's resources, mirroring C's `module_exit`.
///
/// The submodule teardown runs first and a failure there stops the sequence, so
/// the caller keeps a usable flash handle to retry with, exactly as in C.
///
/// # Safety
/// `flash` came from [`ffi_module_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_module_exit(flash: *mut flash_device) -> c_int {
    if flash.is_null() {
        pr_err!("flash pointer is NULL");
        return -libc::EINVAL;
    }

    if let Some(submodule_exit) = (*flash).f_submodule_exit {
        let ret = submodule_exit(flash);
        if ret != 0 {
            pr_err!("submodule resources free failed");
            return ret;
        }
        pr_info!("submodule deallocates success");
    }

    let ret = ffi_flash_module_exit(flash);
    if ret != 0 {
        pr_err!("flash resources free failed");
        return ret;
    }
    pr_info!("flash deallocates success");

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;
    use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

    const MARKER: usize = 0x5a5a;

    static INIT_SEEN: AtomicPtr<flash_device> = AtomicPtr::new(ptr::null_mut());
    static INIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INIT_ERROR: AtomicUsize = AtomicUsize::new(0);
    static EXIT_ERROR: AtomicUsize = AtomicUsize::new(0);

    /// The mock submodule reports through process wide counters and error
    /// slots, so the tests that drive it run one at a time.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Clear the mocks. The guard keeps the tests that share these counters
    /// from running at the same time, so hold it for the whole test.
    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = lock();
        INIT_SEEN.store(ptr::null_mut(), Ordering::SeqCst);
        INIT_CALLS.store(0, Ordering::SeqCst);
        EXIT_CALLS.store(0, Ordering::SeqCst);
        INIT_ERROR.store(0, Ordering::SeqCst);
        EXIT_ERROR.store(0, Ordering::SeqCst);
        guard
    }

    unsafe extern "C" fn mock_submodule_init(flash: *mut flash_device, _flags: u64) -> c_int {
        INIT_CALLS.fetch_add(1, Ordering::SeqCst);
        INIT_SEEN.store(flash, Ordering::SeqCst);

        let error = INIT_ERROR.load(Ordering::SeqCst);
        if error != 0 {
            return -(error as c_int);
        }

        (*flash).set_private(MARKER as *mut core::ffi::c_void);
        (*flash).set_submodule_exit(Some(mock_submodule_exit));
        0
    }

    unsafe extern "C" fn mock_submodule_exit(_flash: *mut flash_device) -> c_int {
        EXIT_CALLS.fetch_add(1, Ordering::SeqCst);
        -(EXIT_ERROR.load(Ordering::SeqCst) as c_int)
    }

    /// A table holding only the mock submodule, the stand-in for C's
    /// `submodule_init[]` until Task 13b fills it.
    static MOCK_TABLE: [Option<SubmoduleInitFn>; MODULE_COUNT] = [Some(mock_submodule_init)];

    #[test]
    fn init_dispatches_the_submodule_and_exit_runs_its_teardown() {
        let _guard = reset();
        let mut flash: *mut flash_device = ptr::null_mut();

        let ret = unsafe { module_init_with(&MOCK_TABLE, PAGE_FTL_MODULE, &mut flash, 0) };
        assert_eq!(ret, 0, "module init");
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(INIT_SEEN.load(Ordering::SeqCst), flash);
        unsafe { assert_eq!((*flash).private(), MARKER as *mut core::ffi::c_void) };

        assert_eq!(unsafe { ffi_module_exit(flash) }, 0);
        assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn init_reports_a_missing_submodule_without_leaking_the_flash() {
        let _guard = reset();
        let mut flash: *mut flash_device = ptr::null_mut();

        let ret = unsafe { module_init_with(&[None], PAGE_FTL_MODULE, &mut flash, 0) };
        assert_eq!(ret, -libc::ENODEV);
        assert!(flash.is_null(), "failed dispatch clears the output pointer");
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn init_rejects_an_out_of_range_module_number() {
        let _guard = reset();
        let mut flash: *mut flash_device = ptr::null_mut();

        let ret = unsafe { module_init_with(&MOCK_TABLE, MODULE_COUNT as c_int, &mut flash, 0) };
        assert_eq!(ret, -libc::EINVAL);
        assert!(flash.is_null());
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 0);

        let ret = unsafe { module_init_with(&MOCK_TABLE, -1, &mut flash, 0) };
        assert_eq!(ret, -libc::EINVAL);
        assert!(flash.is_null());
    }

    #[test]
    fn init_rejects_a_null_output_pointer() {
        let _guard = reset();
        assert_eq!(
            unsafe { ffi_module_init(PAGE_FTL_MODULE, ptr::null_mut(), 0) },
            -libc::EINVAL
        );
    }

    #[test]
    fn a_failing_submodule_init_is_propagated_and_the_flash_is_released() {
        let _guard = reset();
        INIT_ERROR.store(libc::ENOMEM as usize, Ordering::SeqCst);
        let mut flash: *mut flash_device = ptr::null_mut();

        let ret = unsafe { module_init_with(&MOCK_TABLE, PAGE_FTL_MODULE, &mut flash, 0) };
        assert_eq!(ret, -libc::ENOMEM);
        assert!(flash.is_null());
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_submodule_exit_stops_before_the_flash_is_freed() {
        let _guard = reset();
        let mut flash: *mut flash_device = ptr::null_mut();
        assert_eq!(
            unsafe { module_init_with(&MOCK_TABLE, PAGE_FTL_MODULE, &mut flash, 0) },
            0
        );

        EXIT_ERROR.store(libc::EIO as usize, Ordering::SeqCst);
        assert_eq!(unsafe { ffi_module_exit(flash) }, -libc::EIO);
        assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1);

        // C keeps the handle for a retry, so release it the same way here.
        EXIT_ERROR.store(0, Ordering::SeqCst);
        unsafe { (*flash).set_submodule_exit(None) };
        assert_eq!(unsafe { ffi_module_exit(flash) }, 0);
    }

    #[test]
    fn exit_rejects_a_null_flash_handle() {
        assert_eq!(unsafe { ffi_module_exit(ptr::null_mut()) }, -libc::EINVAL);
    }
}
