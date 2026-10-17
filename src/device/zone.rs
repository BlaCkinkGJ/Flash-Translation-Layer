//! Stub `zone` backend, the Rust port of `device/zone/zone.c`.
//!
//! The real backend opens a zoned block device through libzbd, which is not
//! available here, so every [`DeviceOperations`] method returns `-ENOSYS` and
//! the init routine only installs the stub backend. The geometry C's
//! `zone_open` fills in, and the zoned build's bit counts (`3/3/5/21`), land
//! with the hardware implementation.

use core::ffi::{c_char, c_int};

use libc::{EINVAL, ENOSYS};

use crate::device::{device, device_request, DeviceOperations};

/// Stub zone backend. See the module docs.
pub struct ZoneBackend;

impl DeviceOperations for ZoneBackend {
    unsafe fn open(&self, _dev: *mut device, _name: *const c_char, _flags: c_int) -> c_int {
        -ENOSYS
    }

    unsafe fn write(&self, _dev: *mut device, _request: *mut device_request) -> isize {
        -(ENOSYS as isize)
    }

    unsafe fn read(&self, _dev: *mut device, _request: *mut device_request) -> isize {
        -(ENOSYS as isize)
    }

    unsafe fn erase(&self, _dev: *mut device, _request: *mut device_request) -> c_int {
        -ENOSYS
    }

    unsafe fn close(&self, _dev: *mut device) -> c_int {
        -ENOSYS
    }
}

/// Init routine of the zone module, C's `zone_device_init`.
///
/// # Safety
/// `dev` is `NULL` or a live device with no backend installed yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_zone_device_init(dev: *mut device, _flags: u64) -> c_int {
    if dev.is_null() {
        return -EINVAL;
    }
    (*dev).install_backend(ZoneBackend);
    (*dev).d_submodule_exit = Some(ffi_zone_device_exit);
    0
}

/// Exit routine of the zone module, C's `zone_device_exit`.
///
/// # Safety
/// `dev` is `NULL` or a device whose backend is a [`ZoneBackend`] from
/// [`ffi_zone_device_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_zone_device_exit(dev: *mut device) -> c_int {
    if dev.is_null() || (*dev).d_private.is_null() {
        return 0;
    }
    (*dev).uninstall_backend::<ZoneBackend>();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::MaybeUninit;
    use core::ptr;

    fn zeroed_device() -> device {
        unsafe { MaybeUninit::<device>::zeroed().assume_init() }
    }

    #[test]
    fn every_operation_returns_enosys_and_init_wires_the_backend() {
        let backend = ZoneBackend;
        unsafe {
            assert_eq!(backend.open(ptr::null_mut(), ptr::null(), 0), -ENOSYS);
            assert_eq!(
                backend.write(ptr::null_mut(), ptr::null_mut()),
                -(ENOSYS as isize)
            );
            assert_eq!(
                backend.read(ptr::null_mut(), ptr::null_mut()),
                -(ENOSYS as isize)
            );
            assert_eq!(backend.erase(ptr::null_mut(), ptr::null_mut()), -ENOSYS);
            assert_eq!(backend.close(ptr::null_mut()), -ENOSYS);

            assert_eq!(ffi_zone_device_init(ptr::null_mut(), 0), -EINVAL);
            assert_eq!(ffi_zone_device_exit(ptr::null_mut()), 0);

            let mut dev = zeroed_device();
            assert_eq!(ffi_zone_device_init(&mut dev, 0), 0);
            assert!(!dev.d_op.is_null());
            assert!(!dev.d_private.is_null());
            assert_eq!(ffi_zone_device_exit(&mut dev), 0);
            assert!(dev.d_op.is_null());
            assert!(dev.d_private.is_null());
        }
    }
}
