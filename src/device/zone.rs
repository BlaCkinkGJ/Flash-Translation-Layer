//! Zoned block device backend ported from `device/zone/zone.c`.
//!
//! [`ZoneBackend`] drives a host-managed zoned block device through libzbd.
//! `open` keeps C's two descriptors (a read-only one and a write-only one
//! opened with `O_DIRECT`), lists every zone once and caches the array. The
//! request paths move whole pages with `pread` / `pwrite` through a
//! page-aligned bounce buffer. `erase` resets one zone and rewinds its cached
//! write pointer.
//!
//! `device/zone/zone.c` is deleted, so [`DeviceOperations`] holds the logic and
//! [`ffi_zone_device_init`] installs it through the shared backend machinery in
//! [`crate::device`]. The C line numbers below are from that file as it stood
//! before the port (456 lines).
//!
//! # libzbd binding
//!
//! libzbd is bound by hand, with `extern "C"` declarations plus
//! `#[link(name = "zbd")]` for the entry points the port needs. The two ABI
//! structs it passes get `#[repr(C)]` mirrors. Three of the names the C source
//! uses are not linkable symbols in libzbd 2.0.4:
//! `nm -D --defined-only libzbd.so.2` exports exactly `zbd_close`,
//! `zbd_device_is_zoned`, `zbd_device_model_str`, `zbd_get_info`,
//! `zbd_list_zones`, `zbd_open`, `zbd_report_zones`, `zbd_set_log_level`,
//! `zbd_zone_cond_str`, `zbd_zone_type_str` and `zbd_zones_operation`.
//!
//! - `zbd_reset_zones` and `zbd_finish_zones` are `static inline` wrappers over
//!   `zbd_zones_operation` (`libzbd/zbd.h`), so the port calls that exported
//!   function with the same `ZBD_OP_RESET` / `ZBD_OP_FINISH` operation code and
//!   returns what C's call through the wrapper returns.
//! - `zbd_zone_type(z)` is `#define zbd_zone_type(z) ((z)->type)` (`zbd.h`), so
//!   the port reads the `type` field of the mirrored zone.
//!
//! The narrow C wrapper the issue allows as a fallback is for a header whose
//! fields are not reachable from Rust. They are reachable here. `struct
//! zbd_zone` and `struct zbd_info` are plain C structs whose layout
//! `#[repr(C)]` reproduces byte for byte. Their `__attribute__((packed))`
//! changes nothing, because every member is naturally aligned either way. The
//! size assertions below pin that, so no `zbd_wrap.c` and no extra build step.
//!
//! # Geometry
//!
//! C's `zone_open` builds the device geometry from the build's
//! `DEVICE_NR_*_BITS`. `USE_ZONE_DEVICE=1` makes the Makefile compile with
//! `3/3/5/21` where the default build uses `2/2/7/19`. It also turns on the
//! `zone` cargo feature, which now selects the same counts in
//! [`crate::device`]. `open` writes C's `nr_bus = 8`, `nr_chips = 8`,
//! `nr_pages = 32`, `page_size = 8192` and `nr_blocks = nr_zones` on both
//! sides. Both geometries put the block field at bit 11 and give a segment 2048
//! pages. The zoned build therefore only changes how the low 11 bits split
//! between bus, chip and page.
//!
//! Deliberate deviations from the C original:
//!
//! - C's `struct zone_meta` (`include/zone.h`) is not mirrored. Its `struct
//!   zbd_info` member is never written or read (`zone_open` passes a local
//!   `struct zbd_info` to `zbd_open`), and `block_size` is stored and never
//!   read. `struct zone_file_descriptor` wraps one `int`. The state lives in
//!   [`ZoneState`], with the two descriptors flattened and the zone array owned
//!   by a `Vec`.
//! - The zone array comes from libzbd's `calloc` (`zbd.c`, `zbd_list_zones`),
//!   so `Vec::from_raw_parts` adopts it and the default system allocator
//!   releases it with `free`, which is C's `free(meta->zones)` at `zone.c:386`.
//! - Re-opening releases the previous descriptors and zone array. C overwrites
//!   `meta->read.fd` (`zone.c:55`), `meta->write.fd` (`zone.c:61`) and
//!   `meta->zones` (`zone.c:93`) and leaks all three.
//! - `open` returns `-EINVAL` when `zbd_list_zones` reports a zone count other
//!   than the device's (`zone.c:95`), and when a zone is not
//!   `ZBD_ZONE_TYPE_SWR` (`zone.c:102`). C prints the error, jumps to the
//!   cleanup label and returns `ret`, which is still 0 on both paths. A caller
//!   that trusted it would use a device whose zones were just freed.
//! - A bounce buffer allocation failure returns `-ENOMEM`. C returns
//!   `posix_memalign`'s positive `errno` (`zone.c:199`, `zone.c:302`), which a
//!   caller reading a byte count takes for success.
//! - `zone_do_rw` advances the buffer across a short transfer. C passes the
//!   same `buffer` to every `pread` / `pwrite` (`zone.c:134`, `zone.c:136`), so
//!   a short transfer overwrites the start of the buffer and sends or reads the
//!   wrong bytes.
//! - `zone_do_rw` reads `errno` before it logs the failure, so the returned
//!   code is the one the failing call set. C logs first and reads `errno`
//!   afterwards (`zone.c:143-145`), so the `errno` it returns can be the one
//!   the log call itself left behind. `open` reads it first on both sides
//!   (`zone.c:57`, `zone.c:63`).
//! - A failed zone finish returns its negative status. C jumps to the exit
//!   label with `ret` still holding the byte count (`zone.c:232-240`), so the
//!   write reports success after the zone was left open.
//! - A zone index past the cached array returns `-EINVAL`. C indexes
//!   `meta->zones` unchecked (`zone.c:214`, `zone.c:358`), and `zone_close`
//!   clears that pointer (`zone.c:387`), so a stale index dereferences `NULL`.
//! - `ffi_zone_device_exit` clears `d_op` as well as `d_private`, through
//!   [`device::uninstall_backend`], where C leaves `d_op` pointing at the
//!   vtable of the metadata it just freed (`zone.c:450-452`).
//! - `ffi_zone_device_init` cannot report `-ENOMEM`. C returns it when its
//!   `malloc` fails and leaves `d_op` untouched (`zone.c:419-424`).
//!   `install_backend` allocates with `Box`, which aborts on OOM. That is the
//!   whole-module behaviour [`crate::device`] already documents for
//!   `ffi_device_module_init`.
//! - The port has no lock, matching C: concurrent requests to one device are
//!   the caller's responsibility. The `unsafe impl Sync` below records that
//!   contract for Rust callers.
//!
//! # Tests
//!
//! The request paths are unit tested against a plain file descriptor, which is
//! all `zone_do_rw` and the write pointer arithmetic need. The emulator routine
//! at the bottom of this file covers the libzbd calls end to end. It loads
//! nothing itself. It drives a zoned device a developer created with the
//! `null_blk` emulator through `make zone-smoke`.

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::mem::{size_of, zeroed};
use core::ptr;
use core::slice;
use std::alloc::{alloc, dealloc, Layout};
use std::cell::UnsafeCell;
use std::io;

use crate::device::{
    device, device_request, DeviceDirection, DeviceOperations, PADDR_EMPTY,
    DEVICE_NR_BUS_BITS, DEVICE_NR_CHIPS_BITS, DEVICE_NR_PAGES_BITS,
    DEVICE_PAGE_SIZE,
};
use crate::pr_err;

/// `ZBD_RO_ALL`, `enum zbd_report_option` in `libzbd/zbd.h`.
const ZBD_RO_ALL: c_uint = 0x00;

/// `ZBD_DM_HOST_MANAGED`, `enum zbd_dev_model` in `libzbd/zbd.h`.
const ZBD_DM_HOST_MANAGED: c_uint = 1;

/// `ZBD_ZONE_TYPE_SWR`, which is `BLK_ZONE_TYPE_SEQWRITE_REQ` from
/// `linux/blkzoned.h`.
const ZBD_ZONE_TYPE_SWR: c_uint = 0x2;

/// `ZBD_OP_RESET`, `enum zbd_zone_op` in `libzbd/zbd.h`.
const ZBD_OP_RESET: c_int = 0x01;

/// `ZBD_OP_FINISH`, `enum zbd_zone_op` in `libzbd/zbd.h`.
const ZBD_OP_FINISH: c_int = 0x04;

/// `BLK_ZONE_COND_FULL` from `linux/blkzoned.h`, the condition a zone reports
/// after `ZBD_OP_FINISH`. The emulator routine checks it.
#[cfg(test)]
const ZBD_ZONE_COND_FULL: c_uint = 0x0e;

/// `struct zbd_info` from `libzbd/zbd.h`, the device description `zbd_open`
/// fills in. Every field is declared, because `zbd_open` copies the whole
/// struct into the caller's storage.
#[repr(C)]
#[derive(Clone, Copy)]
struct ZbdInfo {
    vendor_id: [c_char; 32],
    nr_sectors: u64,
    nr_lblocks: u64,
    nr_pblocks: u64,
    zone_size: u64,
    zone_sectors: c_uint,
    lblock_size: c_uint,
    pblock_size: c_uint,
    nr_zones: c_uint,
    max_nr_open_zones: c_uint,
    max_nr_active_zones: c_uint,
    model: c_uint,
    reserved: [u8; 36],
}

/// `struct zbd_zone` from `libzbd/zbd.h`, one zone description. `wp` is a byte
/// offset from the start of the device, not from the start of the zone.
#[repr(C)]
#[derive(Clone, Copy)]
struct ZbdZone {
    start: u64,
    len: u64,
    capacity: u64,
    wp: u64,
    flags: c_uint,
    type_: c_uint,
    cond: c_uint,
    reserved: [u8; 20],
}

// The two mirrored structs are `__attribute__((packed))` in `zbd.h`, and are
// 128 and 64 bytes long. Their members are naturally aligned either way, so
// `#[repr(C)]` reproduces the layout exactly. These assertions fail the build
// if a newer libzbd ever adds a field.
const _: () = assert!(size_of::<ZbdInfo>() == 128);
const _: () = assert!(size_of::<ZbdZone>() == 64);

// The exported half of libzbd's ABI that this backend uses. The module docs
// explain why `zbd_reset_zones`, `zbd_finish_zones` and `zbd_zone_type` are
// not declared here. Rustdoc does not document `extern` blocks, so the
// description of each call lives on the wrapper that uses it.
#[link(name = "zbd")]
extern "C" {
    // `zbd_open` returns a descriptor, or a negative value and leaves `errno`
    // set.
    fn zbd_open(
        filename: *const c_char,
        flags: c_int,
        info: *mut ZbdInfo,
    ) -> c_int;

    // `zbd_close` closes a descriptor from `zbd_open` and returns nothing.
    fn zbd_close(fd: c_int);

    // `zbd_list_zones` allocates the zone array with `calloc` and hands it to
    // the caller, who releases it with `free`.
    fn zbd_list_zones(
        fd: c_int,
        ofst: libc::off_t,
        len: libc::off_t,
        ro: c_uint,
        zones: *mut *mut ZbdZone,
        nr_zones: *mut c_uint,
    ) -> c_int;

    // `zbd_zones_operation` runs `op` on every zone in `[ofst, ofst + len)`.
    // It returns 0, or -1 with `errno` set.
    fn zbd_zones_operation(
        fd: c_int,
        op: c_int,
        ofst: libc::off_t,
        len: libc::off_t,
    ) -> c_int;
}

/// `errno` as C reads it after a failed call, for C's `ret = -errno`.
fn last_errno() -> c_int {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// C's `struct zone_meta` state, one per open device.
///
/// The descriptors start at -1, the value C's `zone_device_init` writes
/// (`zone.c:425-426`) so that a `close` before `open` is a no-op.
struct ZoneState {
    total_size: usize,
    zone_size: u64,
    nr_zones: u64,
    o_flags: c_int,
    read_fd: c_int,
    write_fd: c_int,
    zones: Vec<ZbdZone>,
}

impl ZoneState {
    fn new() -> Self {
        Self {
            total_size: 0,
            zone_size: 0,
            nr_zones: 0,
            o_flags: 0,
            read_fd: -1,
            write_fd: -1,
            zones: Vec::new(),
        }
    }
}

/// Page-aligned scratch buffer for the `O_DIRECT` write descriptor. It matches
/// C's `posix_memalign(..., sysconf(_SC_PAGESIZE), page_size)` plus the `free`
/// at its single exit label (`zone.c:199`, `zone.c:247`, `zone.c:302`,
/// `zone.c:319`).
///
/// The buffer is deliberately not zeroed here. The write path fills it with
/// the request payload before any byte of it is read, and the read path zeroes
/// it the way C's `memset` does (`zone.c:309`).
struct AlignedBuffer {
    ptr: *mut u8,
    layout: Layout,
}

impl AlignedBuffer {
    /// `size` bytes aligned to the system page size, or `None` when the
    /// alignment or the allocation fails, which is `posix_memalign` returning
    /// non-zero.
    fn new(size: usize) -> Option<Self> {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let align = page.max(1) as usize;
        let layout = Layout::from_size_align(size, align).ok()?;
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            return None;
        }
        Some(Self { ptr, layout })
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) };
    }
}

/// Zoned block device backend. See the module docs.
pub struct ZoneBackend {
    state: UnsafeCell<ZoneState>,
}

// Every method that touches the cell is `unsafe`, so safe code cannot race
// through a shared `&ZoneBackend`. The marker states the backend contract from
// `DeviceOperations`: a backend is callable from any thread, and the caller
// must not issue overlapping requests. C's zoned backend makes the same deal.
// `UnsafeCell` is what would otherwise make this `!Sync`.
unsafe impl Sync for ZoneBackend {}

impl ZoneBackend {
    fn new() -> Self {
        Self {
            state: UnsafeCell::new(ZoneState::new()),
        }
    }

    /// C's `zone_close`: close both descriptors, then release the zone array,
    /// in that order (`zone.c:378-389`). A `NULL` metadata is a no-op there. An
    /// unopened backend has -1 descriptors, so this is idempotent.
    ///
    /// # Safety
    /// No other borrow of the state cell is live.
    unsafe fn close_state(&self) {
        let state = &mut *self.state.get();
        if state.read_fd >= 0 {
            zbd_close(state.read_fd);
            state.read_fd = -1;
        }
        if state.write_fd >= 0 {
            zbd_close(state.write_fd);
            state.write_fd = -1;
        }
        state.zones = Vec::new();
    }

    /// C's `exception:` label of `zone_open`: close what the attempt opened,
    /// then return its error (`zone.c:109-112`).
    ///
    /// # Safety
    /// No other borrow of the state cell is live.
    unsafe fn close_after_failed_open(&self, ret: c_int) -> c_int {
        self.close_state();
        ret
    }
}

impl DeviceOperations for ZoneBackend {
    /// Open the zoned block device and cache its zones. Mirrors C's
    /// `zone_open`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn open(
        &self,
        dev: *mut device,
        name: *const c_char,
        flags: c_int,
    ) -> c_int {
        (*dev).info.nr_bus = 1usize << DEVICE_NR_BUS_BITS;
        (*dev).info.nr_chips = 1usize << DEVICE_NR_CHIPS_BITS;
        (*dev).info.package.block.nr_pages = 1usize << DEVICE_NR_PAGES_BITS;
        (*dev).info.package.block.page.size = DEVICE_PAGE_SIZE;

        // C overwrites the descriptors and the zone array it is about to
        // replace (`zone.c:55`, `zone.c:61`, `zone.c:93`) and leaks all three.
        self.close_state();
        (*self.state.get()).o_flags = flags;

        let mut info: ZbdInfo = zeroed();
        {
            let state = &mut *self.state.get();
            state.read_fd = zbd_open(name, libc::O_RDONLY, &mut info);
            if state.read_fd < 0 {
                // C reads `errno` before it logs (`zone.c:57`).
                let ret = -last_errno();
                pr_err!("zone read file descriptor open failed");
                return self.close_after_failed_open(ret);
            }
        }
        {
            let state = &mut *self.state.get();
            state.write_fd =
                zbd_open(name, libc::O_WRONLY | libc::O_DIRECT, &mut info);
            if state.write_fd < 0 {
                let ret = -last_errno();
                pr_err!("zone write file descriptor open failed");
                return self.close_after_failed_open(ret);
            }
        }

        let total_size = info.nr_zones as usize * info.zone_size as usize;
        {
            let state = &mut *self.state.get();
            state.total_size = total_size;
        }
        if flags & libc::O_CREAT != 0 {
            let fd = (*self.state.get()).write_fd;
            let ret = zbd_zones_operation(
                fd,
                ZBD_OP_RESET,
                0,
                total_size as libc::off_t,
            );
            if ret != 0 {
                pr_err!("zone reset failed");
                return self.close_after_failed_open(ret);
            }
        }

        let page_size = (*dev).page_size();
        {
            let state = &mut *self.state.get();
            state.zone_size = info.zone_size;
            state.nr_zones = info.nr_zones as u64;
            (*dev).info.package.nr_blocks = state.nr_zones as usize;
            if !state.zone_size.is_multiple_of(page_size as u64) {
                pr_err!("zone size is not aligned {} bytes", page_size);
                return self.close_after_failed_open(-libc::EINVAL);
            }
        }
        if info.model != ZBD_DM_HOST_MANAGED {
            pr_err!("not host managed");
            return self.close_after_failed_open(-libc::EINVAL);
        }

        let nr_zones = (*self.state.get()).nr_zones;
        let mut zones: *mut ZbdZone = ptr::null_mut();
        let mut reported: c_uint = 0;
        let ret = zbd_list_zones(
            (*self.state.get()).read_fd,
            0,
            total_size as libc::off_t,
            ZBD_RO_ALL,
            &mut zones,
            &mut reported,
        );
        if ret != 0 || u64::from(reported) != nr_zones {
            pr_err!("failed to list zones, err {}", ret);
            if !zones.is_null() {
                // libzbd allocated `reported` entries with `calloc`, so adopt
                // them to release them, which is C's `free(meta->zones)`.
                drop(Vec::from_raw_parts(
                    zones,
                    reported as usize,
                    reported as usize,
                ));
            }
            return self.close_after_failed_open(-libc::EINVAL);
        }
        let zones = if zones.is_null() {
            Vec::new()
        } else {
            Vec::from_raw_parts(zones, reported as usize, reported as usize)
        };
        for zone in &zones {
            if zone.type_ != ZBD_ZONE_TYPE_SWR {
                pr_err!("zone type is not sequential mode");
                return self.close_after_failed_open(-libc::EINVAL);
            }
        }
        (*self.state.get()).zones = zones;
        0
    }

    /// Write one page at the zone's write pointer. Mirrors C's `zone_write`.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn write(
        &self,
        dev: *mut device,
        request: *mut device_request,
    ) -> isize {
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
        if (*request).paddr.lpn == PADDR_EMPTY {
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

        let data_len = (*request).data_len;
        let Some(mut buffer) = AlignedBuffer::new(page_size) else {
            pr_err!("buffer allocation failed");
            return -(libc::ENOMEM as isize);
        };
        let payload =
            slice::from_raw_parts((*request).data as *const u8, data_len);
        buffer.as_mut_slice().copy_from_slice(payload);

        let lpn = (*request).paddr.lpn;
        let zone_num = (*request).paddr.block() as u64;
        let state = &mut *self.state.get();
        if zone_num >= state.nr_zones {
            pr_err!("invalid address value detected (lpn: {})", lpn);
            return -(libc::EINVAL as isize);
        }
        let Some(zone) = state.zones.get_mut(zone_num as usize) else {
            pr_err!("invalid address value detected (lpn: {})", lpn);
            return -(libc::EINVAL as isize);
        };
        let expected = lpn as u64 * page_size as u64;
        if zone.wp != expected {
            pr_err!(
                "write pointer doesn't match (expected: {}, actual: {}, \
                 zone: {})",
                expected,
                zone.wp,
                zone_num
            );
            return -(libc::EINVAL as isize);
        }

        let ret = zone_do_rw(
            state.write_fd,
            (*request).flag,
            buffer.as_mut_ptr(),
            data_len,
            lpn as libc::off_t * page_size as libc::off_t,
        );
        if ret != page_size as isize {
            pr_err!(
                "do io sequence failed(expected: {}, actual: {})",
                ret,
                page_size
            );
            return -(libc::EFAULT as isize);
        }
        zone.wp += ret as u64;
        if zone.wp == zone.start + zone.len {
            let status = zbd_zones_operation(
                state.write_fd,
                ZBD_OP_FINISH,
                zone.start as libc::off_t,
                zone.len as libc::off_t,
            );
            if status != 0 {
                pr_err!(
                    "zone close failed (start:{}, len: {})",
                    zone.start,
                    zone.len
                );
                return status as isize;
            }
        }
        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        ret
    }

    /// Read one page. Mirrors C's `zone_read`, including its fallthrough on a
    /// request whose flag is not `DEVICE_READ` (`zone.c:277-280`). The wrong
    /// flag reaches `zone_do_rw`, which fails against the descriptor, and the
    /// zeroed bounce buffer is copied to the caller anyway (`zone.c:313`).
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn read(
        &self,
        dev: *mut device,
        request: *mut device_request,
    ) -> isize {
        let page_size = (*dev).page_size();

        if (*request).data.is_null() {
            pr_err!("NULL data pointer detected");
            return -(libc::ENODATA as isize);
        }
        if (*request).direction() != Some(DeviceDirection::Read) {
            pr_err!(
                "request type is not matched (expected: {}, current: {})",
                DeviceDirection::Read as c_uint,
                (*request).flag
            );
        }
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

        let lpn = (*request).paddr.lpn;
        let zone_num = (*request).paddr.block() as u64;
        let state = &mut *self.state.get();
        if zone_num >= state.nr_zones {
            pr_err!("invalid address value detected (lpn: {})", lpn);
            return -(libc::EINVAL as isize);
        }
        let Some(mut buffer) = AlignedBuffer::new(page_size) else {
            pr_err!("buffer allocation failed");
            return -(libc::ENOMEM as isize);
        };
        buffer.as_mut_slice().fill(0);
        let ret = zone_do_rw(
            state.read_fd,
            (*request).flag,
            buffer.as_mut_ptr(),
            (*request).data_len,
            lpn as libc::off_t * page_size as libc::off_t,
        );
        slice::from_raw_parts_mut((*request).data as *mut u8, page_size)
            .copy_from_slice(buffer.as_mut_slice());
        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        ret
    }

    /// Erase one zone: reset it, then rewind its cached write pointer. Mirrors
    /// C's `zone_erase`, which does not look at the request's flag.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into and `request` is a
    /// live request.
    unsafe fn erase(
        &self,
        _dev: *mut device,
        request: *mut device_request,
    ) -> c_int {
        let zone_num = (*request).paddr.block() as u64;
        let state = &mut *self.state.get();
        if zone_num >= state.nr_zones {
            pr_err!("invalid address detected");
            return -libc::EINVAL;
        }
        if zone_num as usize >= state.zones.len() {
            pr_err!("invalid address detected");
            return -libc::EINVAL;
        }

        let offset = state.zone_size * zone_num;
        let length = state.zone_size;
        let ret = zbd_zones_operation(
            state.write_fd,
            ZBD_OP_RESET,
            offset as libc::off_t,
            length as libc::off_t,
        );
        if ret != 0 {
            pr_err!("zone reset failed ({} ~ {})", offset, offset + length);
            return ret;
        }
        if let Some(end_rq) = (*request).end_rq {
            end_rq(request);
        }
        let zone = &mut state.zones[zone_num as usize];
        zone.wp = zone.start;
        0
    }

    /// Close both descriptors and release the zone array. Mirrors C's
    /// `zone_close`, which always returns 0.
    ///
    /// # Safety
    /// `dev` is the device this backend was installed into.
    unsafe fn close(&self, _dev: *mut device) -> c_int {
        self.close_state();
        0
    }
}

/// C's `zone_do_rw`: move `count` bytes at `offset`, looping while the
/// descriptor reports a short transfer.
///
/// # Safety
/// `fd` is an open descriptor and `buffer` points at `count` writable bytes.
unsafe fn zone_do_rw(
    fd: c_int,
    flag: c_uint,
    buffer: *mut u8,
    count: usize,
    offset: libc::off_t,
) -> isize {
    let mut remaining = count;
    let mut ofst = offset;

    while remaining != 0 {
        let done = count - remaining;
        let ret = if flag == DeviceDirection::Read as c_uint {
            libc::pread(fd, buffer.add(done) as *mut c_void, remaining, ofst)
        } else if flag == DeviceDirection::Write as c_uint {
            libc::pwrite(fd, buffer.add(done) as *mut c_void, remaining, ofst)
        } else {
            pr_err!("invalid flag detected (flag: {})", flag);
            return -(libc::EINVAL as isize);
        };
        if ret < 0 {
            // C logs first and reads `errno` afterwards (`zone.c:143-145`), so
            // the `errno` it returns can be the one the log call left behind.
            // The port reads it first, which is the code C means to report.
            let errno = last_errno();
            pr_err!(
                "{} failed {} ({})",
                if flag == DeviceDirection::Read as c_uint {
                    "read"
                } else {
                    "write"
                },
                errno,
                io::Error::last_os_error()
            );
            return -(errno as isize);
        }
        if ret == 0 {
            break;
        }
        remaining -= ret as usize;
        ofst += ret as libc::off_t;
    }
    (count - remaining) as isize
}

/// Init routine of the zone module, C's `zone_device_init`.
///
/// # Safety
/// `dev` is `NULL` or a live device with no backend installed yet.
#[no_mangle]
pub unsafe extern "C" fn ffi_zone_device_init(
    dev: *mut device,
    flags: u64,
) -> c_int {
    let _ = flags;
    if dev.is_null() {
        return -libc::EINVAL;
    }
    (*dev).install_backend(ZoneBackend::new());
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
    // C calls `zone_close` before it frees the metadata (`zone.c:450`), so the
    // descriptors and the zone array are released while the backend is still
    // installed. Reach it through the vtable, which is what a C caller does.
    if let Some(close) = (*dev).d_op.as_ref().and_then(|ops| ops.close) {
        let _ = close(dev);
    }
    (*dev).uninstall_backend::<ZoneBackend>();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        device_address, ffi_device_module_init, ffi_device_module_exit,
        DeviceModule, Request,
    };
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::ffi::CString;
    use std::path::Path;

    /// One page of the zoned build, 8192 bytes.
    const PAGE: usize = DEVICE_PAGE_SIZE;

    /// 16 MiB, the zone size C's write pointer check needs. That is one segment
    /// of `DEVICE_NR_PAGES_BITS` pages, so the byte offset the FTL computes for
    /// a page inside a segment equals its offset inside the zone.
    const ZONE: u64 = 16 << 20;

    /// A zone description as `zbd_list_zones` fills it in.
    fn synthetic_zone(start: u64, len: u64, wp: u64) -> ZbdZone {
        ZbdZone {
            start,
            len,
            capacity: len,
            wp,
            flags: 0,
            type_: ZBD_ZONE_TYPE_SWR,
            cond: 0x1,
            reserved: [0; 20],
        }
    }

    fn zeroed_device() -> device {
        unsafe { core::mem::MaybeUninit::<device>::zeroed().assume_init() }
    }

    /// A device whose backend reports `nr_zones` synthetic zones of
    /// `zone_size` bytes and holds no descriptors. A successful `open` would
    /// leave the same fields behind, so the request paths can run without a
    /// zoned block device.
    unsafe fn offline_backend(
        dev: &mut device,
        backend: &ZoneBackend,
        nr_zones: u64,
        zone_size: u64,
    ) {
        dev.info.nr_bus = 1usize << DEVICE_NR_BUS_BITS;
        dev.info.nr_chips = 1usize << DEVICE_NR_CHIPS_BITS;
        dev.info.package.nr_blocks = nr_zones as usize;
        dev.info.package.block.nr_pages = 1usize << DEVICE_NR_PAGES_BITS;
        dev.info.package.block.page.size = DEVICE_PAGE_SIZE;

        let state = &mut *backend.state.get();
        state.total_size = (nr_zones * zone_size) as usize;
        state.zone_size = zone_size;
        state.nr_zones = nr_zones;
        state.zones = (0..nr_zones)
            .map(|i| synthetic_zone(i * zone_size, zone_size, i * zone_size))
            .collect();
    }

    /// `O_CREAT` and `O_RDWR` on a file in the temporary directory, which is
    /// all `zone_do_rw` needs to run a real transfer. `tag` keeps the tests
    /// that run in parallel off each other's file.
    unsafe fn scratch_file(tag: &str) -> (CString, c_int) {
        let path = std::env::temp_dir().join(format!(
            "ftl-zone-test-{}-{tag}",
            std::process::id()
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("temp path has no NUL");
        let fd = libc::open(
            path.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC,
            0o600,
        );
        assert!(fd >= 0, "open the scratch file");
        (path, fd)
    }

    #[test]
    fn init_and_exit_wire_the_backend() {
        unsafe {
            assert_eq!(ffi_zone_device_init(ptr::null_mut(), 0), -libc::EINVAL);
            assert_eq!(ffi_zone_device_exit(ptr::null_mut()), 0);

            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DeviceModule::Zone as u64, &mut dev, 0),
                0
            );
            assert!(!dev.is_null());
            assert!(!(*dev).d_op.is_null());
            assert!(!(*dev).d_private.is_null());
            assert!((*dev).badseg_bitmap.is_null());

            // The vtable's close runs before the backend is released.
            let ops = &*(*dev).d_op;
            assert_eq!((ops.close.unwrap())(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn open_reports_a_missing_device_and_keeps_the_backend() {
        unsafe {
            let mut dev: *mut device = ptr::null_mut();
            assert_eq!(
                ffi_device_module_init(DeviceModule::Zone as u64, &mut dev, 0),
                0
            );
            let missing = CString::new("/nonexistent/ftl-zone-device").unwrap();
            let ops = &*(*dev).d_op;

            // libzbd reports a device that is not a zoned block device as -1
            // with `errno` set, and C turns that into `-errno` (`zone.c:57`).
            // The code itself comes from libzbd's own error logging, so only
            // the sign is stable, on both sides.
            let open = ops.open.unwrap();
            assert!(open(dev, missing.as_ptr(), libc::O_RDWR) < 0);
            // C's `exception:` path runs `zone_close`, which releases the
            // descriptors and the zone array but not the metadata.
            assert!(!(*dev).d_private.is_null());
            assert_eq!((ops.close.unwrap())(dev), 0);
            assert_eq!(ffi_device_module_exit(dev), 0);
        }
    }

    #[test]
    fn write_rejects_bad_requests_and_a_stale_zone_index() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            offline_backend(&mut dev, &backend, 1, ZONE);
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; PAGE];
            request.flag = DeviceDirection::Write.into();
            request.paddr = device_address::from(0u32);
            request.data_len = PAGE;
            request.data = page.as_mut_ptr() as *mut c_void;

            // NULL data pointer.
            request.data = ptr::null_mut();
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::ENODATA as isize)

            );

            // Wrong direction.
            request.data = page.as_mut_ptr() as *mut c_void;
            request.flag = DeviceDirection::Read.into();
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // Unassigned physical address.
            request.flag = DeviceDirection::Write.into();
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // Short write.
            request.paddr = device_address::from(0u32);
            request.data_len = PAGE - 1;
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // A zone number past the device.
            request.data_len = PAGE;
            request.paddr = device_address::new(0, 0, 1, 0);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // A write pointer that does not match the address.
            request.paddr = device_address::from(1u32);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // A stale index with no cached zone array, which C dereferences.
            let state = &mut *backend.state.get();
            state.nr_zones = 1;
            state.zones.clear();
            request.paddr = device_address::from(0u32);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // An unopened device fails the transfer itself.
            offline_backend(&mut dev, &backend, 1, ZONE);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EFAULT as isize)

            );
        }
    }

    #[test]
    fn write_and_read_move_pages_through_a_descriptor() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            offline_backend(&mut dev, &backend, 1, ZONE);
            let (path, fd) = scratch_file("rw");
            {
                let state = &mut *backend.state.get();
                state.read_fd = fd;
                state.write_fd = fd;
            }

            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; PAGE];
            END_RQ_CALLS.store(0, Ordering::SeqCst);
            request.flag = DeviceDirection::Write.into();
            request.data_len = PAGE;
            request.data = page.as_mut_ptr() as *mut c_void;
            request.end_rq = Some(count_end_rq);

            for lpn in 0u32..2 {
                page[..4].copy_from_slice(&lpn.to_ne_bytes());
                request.paddr = device_address::from(lpn);
                assert_eq!(
                    backend.write(&mut dev, request.as_ptr()),
                    PAGE as isize
                );
            }
            // The write pointer moved, so the first page is no longer writable.
            request.paddr = device_address::from(0u32);
            assert_eq!(

                backend.write(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            request.flag = DeviceDirection::Read.into();
            for lpn in 0u32..2 {
                page.fill(0xff);
                request.paddr = device_address::from(lpn);
                assert_eq!(
                    backend.read(&mut dev, request.as_ptr()),
                    PAGE as isize
                );
                let stamp = u32::from_ne_bytes(page[..4].try_into().unwrap());
                assert_eq!(stamp, lpn);
            }
            assert_eq!(END_RQ_CALLS.load(Ordering::SeqCst), 4);

            // Past the end of the descriptor `pread` reports zero bytes, which
            // the loop turns into a zero byte transfer.
            page.fill(0xff);
            request.paddr = device_address::from(100u32);
            assert_eq!(backend.read(&mut dev, request.as_ptr()), 0);
            assert!(page.iter().all(|byte| *byte == 0));

            assert_eq!(backend.close(&mut dev), 0);
            libc::close(fd);
            libc::unlink(path.as_ptr());
        }
    }

    #[test]
    fn a_full_zone_is_finished_and_reports_the_failure() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            // Two pages make a zone that fills up, so the write pointer reaches
            // the end of the zone and the port runs `ZBD_OP_FINISH`.
            offline_backend(&mut dev, &backend, 1, 2 * PAGE as u64);
            let (path, fd) = scratch_file("finish");
            (*backend.state.get()).write_fd = fd;

            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; PAGE];
            request.flag = DeviceDirection::Write.into();
            request.data_len = PAGE;
            request.data = page.as_mut_ptr() as *mut c_void;
            request.end_rq = None;

            request.paddr = device_address::from(0u32);
            assert_eq!(
                backend.write(&mut dev, request.as_ptr()),
                PAGE as isize
            );
            // The scratch file is not a zoned device, so libzbd refuses the
            // finish and the port returns that status. C returns the byte
            // count here (`zone.c:232-240`).
            request.paddr = device_address::from(1u32);
            assert_eq!(backend.write(&mut dev, request.as_ptr()), -1);
            let state = &*backend.state.get();
            assert_eq!(state.zones[0].wp, 2 * PAGE as u64);

            libc::close(fd);
            libc::unlink(path.as_ptr());
        }
    }

    #[test]
    fn read_keeps_the_c_fallthrough_and_validates_the_request() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            offline_backend(&mut dev, &backend, 1, ZONE);
            let mut request = Request::new().expect("allocate a request");
            let mut page = vec![0u8; PAGE];
            request.data_len = PAGE;
            request.data = page.as_mut_ptr() as *mut c_void;
            request.paddr = device_address::from(0u32);
            request.end_rq = None;

            // NULL data pointer.
            request.data = ptr::null_mut();
            request.flag = DeviceDirection::Read.into();
            assert_eq!(

                backend.read(&mut dev, request.as_ptr()),

                -(libc::ENODATA as isize)

            );

            // Short read.
            request.data = page.as_mut_ptr() as *mut c_void;
            request.data_len = PAGE - 1;
            assert_eq!(

                backend.read(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // Unassigned physical address.
            request.data_len = PAGE;
            request.paddr = device_address::from(PADDR_EMPTY);
            assert_eq!(

                backend.read(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // A zone number past the device.
            request.paddr = device_address::new(0, 0, 1, 0);
            assert_eq!(

                backend.read(&mut dev, request.as_ptr()),

                -(libc::EINVAL as isize)

            );

            // A write flag falls through to the transfer, which fails against
            // the descriptor, and the zeroed bounce buffer is copied out
            // anyway.
            page.fill(0xff);
            request.paddr = device_address::from(0u32);
            request.flag = DeviceDirection::Write.into();
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EBADF as isize)
            );
            assert!(page.iter().all(|byte| *byte == 0));

            // An erase flag is neither direction, so the transfer rejects it
            // before it reaches the descriptor.
            request.flag = DeviceDirection::Erase.into();
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EINVAL as isize)
            );

            // A correct request against an unopened device fails the same way.
            request.flag = DeviceDirection::Read.into();
            assert_eq!(
                backend.read(&mut dev, request.as_ptr()),
                -(libc::EBADF as isize)
            );
        }
    }

    #[test]
    fn erase_validates_the_zone_and_reports_a_failed_reset() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            offline_backend(&mut dev, &backend, 1, ZONE);
            let mut request = Request::new().expect("allocate a request");
            request.flag = DeviceDirection::Erase.into();
            request.paddr = device_address::new(0, 0, 1, 0);
            assert_eq!(
                backend.erase(&mut dev, request.as_ptr()),
                -libc::EINVAL
            );

            // A stale index with no cached zone array.
            {
                let state = &mut *backend.state.get();
                state.zones.clear();
            }
            request.paddr = device_address::new(0, 0, 0, 0);
            assert_eq!(
                backend.erase(&mut dev, request.as_ptr()),
                -libc::EINVAL
            );

            // libzbd refuses the reset on an unopened device and returns -1.
            offline_backend(&mut dev, &backend, 1, ZONE);
            assert_eq!(backend.erase(&mut dev, request.as_ptr()), -1);
        }
    }

    #[test]
    fn close_releases_the_descriptors_and_is_idempotent() {
        unsafe {
            let mut dev = zeroed_device();
            let backend = ZoneBackend::new();
            offline_backend(&mut dev, &backend, 1, ZONE);
            let (path, fd) = scratch_file("close");
            {
                let state = &mut *backend.state.get();
                state.read_fd = fd;
                state.write_fd = fd;
            }

            assert_eq!(backend.close(&mut dev), 0);
            let state = &*backend.state.get();
            assert_eq!(state.read_fd, -1);
            assert_eq!(state.write_fd, -1);
            assert!(state.zones.is_empty());
            // `zbd_close` only closes a descriptor libzbd opened (`zbd.c`), so
            // this one is still open and the second close is a no-op.
            assert_eq!(libc::fcntl(fd, libc::F_GETFD), 0);
            assert_eq!(backend.close(&mut dev), 0);

            libc::close(fd);
            libc::unlink(path.as_ptr());
        }
    }

    static END_RQ_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_end_rq(_request: *mut device_request) {
        END_RQ_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    /// The emulator's own calls, apart from the counter the unit test reads, so
    /// a run that includes the ignored test cannot mix the two (#124).
    static EMULATOR_END_RQ_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_emulator_end_rq(_request: *mut device_request) {
        EMULATOR_END_RQ_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    /// The emulated-device routine the issue asks for. It is ignored by
    /// default because it needs a zoned block device: `make zone-smoke` loads
    /// the `null_blk` emulator and runs this test. Without the emulator it
    /// prints why and returns, so a developer can run the whole suite.
    #[test]
    #[ignore = "needs the null_blk emulator, run `make zone-smoke`"]
    fn emulator_writes_reads_and_resets_a_zone() {
        let path = std::env::var("FTL_ZONE_DEVICE")
            .unwrap_or_else(|_| "/dev/nullb0".to_string());
        if !Path::new(&path).exists() {
            println!(
                "SKIP: {path} is missing. Create the emulator with \
                 `sudo modprobe null_blk nr_devices=1 zoned=1 zone_size=16 \
                 size=64 memory_backed=1` or point FTL_ZONE_DEVICE at \
                 another zoned device."
            );
            return;
        }
        unsafe { emulator_cycle(&path) };
    }

    /// List the zones of the device at `path` with libzbd, as the array the
    /// caller releases with `free` and the descriptor it closes.
    ///
    /// # Safety
    /// `path` names a host-managed zoned block device.
    unsafe fn list_zones(path: &str) -> (*mut ZbdZone, c_uint, c_int) {
        let name = CString::new(path).expect("device path has no NUL");
        let mut info: ZbdInfo = zeroed();
        let fd = zbd_open(name.as_ptr(), libc::O_RDONLY, &mut info);
        assert!(fd >= 0, "zbd_open({path}) failed, errno {}", last_errno());

        let mut zones: *mut ZbdZone = ptr::null_mut();
        let mut reported: c_uint = 0;
        assert_eq!(
            zbd_list_zones(fd, 0, 0, ZBD_RO_ALL, &mut zones, &mut reported),
            0
        );
        (zones, reported, fd)
    }

    /// The zones `list_zones` handed back. libzbd leaves the array `NULL` when
    /// it reports none, and an empty slice still has to point somewhere valid.
    ///
    /// # Safety
    /// `zones` and `reported` come from one [`list_zones`] call and the array
    /// is not freed yet.
    unsafe fn zones_slice(
        zones: *mut ZbdZone,
        reported: c_uint,
    ) -> &'static [ZbdZone] {
        if zones.is_null() {
            &[]
        } else {
            slice::from_raw_parts(zones, reported as usize)
        }
    }

    /// Load the created device's geometry with libzbd, then open it through
    /// the ported backend, compare the two geometries, and run a write, read
    /// and reset cycle.
    ///
    /// # Safety
    /// `path` names a host-managed zoned block device with 16 MiB zones. The
    /// routine resets its zones.
    unsafe fn emulator_cycle(path: &str) {
        let name = CString::new(path).expect("device path has no NUL");

        // The geometry the C backend reported, read straight from libzbd.
        let mut info: ZbdInfo = zeroed();
        let probe = zbd_open(name.as_ptr(), libc::O_RDONLY, &mut info);
        assert!(probe >= 0, "zbd_open({path}) failed, errno {}", last_errno());
        println!(
            "emulator {path}: zone_size {} bytes, nr_zones {}, model {}, \
             pblock_size {}",
            info.zone_size, info.nr_zones, info.model, info.pblock_size
        );
        assert_eq!(
            info.model,
            ZBD_DM_HOST_MANAGED,
            "device is not host managed"
        );
        assert_eq!(
            info.zone_size,
            (1u64 << DEVICE_NR_PAGES_BITS)
                * (1u64 << DEVICE_NR_BUS_BITS)
                * (1u64 << DEVICE_NR_CHIPS_BITS)
                * PAGE as u64,
            "zone size must equal one segment of the zoned build"
        );
        zbd_close(probe);

        let mut dev: *mut device = ptr::null_mut();
        assert_eq!(
            ffi_device_module_init(DeviceModule::Zone as u64, &mut dev, 0),
            0
        );
        let ops = &*(*dev).d_op;

        // C's `zone_open` with `O_CREAT` resets every zone before it lists
        // them, so this also clears whatever a previous run left behind.
        assert_eq!(
            (ops.open.unwrap())(
                dev,
                name.as_ptr(),
                libc::O_CREAT | libc::O_RDWR
            ),
            0
        );

        // The geometry C's `zone_open` derives from the zoned build's
        // `DEVICE_NR_*_BITS` (Makefile, `USE_ZONE_DEVICE=1`).
        assert_eq!((*dev).info.nr_bus, 1 << DEVICE_NR_BUS_BITS);
        assert_eq!((*dev).info.nr_chips, 1 << DEVICE_NR_CHIPS_BITS);
        assert_eq!(
            (*dev).info.package.block.nr_pages,
            1 << DEVICE_NR_PAGES_BITS
        );
        assert_eq!((*dev).page_size(), PAGE);
        assert_eq!((*dev).nr_segments(), info.nr_zones as usize);
        println!(
            "ported geometry: nr_bus {}, nr_chips {}, nr_pages {}, page_size \
             {}, nr_blocks {}",
            (*dev).info.nr_bus,
            (*dev).info.nr_chips,
            (*dev).info.package.block.nr_pages,
            (*dev).page_size(),
            (*dev).nr_segments()
        );

        let pages_per_zone = info.zone_size as usize / PAGE;
        let mut request = Request::new().expect("allocate a request");
        let mut page = vec![0u8; PAGE];
        EMULATOR_END_RQ_CALLS.store(0, Ordering::SeqCst);
        request.flag = DeviceDirection::Write.into();
        request.data_len = PAGE;
        request.data = page.as_mut_ptr() as *mut c_void;
        request.end_rq = Some(count_emulator_end_rq);

        // Fill zone 0, which drives the write pointer to the end of the zone
        // and makes the port finish it.
        for lpn in 0..pages_per_zone as u32 {
            page[..4].copy_from_slice(&lpn.to_ne_bytes());
            request.paddr = device_address::from(lpn);
            assert_eq!(
                (ops.write.unwrap())(dev, request.as_ptr()),
                PAGE as isize,
                "write lpn {lpn}"
            );
        }
        // The finished zone rejects the page its write pointer has passed.
        request.paddr = device_address::from(0u32);
        assert_eq!(
            (ops.write.unwrap())(dev, request.as_ptr()),
            -(libc::EINVAL as isize)
        );

        request.flag = DeviceDirection::Read.into();
        for lpn in [0u32, pages_per_zone as u32 - 1] {
            page.fill(0xff);
            request.paddr = device_address::from(lpn);
            assert_eq!(
                (ops.read.unwrap())(dev, request.as_ptr()),
                PAGE as isize,
                "read lpn {lpn}"
            );
            assert_eq!(u32::from_ne_bytes(page[..4].try_into().unwrap()), lpn);
        }

        // The zone state libzbd reports: zone 0 is full and sequential-write
        // required, the rest are empty. A full zone's write pointer is not
        // applicable, and the drivers do not agree on how to say that. `zloop`
        // reports `ULLONG_MAX` (`zloop_mark_full`, `drivers/block/zloop.c`).
        // `null_blk` reports the end of the zone. libzbd scales either by the
        // sector size. So the check here is the condition, and the write
        // pointer is checked after the reset below, where it is a plain byte
        // offset.
        let (zones, reported, probe) = list_zones(path);
        let listed = zones_slice(zones, reported);
        assert_eq!(listed[0].cond, ZBD_ZONE_COND_FULL);
        assert_eq!(listed[0].type_, ZBD_ZONE_TYPE_SWR);
        assert_eq!(listed[1].cond, 0x1, "zone 1 should still be empty");
        println!(
            "zone 0 after the full write: cond {} (full), len {} bytes, \
             wp {:#x}",
            listed[0].cond, listed[0].len, listed[0].wp
        );
        let zone_start = listed[0].start;
        libc::free(zones as *mut c_void);
        zbd_close(probe);

        // Reset zone 0 and write it again, which is what the FTL does between
        // garbage collections.
        request.flag = DeviceDirection::Erase.into();
        request.paddr = device_address::new(0, 0, 0, 0);
        request.end_rq = None;
        assert_eq!((ops.erase.unwrap())(dev, request.as_ptr()), 0);

        request.flag = DeviceDirection::Write.into();
        request.end_rq = Some(count_emulator_end_rq);
        page[..4].copy_from_slice(&0xabcd_u32.to_ne_bytes());
        request.paddr = device_address::from(0u32);
        assert_eq!((ops.write.unwrap())(dev, request.as_ptr()), PAGE as isize);

        // The reset rewound the zone and the write moved it by one page. The
        // page is not read back a second time. The emulated device backs its
        // zones with regular files and serves the buffered read descriptor the
        // pages it cached before the reset, so a read of the page just written
        // returns the pre-reset content. A real zoned device has no such cache,
        // and the data path is checked by the read back above.
        let (zones, reported, probe) = list_zones(path);
        let listed = zones_slice(zones, reported);
        assert_eq!(listed[0].wp, listed[0].start + PAGE as u64);
        assert_eq!(listed[0].cond, 0x2, "zone 0 should be implicitly open");
        println!(
            "reset cycle: zone 0 starts at {} bytes, its write pointer is at \
             {} bytes, cond {} (implicitly open)",
            zone_start,
            listed[0].wp - listed[0].start,
            listed[0].cond
        );
        libc::free(zones as *mut c_void);
        zbd_close(probe);
        println!(
            "end_rq callbacks: {}",
            EMULATOR_END_RQ_CALLS.load(Ordering::SeqCst)
        );

        assert_eq!((ops.close.unwrap())(dev), 0);
        assert_eq!(ffi_device_module_exit(dev), 0);
    }
}
