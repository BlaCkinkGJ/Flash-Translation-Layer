//! Flash control layer ported from `include/flash.h` + `interface/flash.c`.
//!
//! `flash_device` is the handle `module_init` hands to a caller: it carries the
//! vtable of the submodule that took over the flash (`f_op`), that submodule's
//! private data (`f_private`) and its teardown (`f_submodule_exit`). The struct
//! is a `#[repr(C)]` mirror, so the field order and offsets are the contract.
//! They are pinned here by `layout_matches_c` and on the C side by the
//! `FLASH_STATIC_ASSERT`s in `include/flash.h`.
//!
//! The page FTL installs its own vtable through `f_op` (C's `__page_fops` in
//! `ftl/page/page-interface.c`), so this module owns the struct, the allocation
//! pair and the accessors. `crate::ftl::page_interface::ffi_page_ftl_module_init`
//! is the installed bridge.
//!
//! Symbols carry the `ffi_` prefix so they can coexist with `interface/flash.c`
//! while both implementations are linked. The cutover is Task 14.
//!
//! Deliberate deviations from the C original:
//!
//! - `ffi_flash_module_init` rejects a `NULL` output pointer with `-EINVAL`. C
//!   dereferences it.
//! - `ffi_flash_module_init` cannot fail with `-ENOMEM`: `Box` aborts on OOM, as
//!   in `src/list.rs` and `src/device/mod.rs`. C's failure path would call
//!   `flash_module_exit(NULL)`, which is a logged no-op.
//! - `flags` is accepted and ignored, as C's `(void)flags` documents. The flag
//!   word is consumed by the submodule, not by the flash layer.

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;
use libc::{off_t, size_t, ssize_t};

use crate::pr_err;

/// C's `FLASH_DEFAULT_FLAG`, the only flag the header defines.
pub const FLASH_DEFAULT_FLAG: u64 = 0;

/// Mirrors `struct flash_device`.
///
/// A `NULL` `f_op` means no submodule has installed itself yet, which is the
/// state `flash_module_init` leaves behind.
#[repr(C)]
pub struct flash_device {
    pub f_op: *const flash_operations,
    pub f_private: *mut c_void,
    pub f_submodule_exit: Option<unsafe extern "C" fn(*mut flash_device) -> c_int>,
}

/// Mirrors `struct flash_operations`, the vtable a submodule installs into
/// [`flash_device::f_op`].
///
/// `open` / `write` / `read` / `close` are the byte oriented interface the
/// examples and `integration-test.c` drive. The C header declares `ioctl`
/// variadic (`include/flash.h:59-60`) and the page FTL implements it for
/// `PAGE_FTL_IOCTL_TRIM`, but no call site in this repository ever passes a
/// variadic argument: `integration-test.c:162` and `example/rw_example.c:32`
/// both pass the request alone. Stable Rust cannot define a variadic function,
/// so the slot is narrowed to that one shape, which keeps C's pointer sized
/// field in place (`layout_matches_c`) and lets the Rust page FTL implement
/// `ioctl` like the other four. C built tables keep their variadic pointer and
/// the C callers keep reaching it, since only Rust defines the type.
#[repr(C)]
pub struct flash_operations {
    pub open: Option<unsafe extern "C" fn(*mut flash_device, *const c_char, c_int) -> c_int>,
    pub write:
        Option<unsafe extern "C" fn(*mut flash_device, *mut c_void, size_t, off_t) -> ssize_t>,
    pub read:
        Option<unsafe extern "C" fn(*mut flash_device, *mut c_void, size_t, off_t) -> ssize_t>,
    pub ioctl: Option<unsafe extern "C" fn(*mut flash_device, c_uint) -> c_int>,
    pub close: Option<unsafe extern "C" fn(*mut flash_device) -> c_int>,
}

impl flash_device {
    /// Install a submodule's vtable (C's `flash->f_op = &__page_fops`).
    ///
    /// # Safety
    /// `ops` outlives this flash device, which is how C's `static const`
    /// tables behave.
    pub unsafe fn set_operations(&mut self, ops: *const flash_operations) {
        self.f_op = ops;
    }

    /// Hand the submodule's own state to the flash handle (C's `f_private`).
    ///
    /// # Safety
    /// `private` is the pointer the submodule will hand back to its own
    /// operations.
    pub unsafe fn set_private(&mut self, private: *mut c_void) {
        self.f_private = private;
    }

    /// Install the submodule's teardown, run by [`crate::module::ffi_module_exit`]
    /// and by C's `module_exit`.
    ///
    /// # Safety
    /// `exit` accepts this flash device and releases only what the submodule
    /// allocated.
    pub unsafe fn set_submodule_exit(
        &mut self,
        exit: Option<unsafe extern "C" fn(*mut flash_device) -> c_int>,
    ) {
        self.f_submodule_exit = exit;
    }
}

/// Initialize the flash layer, mirroring C's `flash_module_init`.
///
/// Allocates the flash handle and leaves `f_op`, `f_private` and
/// `f_submodule_exit` empty for the submodule to fill. `flags` is ignored, as
/// in C.
///
/// Returns `0` on success and a negative value on failure, in which case
/// `*flash` is left untouched.
///
/// # Safety
/// `flash` is `NULL` or points to a writable `*mut flash_device`.
#[no_mangle]
pub unsafe extern "C" fn ffi_flash_module_init(
    flash: *mut *mut flash_device,
    _flags: u64,
) -> c_int {
    if flash.is_null() {
        pr_err!("flash output pointer is NULL");
        return -libc::EINVAL;
    }

    let handle: Box<flash_device> = Box::new(flash_device {
        f_op: ptr::null(),
        f_private: ptr::null_mut(),
        f_submodule_exit: None,
    });
    *flash = Box::into_raw(handle);

    0
}

/// Release the flash handle, mirroring C's `flash_module_exit`.
///
/// A `NULL` handle is a logged no-op that returns `0`, exactly as in C. The
/// submodule's own teardown is C's `module_exit` job, not this function's.
///
/// # Safety
/// `flash` came from [`ffi_flash_module_init`] and no submodule is using it.
#[no_mangle]
pub unsafe extern "C" fn ffi_flash_module_exit(flash: *mut flash_device) -> c_int {
    if flash.is_null() {
        pr_err!("flash pointer is null detected");
        return 0;
    }

    drop(Box::from_raw(flash));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};
    use core::ptr;

    const PTR: usize = size_of::<*const c_void>();

    fn init_flash() -> *mut flash_device {
        let mut flash: *mut flash_device = ptr::null_mut();
        let ret = unsafe { ffi_flash_module_init(&mut flash, FLASH_DEFAULT_FLAG) };
        assert_eq!(ret, 0, "flash module init");
        assert!(!flash.is_null());
        flash
    }

    #[test]
    fn layout_matches_c() {
        assert_eq!(offset_of!(flash_device, f_op), 0);
        assert_eq!(offset_of!(flash_device, f_private), PTR);
        assert_eq!(offset_of!(flash_device, f_submodule_exit), 2 * PTR);
        assert_eq!(size_of::<flash_device>(), 3 * PTR);

        assert_eq!(offset_of!(flash_operations, open), 0);
        assert_eq!(offset_of!(flash_operations, write), PTR);
        assert_eq!(offset_of!(flash_operations, read), 2 * PTR);
        assert_eq!(offset_of!(flash_operations, ioctl), 3 * PTR);
        assert_eq!(offset_of!(flash_operations, close), 4 * PTR);
        assert_eq!(size_of::<flash_operations>(), 5 * PTR);
    }

    #[test]
    fn init_hands_back_an_empty_handle_and_exit_releases_it() {
        let flash = init_flash();

        unsafe {
            assert!((*flash).f_op.is_null());
            assert!((*flash).f_private.is_null());
            assert!((*flash).f_submodule_exit.is_none());
            assert_eq!(ffi_flash_module_exit(flash), 0);
        }
    }

    #[test]
    fn init_rejects_a_null_output_pointer() {
        assert_eq!(
            unsafe { ffi_flash_module_init(ptr::null_mut(), FLASH_DEFAULT_FLAG) },
            -libc::EINVAL
        );
    }

    #[test]
    fn exit_tolerates_a_null_device() {
        assert_eq!(unsafe { ffi_flash_module_exit(ptr::null_mut()) }, 0);
    }

    unsafe extern "C" fn mark_exit(flash: *mut flash_device) -> c_int {
        assert!(!flash.is_null());
        42
    }

    #[test]
    fn accessors_carry_the_submodule_state() {
        static OPS: flash_operations = flash_operations {
            open: None,
            write: None,
            read: None,
            ioctl: None,
            close: None,
        };
        let flash = init_flash();
        let marker = 0x5a5a_usize as *mut c_void;

        unsafe {
            (*flash).set_operations(&OPS);
            (*flash).set_private(marker);
            (*flash).set_submodule_exit(Some(mark_exit));

            assert_eq!((*flash).f_op, &OPS as *const flash_operations);
            assert_eq!((*flash).f_private, marker);

            let exit = (*flash).f_submodule_exit.expect("submodule exit installed");
            assert_eq!(exit(flash), 42);

            assert_eq!(ffi_flash_module_exit(flash), 0);
        }
    }
}
