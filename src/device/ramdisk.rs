//! Ramdisk device backend ported from `device/ramdisk/ramdisk.c` +
//! `include/ramdisk.h`.
//!
//! [`Ramdisk`] is an in-memory flash store with flash semantics: a page can be
//! written at most once between erases and an erase clears a whole segment back
//! to zero. It is the default backend and the one the C tests exercise.
//!
//! The C ABI layer keeps the C symbol names with the `ffi_` prefix
//! (`ffi_ramdisk_*`) so it can coexist with `device/ramdisk/ramdisk.c` while
//! both are linked. The cutover is Task 14. [`DeviceOperations`] holds the
//! logic and `ffi_ramdisk_device_init` installs it through the shared backend
//! machinery in [`crate::device`], so there is one vtable and one owner.
//!
//! Deliberate deviations from the C original:
//!
//! - C's `struct ramdisk` (`size`, `buffer`, `is_used`, `o_flags`) is not
//!   mirrored. Nothing outside `ramdisk.c` reads it, `buffer` and `is_used`
//!   become owning `Vec`s, and `size` is `buffer.len()`. `o_flags` is dead in C
//!   (stored, never read) and is dropped. No `_Static_assert` is needed because
//!   there is no shared layout.
//! - The `is_used` bitmap is sized `total_pages.div_ceil(64)`. C's
//!   `BITS_TO_UINT64_ALIGN` over-allocates by one `u64`, which no accessor
//!   reaches.
//! - `open` cannot fail with `-ENOMEM` on the page buffer: `vec!` aborts on
//!   OOM, as `Box` does in the rest of the port. The small `badseg_bitmap`
//!   `calloc` keeps the `-ENOMEM` path, because `fd->badseg_bitmap` is a raw C
//!   pointer that C still reads (`ftl/page/page-map.c`).
//! - Re-opening releases the previous buffers instead of leaking them. C
//!   overwrites `ramdisk->buffer` and `dev->badseg_bitmap` and leaks both.
//! - The `ffi_ramdisk_{open,write,read,erase,close}` wrappers dispatch through
//!   `dev->d_op` instead of reinterpreting `dev->d_private`. C's free functions
//!   assume `d_private` is a `struct ramdisk`, which is wrong for a device C
//!   built itself, and right for a Rust one. Dispatching through the vtable is
//!   correct for both while the two implementations are linked. They also
//!   accept `NULL` and report `-EINVAL`, where C dereferences it.
//! - `read` drops C's `pr_debug` line that dereferences `request->rq_private`
//!   as a `struct device_request`. It only existed to print a mutex address.
//! - A `paddr.lpn` or block past the end of the store panics on the `Vec` and
//!   slice bounds checks, where C reads or writes past the buffer. The panic
//!   aborts if it crosses the `extern "C"` shim, which is a defined outcome
//!   where C corrupts the heap.
//! - C's ramdisk has no locking and neither does this port: concurrent requests
//!   to one device are the caller's responsibility, exactly as in C. The
//!   `unsafe impl Sync` below records that contract for Rust callers.

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::mem::size_of;
use core::ptr;
use core::slice;
use std::cell::UnsafeCell;

use crate::bits::{get_bit, reset_bit, set_bit, BITS_PER_UINT64};
use crate::device::{
    device, device_address, device_request, DeviceDirection, DeviceOperations, PADDR_EMPTY,
    DEVICE_NR_BUS_BITS, DEVICE_NR_CHIPS_BITS, DEVICE_NR_PAGES_BITS, DEVICE_PAGE_SIZE,
};
use crate::{pr_err, pr_info};

/// C's `package->nr_blocks = 64`, the "make 4GiB disk" comment in
/// `ramdisk_open`. With the default geometry it is a 1GiB disk.
const RAMDISK_NR_BLOCKS: usize = 64;

/// In-memory flash store, the Rust form of C's `struct ramdisk`.
///
/// The two buffers are `UnsafeCell` because [`DeviceOperations`] hands out
/// `&self`, and the state is mutated in place. There is no lock, matching C.
pub struct Ramdisk {
    buffer: UnsafeCell<Vec<u8>>,
    is_used: UnsafeCell<Vec<u64>>,
}

// Every method that touches the cells above is `unsafe`, so safe code cannot
// race through a shared `&Ramdisk`. The marker states the backend contract from
// `DeviceOperations`: a backend is callable from any thread, and it is the
// caller's job not to issue overlapping requests, the same deal C's ramdisk
// makes. `UnsafeCell` is what would otherwise make this `!Sync`.
unsafe impl Sync for Ramdisk {}

impl Ramdisk {
    /// The backing store of `total_size` bytes, empty until `open`.
    fn new() -> Self {
        Self {
            buffer: UnsafeCell::new(Vec::new()),
            is_used: UnsafeCell::new(Vec::new()),
        }
    }
}

impl DeviceOperations for Ramdisk {
    /// Allocate the flash geometry, the page store, the used bitmap and the bad
    /// segment bitmap. Mirrors C's `ramdisk_open`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn open(&self, dev: *mut device, _name: *const c_char, _flags: c_int) -> c_int {
        (*dev).info.nr_bus = 1usize << DEVICE_NR_BUS_BITS;
        (*dev).info.nr_chips = 1usize << DEVICE_NR_CHIPS_BITS;
        (*dev).info.package.nr_blocks = RAMDISK_NR_BLOCKS;
        (*dev).info.package.block.nr_pages = 1usize << DEVICE_NR_PAGES_BITS;
        (*dev).info.package.block.page.size = DEVICE_PAGE_SIZE;

        let size = (*dev).total_size();
        let total_pages = size / DEVICE_PAGE_SIZE;
        let buffer = vec![0u8; size];
        let is_used = vec![0u64; total_pages.div_ceil(BITS_PER_UINT64)];

        let nr_segments = (*dev).nr_segments();
        // C leaks the previous bitmap when `open` runs twice. Release it here
        // the way `close` does, so the two paths cannot disagree.
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

        *self.buffer.get() = buffer;
        *self.is_used.get() = is_used;
        (*dev).badseg_bitmap = badseg;

        pr_info!("ramdisk generated (size: {} bytes)", size);
        0
    }

    /// Write one page. Rejects an overwrite of a page written since the last
    /// erase. Mirrors C's `ramdisk_write`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn write(&self, dev: *mut device, request: *mut device_request) -> isize {
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
        if (*request).paddr.lpn == PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }
        let page_size = (*dev).page_size();
        if (*request).data_len != page_size {
            pr_err!(
                "data write size is must be {} (current: {})",
                page_size,
                (*request).data_len
            );
            return -(libc::EINVAL as isize);
        }

        let lpn = (*request).paddr.lpn as usize;
        let is_used = &mut *self.is_used.get();
        if get_bit(is_used, lpn as u64) {
            pr_err!("you overwrite the already written page");
            return -(libc::EINVAL as isize);
        }
        set_bit(is_used, lpn as u64);

        let start = lpn * page_size;
        let data_len = (*request).data_len;
        let src = slice::from_raw_parts((*request).data as *const u8, data_len);
        let buffer = &mut *self.buffer.get();
        buffer[start..start + data_len].copy_from_slice(src);

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        data_len as isize
    }

    /// Read one page, zeroed for a page never written since the last erase.
    /// Mirrors C's `ramdisk_read`, which does not consult the used bitmap.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn read(&self, dev: *mut device, request: *mut device_request) -> isize {
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
        if (*request).paddr.lpn == PADDR_EMPTY {
            pr_err!("physical address is not specified...");
            return -(libc::EINVAL as isize);
        }

        let lpn = (*request).paddr.lpn as usize;
        let data_len = (*request).data_len;
        let start = lpn * page_size;
        let buffer = &*self.buffer.get();
        let dst = slice::from_raw_parts_mut((*request).data as *mut u8, data_len);
        dst.copy_from_slice(&buffer[start..start + data_len]);

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        data_len as isize
    }

    /// Erase one segment: zero its pages and clear their used bits. Mirrors C's
    /// `ramdisk_erase`, including its `uint16_t` truncation of the block number.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn erase(&self, dev: *mut device, request: *mut device_request) -> c_int {
        if (*request).direction() != Some(DeviceDirection::Erase) {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                DeviceDirection::Erase as c_uint,
                (*request).flag
            );
            return -libc::EINVAL;
        }

        // C reads `request->paddr.format.block` through a uint16_t.
        let segnum = (*request).paddr.block() & 0xffff;
        let page_size = (*dev).page_size();
        let nr_pages = (*dev).pages_per_segment();
        let start_lpn = device_address::new(0, 0, segnum, 0).lpn;

        let is_used = &mut *self.is_used.get();
        let buffer = &mut *self.buffer.get();
        for lpn in start_lpn..start_lpn + nr_pages as u32 {
            let start = lpn as usize * page_size;
            buffer[start..start + page_size].fill(0);
            reset_bit(is_used, lpn as u64);
        }

        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        0
    }

    /// Release the flash store and the bad segment bitmap. Mirrors C's
    /// `ramdisk_close`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn close(&self, dev: *mut device) -> c_int {
        if !(*dev).badseg_bitmap.is_null() {
            libc::free((*dev).badseg_bitmap as *mut c_void);
            (*dev).badseg_bitmap = ptr::null_mut();
        }
        *self.buffer.get() = Vec::new();
        *self.is_used.get() = Vec::new();
        0
    }
}

/// Open the ramdisk, mirroring C's `ramdisk_open`.
///
/// Dispatches through `dev->d_op`, so a device from [`ffi_ramdisk_device_init`]
/// and one C built with its own `ramdisk_device_init` both reach the right
/// `open`.
///
/// # Safety
/// `dev` is `NULL` or a live device.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_open(
    dev: *mut device,
    name: *const c_char,
    flags: c_int,
) -> c_int {
    if dev.is_null() {
        return -libc::EINVAL;
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.open) {
        Some(open) => open(dev, name, flags),
        None => -libc::ENODEV,
    }
}

/// Write to the ramdisk, mirroring C's `ramdisk_write`.
///
/// # Safety
/// `dev` is `NULL` or a live device, `request` is `NULL` or a live request, and
/// `dev` has a backend with `write`.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_write(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    if dev.is_null() || request.is_null() {
        return -(libc::EINVAL as isize);
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.write) {
        Some(write) => write(dev, request),
        None => -(libc::ENODEV as isize),
    }
}

/// Read from the ramdisk, mirroring C's `ramdisk_read`.
///
/// # Safety
/// `dev` is `NULL` or a live device, `request` is `NULL` or a live request, and
/// `dev` has a backend with `read`.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_read(
    dev: *mut device,
    request: *mut device_request,
) -> isize {
    if dev.is_null() || request.is_null() {
        return -(libc::EINVAL as isize);
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.read) {
        Some(read) => read(dev, request),
        None => -(libc::ENODEV as isize),
    }
}

/// Erase a segment, mirroring C's `ramdisk_erase`.
///
/// # Safety
/// `dev` is `NULL` or a live device, `request` is `NULL` or a live request, and
/// `dev` has a backend with `erase`.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_erase(
    dev: *mut device,
    request: *mut device_request,
) -> c_int {
    if dev.is_null() || request.is_null() {
        return -libc::EINVAL;
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.erase) {
        Some(erase) => erase(dev, request),
        None => -libc::ENODEV,
    }
}

/// Close the ramdisk, mirroring C's `ramdisk_close`.
///
/// # Safety
/// `dev` is `NULL` or a live device.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_close(dev: *mut device) -> c_int {
    if dev.is_null() {
        return -libc::EINVAL;
    }
    match (*dev).d_op.as_ref().and_then(|ops| ops.close) {
        Some(close) => close(dev),
        None => -libc::ENODEV,
    }
}

/// Initialize the ramdisk as the device backend, mirroring C's
/// `ramdisk_device_init`. The C init only allocates the backend and installs
/// the vtable, it does not open the disk, and neither does this.
///
/// # Safety
/// `dev` is `NULL` or a live device with no backend installed yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_device_init(dev: *mut device, flags: u64) -> c_int {
    let _ = flags;
    if dev.is_null() {
        return -libc::EINVAL;
    }
    (*dev).install_backend(Ramdisk::new());
    (*dev).d_submodule_exit = Some(ffi_ramdisk_device_exit);
    0
}

/// Deallocate the ramdisk backend, mirroring C's `ramdisk_device_exit`.
///
/// # Safety
/// `dev` is `NULL` or a device whose backend is a [`Ramdisk`] from
/// [`ffi_ramdisk_device_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_ramdisk_device_exit(dev: *mut device) -> c_int {
    if dev.is_null() || (*dev).d_private.is_null() {
        return 0;
    }
    let _ = ffi_ramdisk_close(dev);
    (*dev).uninstall_backend::<Ramdisk>();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{ffi_device_module_init, ffi_device_module_exit, DeviceModule, Request};
    use core::sync::atomic::{AtomicUsize, Ordering};

    fn init_device() -> *mut device {
        let mut dev: *mut device = ptr::null_mut();
        let ret = unsafe { ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0) };
        assert_eq!(ret, 0, "ramdisk module init");
        assert!(!dev.is_null());
        dev
    }

    fn zeroed_device() -> device {
        unsafe { core::mem::MaybeUninit::<device>::zeroed().assume_init() }
    }

    unsafe fn open(dev: *mut device) {
        assert_eq!(
            ffi_ramdisk_open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR),
            0
        );
    }

    /// Write `lpn` with the page number stamped into its first four bytes.
    unsafe fn write_page(
        dev: *mut device,
        request: &mut Request,
        page: &mut [u8],
        lpn: u32,
    ) -> isize {
        page[..4].copy_from_slice(&lpn.to_ne_bytes());
        request.paddr = device_address::from(lpn);
        request.data_len = page.len();
        request.flag = DeviceDirection::Write.into();
        request.data = page.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        ffi_ramdisk_write(dev, request.as_ptr())
    }

    unsafe fn read_page(dev: *mut device, request: &mut Request, page: &mut [u8], lpn: u32) -> isize {
        request.paddr = device_address::from(lpn);
        request.data_len = page.len();
        request.flag = DeviceDirection::Read.into();
        request.data = page.as_mut_ptr() as *mut c_void;
        request.end_rq = None;
        ffi_ramdisk_read(dev, request.as_ptr())
    }

    #[test]
    fn init_open_close_round_trip() {
        unsafe {
            let dev = init_device();
            assert!(!(*dev).d_op.is_null());
            open(dev);

            // The default geometry from the Makefile's DEVICE_INFO.
            assert_eq!((*dev).page_size(), DEVICE_PAGE_SIZE);
            assert_eq!((*dev).nr_segments(), RAMDISK_NR_BLOCKS);
            assert_eq!((*dev).pages_per_segment(), 16 * 128);
            assert_eq!((*dev).total_size(), 1 << 30);
            assert!(!(*dev).badseg_bitmap.is_null());

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert!((*dev).badseg_bitmap.is_null());
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn full_write_then_read_back() {
        unsafe {
            let dev = init_device();
            open(dev);
            let page_size = (*dev).page_size();
            let total_pages = (*dev).total_pages();
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; page_size];

            for lpn in 0..total_pages as u32 {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    page_size as isize
                );
            }
            for lpn in 0..total_pages as u32 {
                assert_eq!(
                    read_page(dev, &mut request, &mut page, lpn),
                    page_size as isize
                );
                assert_eq!(u32::from_ne_bytes(page[..4].try_into().unwrap()), lpn);
            }

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn overwrite_is_rejected_until_erased() {
        unsafe {
            let dev = init_device();
            open(dev);
            let page_size = (*dev).page_size();
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; page_size];

            for lpn in [0u32, 1, 5, 7] {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    page_size as isize
                );
            }
            for lpn in [0u32, 1, 5, 7] {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    -(libc::EINVAL as isize)
                );
            }
            // A page never written is still writable.
            assert_eq!(
                write_page(dev, &mut request, &mut page, 2),
                page_size as isize
            );

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn erase_clears_a_segment_then_rewrite_succeeds() {
        unsafe {
            let dev = init_device();
            open(dev);
            let page_size = (*dev).page_size();
            let pages_per_segment = (*dev).pages_per_segment();
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; page_size];

            // Fill segment 0 and segment 1.
            for lpn in 0..(2 * pages_per_segment) as u32 {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    page_size as isize
                );
            }

            request.paddr = device_address::new(0, 0, 0, 0);
            request.flag = DeviceDirection::Erase.into();
            assert_eq!(ffi_ramdisk_erase(dev, request.as_ptr()), 0);

            // Erased pages read back as zero and are writable again.
            page.fill(0xff);
            assert_eq!(
                read_page(dev, &mut request, &mut page, 0),
                page_size as isize
            );
            assert!(page.iter().all(|byte| *byte == 0));
            for lpn in 0..pages_per_segment as u32 {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    page_size as isize
                );
            }

            // Segment 1 is untouched, so its pages still reject a rewrite.
            for lpn in pages_per_segment as u32..(2 * pages_per_segment) as u32 {
                assert_eq!(
                    write_page(dev, &mut request, &mut page, lpn),
                    -(libc::EINVAL as isize)
                );
            }

            // A non-erase request through the erase entry point is rejected.
            request.paddr = device_address::from(0u32);
            request.flag = DeviceDirection::Write.into();
            assert_eq!(ffi_ramdisk_erase(dev, request.as_ptr()), -libc::EINVAL);

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn write_rejects_bad_requests() {
        unsafe {
            let dev = init_device();
            open(dev);
            let page_size = (*dev).page_size();
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; page_size];

            // NULL data pointer.
            request.data = ptr::null_mut();
            request.data_len = page_size;
            request.flag = DeviceDirection::Write.into();
            request.paddr = device_address::from(0u32);
            assert_eq!(
                ffi_ramdisk_write(dev, request.as_ptr()),
                -(libc::ENODATA as isize)
            );

            // Wrong direction.
            request.data = page.as_mut_ptr() as *mut c_void;
            request.flag = DeviceDirection::Read.into();
            assert_eq!(
                ffi_ramdisk_write(dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // Unassigned physical address.
            request.flag = DeviceDirection::Write.into();
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(
                ffi_ramdisk_write(dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // Short write.
            request.paddr = device_address::from(0u32);
            request.data_len = page_size - 1;
            assert_eq!(
                ffi_ramdisk_write(dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    static END_RQ_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_end_rq(_request: *mut device_request) {
        END_RQ_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn end_rq_runs_on_every_successful_operation() {
        unsafe {
            let dev = init_device();
            open(dev);
            let page_size = (*dev).page_size();
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; page_size];
            END_RQ_CALLS.store(0, Ordering::SeqCst);

            for lpn in 0u32..3 {
                page[..4].copy_from_slice(&lpn.to_ne_bytes());
                request.paddr = device_address::from(lpn);
                request.data_len = page_size;
                request.flag = DeviceDirection::Write.into();
                request.data = page.as_mut_ptr() as *mut c_void;
                request.end_rq = Some(count_end_rq);
                assert_eq!(
                    ffi_ramdisk_write(dev, request.as_ptr()),
                    page_size as isize
                );
            }

            // A rejected write does not call the callback.
            request.paddr = device_address::from(0u32);
            assert_eq!(
                ffi_ramdisk_write(dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );
            assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 3);

            for lpn in 0u32..3 {
                request.paddr = device_address::from(lpn);
                request.flag = DeviceDirection::Read.into();
                assert_eq!(ffi_ramdisk_read(dev, request.as_ptr()), page_size as isize);
            }

            request.paddr = device_address::new(0, 0, 0, 0);
            request.flag = DeviceDirection::Erase.into();
            assert_eq!(ffi_ramdisk_erase(dev, request.as_ptr()), 0);

            assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 7);

            assert_eq!(ffi_ramdisk_close(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    #[should_panic]
    fn out_of_range_lpn_panics_instead_of_corrupting() {
        // C's `memcpy(&ramdisk->buffer[addr.lpn * page_size], ...)` runs off
        // the buffer. The Rust port bounds-checks. Call the trait method
        // directly so the panic unwinds into the harness instead of aborting
        // across the `extern "C"` shim.
        unsafe {
            let mut dev = zeroed_device();
            let ramdisk = Ramdisk::new();
            assert_eq!(ramdisk.open(&mut dev, ptr::null(), 0), 0);

            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; dev.page_size()];
            request.paddr = device_address::from(dev.total_pages() as u32);
            request.data_len = page.len();
            request.flag = DeviceDirection::Write.into();
            request.data = page.as_mut_ptr() as *mut c_void;
            request.end_rq = None;
            ramdisk.write(&mut dev, request.as_ptr());
        }
    }
}
