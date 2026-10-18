//! Read/write example over the ramdisk backend, the Rust port of
//! `example/rw_example.c`.
//!
//! The program is the reference a reader copies when writing their own. It
//! brings the page FTL up over the ramdisk with [`ffi_module_init`], opens the
//! device, writes and reads an `int` across `8192 * 10` offsets, collects
//! garbage when the C predicate fires, then closes the device and tears the
//! module down with [`ffi_module_exit`].
//!
//! It is the C program's behaviour, kept call for call. The `ffi_` entry points
//! it keeps calling are the C ABI, and the open / write / read / ioctl / close
//! hooks it reaches through `flash->f_op` are the crate's own
//! [`flash_device`]. The program re-declares neither `struct flash_device` nor
//! `f_op`: `crate::flash` owns both, so the layout stays a single contract.
//!
//! Deliberate deviations from the C original:
//!
//! - C's `sector = rand() % (1 << 31)` overflows the 32-bit `int` divisor to
//!   `INT_MIN`, and `rand()` never reaches `2^31`, so the modulo returns the
//!   draw unchanged. The Rust form states the divisor as `1i64 << 31` and gets
//!   the same value without the overflow.
//! - C prints through its own `pr_info` and the port prints through the crate's
//!   JSON log facade, so the line format differs while the values do not.
//! - The ioctl predicate is C's `i % 8192 * 5 == 0`, which `%` and `*` bind
//!   left to right as `(i % 8192) * 5`, and the port writes it the same way.

use core::ffi::{c_char, c_int, c_uint};
use core::mem::size_of;
use core::ptr;

use ftl_rust::device::DeviceModule;
use ftl_rust::flash::flash_device;
use ftl_rust::ftl::PAGE_FTL_IOCTL_TRIM;
use ftl_rust::module::{ffi_module_exit, ffi_module_init, PAGE_FTL_MODULE};
use ftl_rust::pr_info;
use libc::off_t;

/// C's `char buffer[8192]`, the transfer buffer the example drives.
const BUFFER_SIZE: usize = 8192;

/// C's `8192 * 10`, the number of write/read rounds.
const ROUNDS: usize = 8192 * 10;

/// C's `sizeof(int)`, the bytes each round writes and reads back.
const INT_SIZE: usize = size_of::<c_int>();

/// C's `O_CREAT | O_RDWR`, the open flags.
const OPEN_FLAGS: c_int = libc::O_CREAT | libc::O_RDWR;

fn main() {
    let mut flash: *mut flash_device = ptr::null_mut();
    // C asserts `ffi_module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE)`
    // returns 0, where `RAMDISK_MODULE` is `DeviceModule::Ramdisk`.
    let flags = DeviceModule::Ramdisk as u64;
    let ret = unsafe { ffi_module_init(PAGE_FTL_MODULE, &mut flash, flags) };
    assert_eq!(ret, 0, "module init failed");
    pr_info!("module initialize");

    // Safety: `flash` is the live handle the module init just returned, and the
    // page FTL installs all five hooks.
    let f_op = unsafe { &*(*flash).f_op };
    let open = f_op.open.expect("the page FTL installs open");
    let write = f_op.write.expect("the page FTL installs write");
    let read = f_op.read.expect("the page FTL installs read");
    let ioctl = f_op.ioctl.expect("the page FTL installs ioctl");
    let close = f_op.close.expect("the page FTL installs close");

    // C opens with a `NULL` name, which the ramdisk ignores, and does not read
    // the return value.
    unsafe { open(flash, ptr::null::<c_char>(), OPEN_FLAGS) };

    let mut buffer = [0u8; BUFFER_SIZE];
    for i in 0..ROUNDS {
        // C's `int num = i * 2`, stamped into the start of the buffer.
        let num: c_int = (i * 2) as c_int;
        buffer.fill(0);
        buffer[..INT_SIZE].copy_from_slice(&num.to_ne_bytes());
        let sector = rand_sector();

        // Safety: `flash` carries the opened page FTL, and the first
        // `INT_SIZE` bytes of `buffer` are readable.
        unsafe { write(flash, buffer.as_mut_ptr().cast(), INT_SIZE, sector) };
        pr_info!("write value: {}", num);

        buffer.fill(0);
        // Safety: same handle, and the first `INT_SIZE` bytes of `buffer` are
        // writable.
        unsafe { read(flash, buffer.as_mut_ptr().cast(), INT_SIZE, sector) };
        let value = read_int(&buffer);
        pr_info!("read value: {}", value);

        // C's `i % 8192 * 5 == 0`, which `%` and `*` bind left to right as
        // `(i % 8192) * 5`.
        if (i % 8192) * 5 == 0 {
            // Safety: same handle, and the ioctl the page FTL installed takes
            // the FTL lock itself.
            unsafe { ioctl(flash, PAGE_FTL_IOCTL_TRIM as c_uint) };
        }
    }

    // Safety: the loop has finished, so the handle is idle.
    unsafe { close(flash) };
    // Safety: `flash` came from the module init and is now closed.
    assert_eq!(unsafe { ffi_module_exit(flash) }, 0, "module exit failed");
    pr_info!("module deallocation");
}

/// C's `*(int *)buffer`, the first `int` of the round's buffer.
fn read_int(buffer: &[u8]) -> c_int {
    let mut bytes = [0u8; INT_SIZE];
    bytes.copy_from_slice(&buffer[..INT_SIZE]);
    c_int::from_ne_bytes(bytes)
}

/// C's `sector = rand() % (1 << 31)`.
///
/// The divisor is `2^31` as an `i64`, the value the C expression reaches
/// through an overflowing `1 << 31`. `rand()` stays strictly below it, so the
/// modulo returns the draw and the sector matches C's.
fn rand_sector() -> off_t {
    // Safety: `rand` takes no arguments and needs no preconditions.
    let draw = unsafe { libc::rand() } as i64;
    (draw % (1i64 << 31)) as off_t
}
