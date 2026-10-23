//! Page FTL interface bridge, ported from `ftl/page/page-interface.c`.
//!
//! C's `__page_fops` installs one operation per flash hook, and the page FTL
//! reaches its own state through `flash->f_private`. [`PAGE_FOPS`] is that
//! table, and the five functions above it are C's `page_ftl_*_interface` entry
//! points. Every one of them casts `f_private` back to [`PageFtl`]
//! ([`page_ftl_of`]) and then drives the native paths in
//! [`crate::ftl::page_core`] and [`crate::ftl::page_gc`].
//!
//! [`ffi_page_ftl_module_init`] and [`ffi_page_ftl_module_exit`] are C's
//! `page_ftl_module_init` and `page_ftl_module_exit`. The module dispatch table
//! in [`crate::module`] runs that pair: init installs [`PAGE_FOPS`], allocates
//! the page FTL and brings up the device module the flag word names. Exit
//! closes the FTL, releases it and tears the device module down.
//!
//! The read and write entry points are the reason this layer exists. They split
//! the user buffer into one page sized request per page the buffer touches. The
//! first and last request of a transfer that does not start or end on a page
//! boundary are therefore partial, and each request is submitted and completed
//! on its own.
//!
//! [`ffi_flash_write`] and [`ffi_flash_read`] drive the table the same way, for
//! Rust callers. C has no such pair. Its callers reach through `f_op`
//! themselves (`integration-test.c:95`, `benchmark.c:646`).
//!
//! Deliberate deviations from C are listed in the [`crate::ftl`] module docs.

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;
use libc::{off_t, size_t, ssize_t};

use crate::device::{
    device, ffi_device_module_exit, ffi_device_module_init, DeviceDirection, Request,
};
use crate::flash::{flash_device, flash_operations};
use crate::ftl::page_core::{self, submit_request};
use crate::ftl::page_gc::gc_from_list;
use crate::ftl::{PageFtl, PageFtlLock, PAGE_FTL_GC_ALL, PAGE_FTL_IOCTL_TRIM};
use crate::{pr_debug, pr_err, pr_info};

/// The page FTL a flash handle carries, C's
/// `(struct page_ftl *)flash->f_private`.
///
/// Reports `-EINVAL` for a missing handle or a handle without FTL state, where
/// C returns the same code after logging.
///
/// # Safety
/// `flash` is `NULL` or a live handle whose `f_private` is a page FTL owned by
/// [`ffi_page_ftl_module_init`].
unsafe fn page_ftl_of(flash: *mut flash_device) -> Result<*mut PageFtl, c_int> {
    if flash.is_null() {
        pr_err!("flash pointer doesn't exist");
        return Err(-libc::EINVAL);
    }

    let pgftl = (*flash).f_private as *mut PageFtl;
    if pgftl.is_null() {
        pr_err!("page FTL information doesn't exist");
        return Err(-libc::EINVAL);
    }

    Ok(pgftl)
}

/// C's `page_ftl_open_interface`: hand the open to the page FTL.
///
/// # Safety
/// `flash` is a live handle carrying a page FTL, and `name` is `NULL` or a NUL
/// terminated string.
unsafe extern "C" fn page_ftl_open_interface(
    flash: *mut flash_device,
    name: *const c_char,
    flags: c_int,
) -> c_int {
    match page_ftl_of(flash) {
        Ok(pgftl) => page_core::open(&mut *pgftl, name, flags),
        Err(err) => err,
    }
}

/// C's `page_ftl_write_interface`: write `count` bytes from `buffer` at
/// `sector`.
///
/// The buffer is split into one request per page, so a transfer that does not
/// start or end on a page boundary submits a partial first and last request.
/// Each request allocates its own [`Request`] and completes before the next one
/// is built.
///
/// The return value is C's, quirk included. A request that fails after an
/// earlier one succeeded returns the byte count written so far, not an error
/// (`ftl/page/page-interface.c:119-125`, `:140-144`). See the deviations in
/// [`crate::ftl`].
///
/// # Safety
/// `flash` is a live handle carrying an opened page FTL, and `buffer` points at
/// `count` readable bytes.
unsafe extern "C" fn page_ftl_write_interface(
    flash: *mut flash_device,
    buffer: *mut c_void,
    count: size_t,
    offset: off_t,
) -> ssize_t {
    let pgftl = match page_ftl_of(flash) {
        Ok(pgftl) => pgftl,
        Err(err) => return err as ssize_t,
    };
    // Hold C's `pgftl->mutex` for the whole transfer, the way the native write
    // path expects the exclusive borrow to be held.
    let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
    let pgftl = lock.get();

    if !matches!(pgftl.o_flags & libc::O_ACCMODE, libc::O_WRONLY | libc::O_RDWR) {
        pr_err!(
            "cannot find the valid write flags (flags: 0x{:x})",
            pgftl.o_flags
        );
        return -(libc::EINVAL as ssize_t);
    }

    let page_size = pgftl.dev().page_size();
    let buffer_start = buffer as usize;
    let mut size = 0usize;
    let mut remain = count;
    let mut ptr = buffer as *mut u8;
    let mut sector = offset as usize;

    while remain > 0 {
        let pos = pgftl.page_offset(sector);
        let submit_size = if pos + remain < page_size {
            remain
        } else {
            page_size - pos
        };

        let mut request = match Request::new() {
            Ok(request) => request,
            Err(_) => {
                pr_err!("fail to allocate request structure");
                return -(libc::ENOMEM as ssize_t);
            }
        };
        request.flag = DeviceDirection::Write.into();
        request.data_len = submit_size;
        request.sector = sector;
        request.data = ptr as *mut c_void;

        pr_debug!(
            "{} (length: {}, buffer: {}, remain:{})",
            request.sector,
            request.data_len,
            ptr as usize - buffer_start,
            remain
        );

        let write_size = match submit_request(pgftl, &mut request) {
            Ok(write_size) => write_size,
            Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO) as ssize_t),
        };
        if write_size as usize != submit_size {
            pr_err!(
                "page FTL submit request failed (write size: {})",
                write_size
            );
            return size as ssize_t;
        }

        sector += write_size as usize;
        remain -= write_size as usize;
        ptr = ptr.add(write_size as usize);
        size += write_size as usize;
    }

    size as ssize_t
}

/// C's `page_ftl_read_interface`: read `count` bytes into `buffer` from
/// `sector`.
///
/// The buffer is split the same way the write path splits it, one request per
/// page with a partial first and last request.
///
/// C stages every chunk through a page sized `malloc` and copies it out of
/// there (`ftl/page/page-interface.c:200-205`, `:241`). The port hands the
/// caller's buffer to the read path directly, because
/// [`crate::ftl::page_read::read_page`] already stages the device read in its
/// own scratch page and copies the requested subrange into the request's
/// buffer.
///
/// The read interface reports `-ENOMEM` when it cannot allocate the per chunk
/// request (`ftl/page/page-interface.c:222-227`). Every other failure reports
/// `-EINVAL`, whatever the read path returned (`:236-239`), which is what C's
/// interface does.
///
/// # Safety
/// `flash` is a live handle carrying an opened page FTL, and `buffer` points at
/// `count` writable bytes.
unsafe extern "C" fn page_ftl_read_interface(
    flash: *mut flash_device,
    buffer: *mut c_void,
    count: size_t,
    offset: off_t,
) -> ssize_t {
    let mut ptr = buffer as *mut u8;
    if ptr.is_null() {
        pr_err!("buffer pointer doesn't exist");
        return -(libc::EINVAL as ssize_t);
    }

    let pgftl = match page_ftl_of(flash) {
        Ok(pgftl) => pgftl,
        Err(err) => return err as ssize_t,
    };
    // The read path borrows the FTL exclusively too, so it holds the same lock.
    let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
    let pgftl = lock.get();

    if !matches!(pgftl.o_flags & libc::O_ACCMODE, libc::O_RDONLY | libc::O_RDWR) {
        pr_err!(
            "cannot find the valid read flags (flags: 0x{:x})",
            pgftl.o_flags
        );
        return -(libc::EINVAL as ssize_t);
    }

    let page_size = pgftl.dev().page_size();
    let mut size = 0usize;
    let mut remain = count;
    let mut sector = offset as usize;

    while remain != 0 {
        let pos = pgftl.page_offset(sector);
        let submit_size = if pos + remain < page_size {
            remain
        } else {
            page_size - pos
        };

        let mut request = match Request::new() {
            Ok(request) => request,
            Err(_) => {
                pr_err!("fail to allocate request structure");
                return -(libc::ENOMEM as ssize_t);
            }
        };
        request.flag = DeviceDirection::Read.into();
        request.data_len = submit_size;
        request.sector = sector;
        request.data = ptr as *mut c_void;

        let read_size = match submit_request(pgftl, &mut request) {
            Ok(read_size) => read_size,
            Err(_) => {
                pr_err!("page FTL submit request failed");
                return -(libc::EINVAL as ssize_t);
            }
        };

        sector += read_size as usize;
        remain -= read_size as usize;
        ptr = ptr.add(read_size as usize);
        size += read_size as usize;
    }

    size as ssize_t
}

/// C's `page_ftl_close_interface`: close the page FTL.
///
/// # Safety
/// `flash` is a live handle carrying a page FTL.
unsafe extern "C" fn page_ftl_close_interface(flash: *mut flash_device) -> c_int {
    match page_ftl_of(flash) {
        Ok(pgftl) => page_core::close(&mut *pgftl),
        Err(err) => err,
    }
}

/// The page FTL's extra commands. C calls this `page_ftl_ioctl_interface`.
///
/// `PAGE_FTL_IOCTL_TRIM` collects every segment queued for garbage collection.
/// Anything else reports `-EINVAL`.
///
/// C declares this operation variadic (`include/flash.h:59-60`) and never reads
/// a variadic argument. The two in-tree callers pass only the request
/// (`integration-test.rs:322`, `example/rw_example.rs:97`), so the Rust slot is
/// narrowed to that shape and the command reaches the operation type safely.
/// See the deviations in [`crate::ftl`].
///
/// # Safety
/// `flash` is a live handle carrying an opened page FTL.
unsafe extern "C" fn page_ftl_ioctl_interface(flash: *mut flash_device, request: c_uint) -> c_int {
    let pgftl = match page_ftl_of(flash) {
        Ok(pgftl) => pgftl,
        Err(err) => return err,
    };
    // GC walks the segment array and rewrites pages, so it holds the FTL lock
    // C's `page_ftl_gc_from_list` takes itself (`ftl/page/page-gc.c:259`).
    let mut lock = unsafe { PageFtlLock::acquire(pgftl) };
    let pgftl = lock.get();

    let mut device_rq = match Request::new() {
        Ok(request) => request,
        Err(_) => {
            pr_err!("request allocation failed");
            return -libc::ENOMEM;
        }
    };

    if request != PAGE_FTL_IOCTL_TRIM as c_uint {
        pr_err!("invalid command requested(commands: {})", request);
        return -libc::EINVAL;
    }

    device_rq.flag = DeviceDirection::Erase.into();
    match gc_from_list(pgftl, &mut device_rq, PAGE_FTL_GC_ALL) {
        Ok(ret) => ret as c_int,
        Err(err) => -(err.raw_os_error().unwrap_or(libc::EIO)),
    }
}

/// The operations table the page FTL installs into `flash->f_op`. C calls it
/// `__page_fops`.
pub static PAGE_FOPS: flash_operations = flash_operations {
    open: Some(page_ftl_open_interface),
    write: Some(page_ftl_write_interface),
    read: Some(page_ftl_read_interface),
    ioctl: Some(page_ftl_ioctl_interface),
    close: Some(page_ftl_close_interface),
};

/// C's `page_ftl_module_init`: bring up the page FTL over the device module
/// `flags` names.
///
/// The table is installed before anything can fail, as in C. A `NULL` flash
/// handle reports `-EINVAL`, where C dereferences it. A failing device module
/// leaves no FTL state on the handle and reports the device module's error.
///
/// # Safety
/// `flash` is `NULL` or a live handle from
/// [`crate::flash::ffi_flash_module_init`], and `flags` is a device module
/// number.
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_module_init(flash: *mut flash_device, flags: u64) -> c_int {
    if flash.is_null() {
        pr_err!("flash pointer doesn't exist");
        return -libc::EINVAL;
    }

    (*flash).f_op = &PAGE_FOPS;

    // C's `malloc(sizeof(struct page_ftl))` plus the `memset(0)`.
    let mut pgftl = Box::new(PageFtl::empty());

    let mut dev: *mut device = ptr::null_mut();
    let err = ffi_device_module_init(flags, &mut dev, 0);
    if err != 0 {
        pr_err!("initialize the device module failed");
        // C runs `page_ftl_module_exit`, which is a logged no-op while
        // `f_private` is still NULL, and leaks the page FTL. The Box releases
        // it here.
        return err;
    }

    pgftl.dev = dev;
    (*flash).f_private = Box::into_raw(pgftl) as *mut c_void;
    (*flash).f_submodule_exit = Some(ffi_page_ftl_module_exit);

    0
}

/// C's `page_ftl_module_exit`: close the page FTL, release it and tear the
/// device module down.
///
/// A `NULL` flash handle or a flash handle without a page FTL is a logged
/// no-op that returns `0`, as in C.
///
/// # Safety
/// `flash` is `NULL` or a live handle from [`ffi_page_ftl_module_init`].
#[no_mangle]
pub unsafe extern "C" fn ffi_page_ftl_module_exit(flash: *mut flash_device) -> c_int {
    if flash.is_null() {
        pr_info!("flash pointer is null detected");
        return 0;
    }

    let pgftl = (*flash).f_private as *mut PageFtl;
    if pgftl.is_null() {
        pr_info!("page ftl doesn't exist");
        return 0;
    }

    let dev = (*pgftl).dev;
    page_core::close(&mut *pgftl);
    drop(Box::from_raw(pgftl));
    // C leaves the freed pointer in `f_private`, so a second `module_exit`
    // frees it again. Clearing it makes the repeat a logged no-op.
    (*flash).f_private = ptr::null_mut();

    if dev.is_null() {
        pr_info!("device module doesn't exist");
        return 0;
    }

    let ret = ffi_device_module_exit(dev);
    if ret != 0 {
        pr_err!("device module exit failed");
        return ret;
    }

    0
}

/// Drive the flash handle's write operation, the call C spells
/// `flash->f_op->write(flash, buffer, count, offset)`.
///
/// A `NULL` handle, a handle without an operations table and a table without a
/// write operation all report `-EINVAL`. C dereferences the first two and calls
/// the third.
///
/// # Safety
/// `flash` is a live handle and `buffer` points at `count` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn ffi_flash_write(
    flash: *mut flash_device,
    buffer: *mut c_void,
    count: size_t,
    offset: off_t,
) -> ssize_t {
    if flash.is_null() || (*flash).f_op.is_null() {
        pr_err!("flash pointer or operations don't exist");
        return -(libc::EINVAL as ssize_t);
    }

    match (*(*flash).f_op).write {
        Some(write) => write(flash, buffer, count, offset),
        None => {
            pr_err!("flash operations have no write");
            -(libc::EINVAL as ssize_t)
        }
    }
}

/// Drive the flash handle's read operation, the call C spells
/// `flash->f_op->read(flash, buffer, count, offset)`.
///
/// A `NULL` handle, a handle without an operations table and a table without a
/// read operation all report `-EINVAL`. C dereferences the first two and calls
/// the third.
///
/// # Safety
/// `flash` is a live handle and `buffer` points at `count` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn ffi_flash_read(
    flash: *mut flash_device,
    buffer: *mut c_void,
    count: size_t,
    offset: off_t,
) -> ssize_t {
    if flash.is_null() || (*flash).f_op.is_null() {
        pr_err!("flash pointer or operations don't exist");
        return -(libc::EINVAL as ssize_t);
    }

    match (*(*flash).f_op).read {
        Some(read) => read(flash, buffer, count, offset),
        None => {
            pr_err!("flash operations have no read");
            -(libc::EINVAL as ssize_t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::set_bit;
    use crate::device::{device_address, DeviceModule, PADDR_EMPTY, DEVICE_PAGE_SIZE};
    use crate::ftl::test_support::Fixture;
    use crate::module::{ffi_module_exit, ffi_module_init, PAGE_FTL_MODULE};
    use core::sync::atomic::Ordering;

    /// The flag pair C's callers pass (`integration-test.rs:341`).
    const FLAGS: c_int = libc::O_CREAT | libc::O_RDWR;

    /// Four segments of four pages: small enough that a multi page transfer
    /// crosses several segment boundaries, and that every page of the region a
    /// test writes fits.
    const NR_SEGMENTS: usize = 4;
    const PAGES_PER_SEGMENT: usize = 4;

    type FlashOp = unsafe extern "C" fn(*mut flash_device, *mut c_void, size_t, off_t) -> ssize_t;

    /// One operation of the installed table, which a test then drives the way
    /// the C callers drive `flash->f_op`.
    fn op(slot: Option<FlashOp>) -> FlashOp {
        slot.expect("the page FTL installs every operation")
    }

    /// A flash handle over the fixture's page FTL, the state C's
    /// `page_ftl_module_init` would have built.
    fn flash_of(f: &mut Fixture) -> flash_device {
        f.pgftl.o_flags = FLAGS;
        flash_device {
            f_op: &PAGE_FOPS,
            f_private: f.pgftl.as_mut() as *mut PageFtl as *mut c_void,
            f_submodule_exit: None,
        }
    }

    /// The dispatch to module exit cycle the issue names: ramdisk module init,
    /// page FTL module init, open, a short write, the read back, close, module
    /// exit.
    #[test]
    fn module_write_read_cycle() {
        let mut flash: *mut flash_device = ptr::null_mut();
        assert_eq!(
            unsafe { ffi_module_init(PAGE_FTL_MODULE, &mut flash, DeviceModule::Ramdisk as u64) },
            0
        );
        assert!(!flash.is_null());

        unsafe {
            assert_eq!((*flash).f_op, &PAGE_FOPS as *const flash_operations);
            assert!(!(*flash).f_private.is_null());
            assert!((*flash).f_submodule_exit.is_some());

            // A NULL name keeps the checkpoint off the filesystem, which the
            // ramdisk ignores anyway.
            let open = PAGE_FOPS.open.expect("the page FTL installs open");
            assert_eq!(open(flash, ptr::null(), FLAGS), 0);

            let mut payload = *b"hello";
            assert_eq!(
                ffi_flash_write(flash, payload.as_mut_ptr() as *mut c_void, payload.len(), 0),
                payload.len() as ssize_t
            );

            let mut readback = [0u8; 5];
            assert_eq!(
                ffi_flash_read(
                    flash,
                    readback.as_mut_ptr() as *mut c_void,
                    readback.len(),
                    0
                ),
                readback.len() as ssize_t
            );
            assert_eq!(&readback, &payload);

            let close = PAGE_FOPS.close.expect("the page FTL installs close");
            assert_eq!(close(flash), 0);

            assert_eq!(ffi_module_exit(flash), 0);
        }
    }

    /// The criterion inherited from Tasks 9 and 10: a buffer that starts mid
    /// page, spans several segments and ends mid page, written and read through
    /// the interface, with the assembled bytes asserted.
    #[test]
    fn interface_splits_a_multi_segment_transfer_at_page_boundaries() {
        let mut f = Fixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let page_size = DEVICE_PAGE_SIZE;
        let segment_size = PAGES_PER_SEGMENT * page_size;

        // Starts 92 bytes before the end of page 0, so the first request is a
        // partial page. Ends 580 bytes into page 12, so the last one is partial
        // too, and pages 0 to 12 span four segments of four pages.
        let start = page_size - 92;
        let len = 92 + 11 * page_size + 580;
        let end = start + len;
        assert_eq!(end, 12 * page_size + 580);
        assert!(end > 3 * segment_size, "the buffer reaches every segment");

        let mut flash = flash_of(&mut f);
        let mut payload: Vec<u8> = (0..len).map(|i| ((start + i) % 253) as u8).collect();

        assert_eq!(
            unsafe {
                op(PAGE_FOPS.write)(
                    &mut flash,
                    payload.as_mut_ptr() as *mut c_void,
                    payload.len(),
                    start as off_t,
                )
            },
            len as ssize_t
        );

        // One request per page: 13 logical pages, each mapped to a page of its
        // own, spread over several segments.
        let first_lpn = start / page_size;
        let last_lpn = (end - 1) / page_size;
        assert_eq!(last_lpn - first_lpn + 1, 13);

        let mut blocks = Vec::new();
        for lpn in 0..=last_lpn {
            let paddr = device_address::from(f.pgftl.trans_map[lpn]);
            if lpn < first_lpn {
                assert_eq!(paddr.lpn, PADDR_EMPTY, "lpn {} stays unmapped", lpn);
                continue;
            }
            assert_ne!(paddr.lpn, PADDR_EMPTY, "lpn {} is mapped", lpn);
            let block = paddr.block() as usize;
            assert!(
                block < NR_SEGMENTS,
                "lpn {} maps to block {}, past the device",
                lpn,
                block
            );
            if !blocks.contains(&block) {
                blocks.push(block);
            }
        }
        assert!(
            blocks.len() >= 3,
            "the transfer crossed segments (blocks: {:?})",
            blocks
        );

        // Read the whole span back, page aligned this time, so the read split
        // differs from the write split and the bytes can only be right if both
        // loops placed every one of them at its absolute sector.
        let span = 13 * page_size;
        let mut assembled = vec![0xaau8; span];
        assert_eq!(
            unsafe {
                op(PAGE_FOPS.read)(
                    &mut flash,
                    assembled.as_mut_ptr() as *mut c_void,
                    assembled.len(),
                    0,
                )
            },
            span as ssize_t
        );
        assert!(
            assembled[..start].iter().all(|byte| *byte == 0),
            "the bytes before the payload stay zero"
        );
        assert_eq!(assembled[start..end], payload[..], "the payload");
        assert!(
            assembled[end..].iter().all(|byte| *byte == 0),
            "the bytes after the payload stay zero"
        );

        // And read the payload back through its own split, partial pages on
        // both ends.
        let mut single = vec![0u8; len];
        assert_eq!(
            unsafe {
                op(PAGE_FOPS.read)(
                    &mut flash,
                    single.as_mut_ptr() as *mut c_void,
                    single.len(),
                    start as off_t,
                )
            },
            len as ssize_t
        );
        assert_eq!(single, payload);
    }

    /// A region that was never written comes back zeroed, page aligned or not.
    #[test]
    fn interface_serves_an_unmapped_region_with_zeros() {
        let mut f = Fixture::new(1, PAGES_PER_SEGMENT);
        let mut flash = flash_of(&mut f);

        let mut buffer = vec![0x5au8; DEVICE_PAGE_SIZE + 3];
        assert_eq!(
            unsafe {
                op(PAGE_FOPS.read)(
                    &mut flash,
                    buffer.as_mut_ptr() as *mut c_void,
                    buffer.len(),
                    5,
                )
            },
            buffer.len() as ssize_t
        );
        assert!(buffer.iter().all(|byte| *byte == 0));
    }

    /// Both loops check the open flags, as C's interfaces do.
    #[test]
    fn interface_rejects_the_wrong_open_flags() {
        let mut f = Fixture::new(1, PAGES_PER_SEGMENT);
        f.pgftl.o_flags = libc::O_CREAT | libc::O_RDONLY;
        let mut flash = flash_device {
            f_op: &PAGE_FOPS,
            f_private: f.pgftl.as_mut() as *mut PageFtl as *mut c_void,
            f_submodule_exit: None,
        };
        let mut byte = [0u8; 1];
        let buffer = byte.as_mut_ptr() as *mut c_void;

        assert_eq!(
            unsafe { op(PAGE_FOPS.write)(&mut flash, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(unsafe { op(PAGE_FOPS.read)(&mut flash, buffer, 1, 0) }, 1);

        // A write-only handle reads nothing, and an unopened one makes no
        // write loop.
        f.pgftl.o_flags = libc::O_WRONLY;
        assert_eq!(
            unsafe { op(PAGE_FOPS.read)(&mut flash, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        f.pgftl.o_flags = 0;
        assert_eq!(
            unsafe { op(PAGE_FOPS.write)(&mut flash, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
    }

    /// The interface entry points refuse the pointers C dereferences, and the
    /// two wrappers refuse a handle that carries no operation.
    #[test]
    fn interface_rejects_null_pointers() {
        let mut f = Fixture::new(1, PAGES_PER_SEGMENT);
        let mut flash = flash_of(&mut f);
        let mut byte = [0u8; 1];
        let buffer = byte.as_mut_ptr() as *mut c_void;

        assert_eq!(
            unsafe { op(PAGE_FOPS.write)(ptr::null_mut(), buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { op(PAGE_FOPS.read)(ptr::null_mut(), buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        // C checks the read buffer before it looks at the handle.
        assert_eq!(
            unsafe { op(PAGE_FOPS.read)(&mut flash, ptr::null_mut(), 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { PAGE_FOPS.open.expect("open")(ptr::null_mut(), ptr::null(), FLAGS) },
            -libc::EINVAL
        );

        // A handle whose `f_private` was never filled, the state a failed
        // submodule init leaves behind.
        let mut bare = flash_device {
            f_op: &PAGE_FOPS,
            f_private: ptr::null_mut(),
            f_submodule_exit: None,
        };
        assert_eq!(
            unsafe { op(PAGE_FOPS.write)(&mut bare, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { PAGE_FOPS.close.expect("close")(&mut bare) },
            -libc::EINVAL
        );

        let empty_ops = flash_operations {
            open: None,
            write: None,
            read: None,
            ioctl: None,
            close: None,
        };
        let mut empty = flash_device {
            f_op: &empty_ops,
            f_private: ptr::null_mut(),
            f_submodule_exit: None,
        };
        assert_eq!(
            unsafe { ffi_flash_write(ptr::null_mut(), buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { ffi_flash_read(&mut empty, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { ffi_flash_write(&mut empty, buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
        assert_eq!(
            unsafe { ffi_flash_read(ptr::null_mut(), buffer, 1, 0) },
            -(libc::EINVAL as ssize_t)
        );
    }

    /// The ioctl drives the collection for `PAGE_FTL_IOCTL_TRIM` and refuses
    /// anything else.
    #[test]
    fn ioctl_trims_and_rejects_an_unknown_command() {
        let mut f = Fixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let mut flash = flash_of(&mut f);
        let ioctl = PAGE_FOPS.ioctl.expect("the page FTL installs ioctl");

        // An exhausted segment on the GC list, the state `invalidate` queues a
        // segment in. It has no valid page to relocate, so the collection
        // erases it and resets its counters.
        f.pgftl.segments[1].nr_free_pages.store(0, Ordering::SeqCst);
        f.pgftl.gc_list.insert(0, 1);
        set_bit(&mut f.pgftl.gc_seg_bits, 1);
        assert_eq!(f.gc_list_len(), 1);

        // The one segment it erased is the one the count does not report
        // (`ftl/page/page-gc.c:317-321`).
        assert_eq!(
            unsafe { ioctl(&mut flash, PAGE_FTL_IOCTL_TRIM as c_uint) },
            0
        );
        assert_eq!(f.free(1), PAGES_PER_SEGMENT as i32);
        assert_eq!(f.valid(1), 0);
        assert!(!f.gc_bit(1));
        assert_eq!(f.gc_list_len(), 0);

        assert_eq!(
            unsafe { ioctl(&mut flash, PAGE_FTL_IOCTL_TRIM as c_uint + 7) },
            -libc::EINVAL
        );
        assert_eq!(
            unsafe { ioctl(ptr::null_mut(), PAGE_FTL_IOCTL_TRIM as c_uint) },
            -libc::EINVAL
        );
    }

    /// The pair the module dispatch calls: a bad device module number leaves no
    /// FTL state behind, and a repeated exit is a no-op.
    #[test]
    fn module_init_and_exit_report_their_failures() {
        assert_eq!(
            unsafe { ffi_page_ftl_module_init(ptr::null_mut(), 0) },
            -libc::EINVAL
        );

        let mut bare = flash_device {
            f_op: ptr::null(),
            f_private: ptr::null_mut(),
            f_submodule_exit: None,
        };
        assert_eq!(
            unsafe { ffi_page_ftl_module_init(&mut bare, u64::MAX) },
            -libc::EINVAL
        );
        assert_eq!(bare.f_op, &PAGE_FOPS as *const flash_operations);
        assert!(bare.f_private.is_null(), "no FTL state was installed");
        assert!(bare.f_submodule_exit.is_none());

        assert_eq!(unsafe { ffi_page_ftl_module_exit(&mut bare) }, 0);
        assert_eq!(unsafe { ffi_page_ftl_module_exit(ptr::null_mut()) }, 0);

        // The full dispatch pairs with the exit it installs, open or not.
        let mut flash: *mut flash_device = ptr::null_mut();
        assert_eq!(
            unsafe { ffi_module_init(PAGE_FTL_MODULE, &mut flash, DeviceModule::Ramdisk as u64) },
            0
        );
        assert_eq!(unsafe { ffi_module_exit(flash) }, 0);
    }

    /// Every chunk takes one page, so a transfer of one page more than a
    /// segment holds fills the first segment and starts the next one.
    #[test]
    fn each_chunk_takes_one_page_of_one_segment() {
        let mut f = Fixture::new(NR_SEGMENTS, PAGES_PER_SEGMENT);
        let mut flash = flash_of(&mut f);
        let mut payload = vec![7u8; PAGES_PER_SEGMENT * DEVICE_PAGE_SIZE + 1];

        assert_eq!(
            unsafe {
                op(PAGE_FOPS.write)(
                    &mut flash,
                    payload.as_mut_ptr() as *mut c_void,
                    payload.len(),
                    0,
                )
            },
            payload.len() as ssize_t
        );

        assert_eq!(f.pgftl.segments[0].free_pages(), 0);
        assert_eq!(f.pgftl.segments[1].free_pages(), PAGES_PER_SEGMENT as i32 - 1);
        assert_eq!(
            f.pgftl.trans_map[PAGES_PER_SEGMENT],
            device_address::from_segment_page(1, 0).lpn
        );
    }
}
