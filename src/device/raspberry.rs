//! Stub `raspberry` backend, the Rust port of
//! `device/raspberry/raspberry.c`.
//!
//! The real backend calls the WiringPi/libnand NAND API, which is not available
//! here, so every [`DeviceOperations`] method returns `-ENOSYS` and the init
//! routine only installs the stub backend. The geometry C's `raspberry_open`
//! fills in, and the raspberry build's bit counts (`1/1/4/24` with a 2048 byte
//! page), land with the hardware implementation.
//!
//! One deliberate difference from C: `raspberry_device_init` calls `nand_init()`
//! and returns its error, so C's module init fails when the NAND stack is
//! missing. The stub cannot call it, so init returns `0` and the first `open`
//! reports `-ENOSYS` instead.

use core::ffi::{c_char, c_int};

use libc::ENOSYS;

use crate::device::{device, device_request, DeviceOperations};

/// Stub raspberry backend. See the module docs.
pub struct RaspberryBackend;

impl DeviceOperations for RaspberryBackend {
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

/// Init routine of the raspberry module, C's `raspberry_device_init`.
///
/// # Safety
/// C's `submodule_init[]` contract: `dev` is a zeroed device.
#[no_mangle]
pub unsafe extern "C" fn ffi_raspberry_device_init(dev: *mut device, _flags: u64) -> c_int {
    (*dev).install_backend(RaspberryBackend);
    (*dev).d_submodule_exit = Some(ffi_raspberry_device_exit);
    0
}

/// Exit routine of the raspberry module, C's `raspberry_device_exit`.
///
/// # Safety
/// `dev` was initialized by [`ffi_raspberry_device_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_raspberry_device_exit(dev: *mut device) -> c_int {
    (*dev).uninstall_backend::<RaspberryBackend>();
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
        let backend = RaspberryBackend;
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

            let mut dev = zeroed_device();
            assert_eq!(ffi_raspberry_device_init(&mut dev, 0), 0);
            assert!(!dev.d_op.is_null());
            assert!(!dev.d_private.is_null());
            assert_eq!(ffi_raspberry_device_exit(&mut dev), 0);
            assert!(dev.d_op.is_null());
            assert!(dev.d_private.is_null());
        }
    }
}
