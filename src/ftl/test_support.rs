//! Shared ramdisk fixture for the page FTL test modules (test builds only).
//!
//! The write and GC paths share the same setup: a real ramdisk through
//! the device layer, narrowed to a geometry small enough that a handful of
//! writes fills a segment, plus the page FTL state those paths read and write
//! (the L2P table, the segment array, the `use_bits` bitmaps and the
//! `gc_seg_bits` / `gc_list` bookkeeping). The state lives in the fixture and
//! reaches the FTL by raw pointer, which is how C owns it too.
//!
//! Geometry: the ramdisk sizes its store from the 1GiB geometry `open`
//! installs. The fixture then narrows `dev.info` to the requested geometry so a
//! test can fill a whole segment in a handful of writes. Every address the
//! narrowed geometry produces still lands inside the ramdisk's store, because
//! the packed address keeps the segment in the high bits and the store is sized
//! by the *default* page count.

use core::ffi::{c_char, c_int};
use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};
use std::io;

use crate::bits::get_bit;
use crate::device::ramdisk::{
    ffi_ramdisk_close, ffi_ramdisk_open, ffi_ramdisk_read, ffi_ramdisk_write,
};
use crate::device::{
    device, device_address, device_operations, device_request, ffi_device_module_exit,
    ffi_device_module_init, DeviceDirection, DeviceModule, Request, DEVICE_PAGE_SIZE, PADDR_EMPTY,
};
use crate::ftl::page_read::read_page;
use crate::ftl::page_write::write_page;
use crate::ftl::{PageFtl, PageFtlSegment};
use crate::list::{ffi_list_free, ffi_list_length};

/// `u64` words reserved for one segment's `use_bits` bitmap, so each segment
/// gets its own region of the shared allocation. C sizes the bitmap with
/// `BITS_TO_UINT64_ALIGN(pages_per_segment)` and this is comfortably past it
/// for every geometry a test uses.
const WORDS_PER_SEGMENT: usize = 64;

/// A real ramdisk device plus the page FTL state the page paths need.
pub struct Fixture {
    pub dev: *mut device,
    pub trans_map: Vec<u32>,
    pub segments: Vec<PageFtlSegment>,
    pub use_bits: Vec<u64>,
    pub gc_seg_bits: Vec<u64>,
    pub pgftl: Box<PageFtl>,
}

impl Fixture {
    /// A ramdisk FTL of `nr_segments` segments with `pages_per_segment` pages
    /// each.
    pub fn new(nr_segments: usize, pages_per_segment: usize) -> Self {
        assert!(nr_segments > 0 && pages_per_segment > 0);

        let mut dev: *mut device = ptr::null_mut();
        unsafe {
            assert_eq!(
                ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                0
            );
            assert_eq!(
                ffi_ramdisk_open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR),
                0
            );
            (*dev).info.nr_bus = 1;
            (*dev).info.nr_chips = 1;
            (*dev).info.package.nr_blocks = nr_segments;
            (*dev).info.package.block.nr_pages = pages_per_segment;
        }

        let mut trans_map = vec![PADDR_EMPTY; nr_segments * pages_per_segment + 1];
        let mut use_bits = vec![0u64; nr_segments * WORDS_PER_SEGMENT];
        let mut gc_seg_bits = vec![0u64; nr_segments.div_ceil(64)];
        let segments: Vec<PageFtlSegment> = (0..nr_segments)
            .map(|segnum| PageFtlSegment {
                nr_free_pages: AtomicI32::new(pages_per_segment as i32),
                nr_valid_pages: AtomicI32::new(0),
                is_gc: 0,
                use_bits: unsafe { use_bits.as_mut_ptr().add(segnum * WORDS_PER_SEGMENT) },
                lpn_list: ptr::null_mut(),
            })
            .collect();

        let mut pgftl =
            Box::new(unsafe { core::mem::MaybeUninit::<PageFtl>::zeroed().assume_init() });
        pgftl.dev = dev;
        pgftl.trans_map = trans_map.as_mut_ptr();
        pgftl.segments = segments.as_ptr() as *mut PageFtlSegment;
        pgftl.gc_seg_bits = gc_seg_bits.as_mut_ptr();
        // `page_ftl_open` initializes this mutex and the C ABI entry points
        // lock it. The fixture stands in for `open`, so it initializes it too.
        assert_eq!(
            unsafe { libc::pthread_mutex_init(ptr::addr_of_mut!(pgftl.mutex), ptr::null()) },
            0
        );

        Fixture {
            dev,
            trans_map,
            segments,
            use_bits,
            gc_seg_bits,
            pgftl,
        }
    }

    /// The raw pointer to segment `segnum`, for identity assertions against
    /// the pointer a list holds.
    pub fn segment_ptr(&self, segnum: usize) -> *const PageFtlSegment {
        &self.segments[segnum] as *const PageFtlSegment
    }

    /// The pointer held by the GC list head's node.
    pub fn gc_list_head(&self) -> *const PageFtlSegment {
        unsafe { (*self.pgftl.gc_list).data as *const PageFtlSegment }
    }

    pub fn free(&self, segnum: usize) -> i32 {
        self.segments[segnum].free_pages()
    }

    pub fn valid(&self, segnum: usize) -> i32 {
        self.segments[segnum].nr_valid_pages.load(Ordering::SeqCst)
    }

    pub fn lpn_list_len(&self, segnum: usize) -> usize {
        unsafe { ffi_list_length(self.segments[segnum].lpn_list) }
    }

    pub fn gc_list_len(&self) -> usize {
        unsafe { ffi_list_length(self.pgftl.gc_list) }
    }

    pub fn gc_bit(&self, segnum: usize) -> bool {
        get_bit(&self.gc_seg_bits, segnum as u64)
    }

    /// The first `u64` of a segment's `use_bits` bitmap.
    pub fn use_bits_word(&self, segnum: usize) -> u64 {
        self.use_bits[segnum * WORDS_PER_SEGMENT]
    }

    /// Run `body` with the device's read operation failing, then restore the
    /// real backend.
    ///
    /// # Safety
    /// The body must not leave a request in flight.
    pub unsafe fn with_failing_read<T>(&mut self, body: impl FnOnce(&mut Fixture) -> T) -> T {
        self.with_op(
            device_operations {
                open: None,
                write: None,
                read: Some(failing_read),
                erase: None,
                close: None,
            },
            body,
        )
    }

    /// Run `body` with the device's erase operation failing, then restore the
    /// real backend.
    ///
    /// # Safety
    /// The body must not leave a request in flight.
    pub unsafe fn with_failing_erase<T>(&mut self, body: impl FnOnce(&mut Fixture) -> T) -> T {
        self.with_op(
            device_operations {
                open: None,
                write: None,
                read: None,
                erase: Some(failing_erase),
                close: None,
            },
            body,
        )
    }

    /// Swap the operations `ops` names for the ramdisk's, run `body`, then
    /// restore the real backend. Every `None` field keeps the ramdisk's
    /// operation, so a caller names only the one it wants to break.
    unsafe fn with_op<T>(
        &mut self,
        ops: device_operations,
        body: impl FnOnce(&mut Fixture) -> T,
    ) -> T {
        let saved = (*self.dev).d_op;
        let real = &*saved;
        let broken = device_operations {
            open: real.open,
            write: ops.write.or(real.write),
            read: ops.read.or(real.read),
            erase: ops.erase.or(real.erase),
            close: real.close,
        };
        (*self.dev).d_op = &broken;
        let ret = body(self);
        (*self.dev).d_op = saved;
        ret
    }
}

/// Fails every device read with `EIO`.
unsafe extern "C" fn failing_read(_dev: *mut device, _request: *mut device_request) -> isize {
    -(libc::EIO as isize)
}

/// Fails every device erase with `EIO` and deliberately does not run the
/// request's completion callback, which is how a backend that fails behaves
/// and what keeps the request owned by the caller.
unsafe extern "C" fn failing_erase(_dev: *mut device, _request: *mut device_request) -> i32 {
    -libc::EIO
}

/// Leaves the geometry the ramdisk's `open` installed alone, so
/// [`CoreFixture`] keeps the narrowed geometry and the store the ramdisk
/// already allocated while `page_ftl_open` runs.
unsafe extern "C" fn keep_geometry_open(
    _dev: *mut device,
    _name: *const c_char,
    _flags: c_int,
) -> c_int {
    0
}

/// Fails every device open with `EIO`, so a test can drive the FTL's open error
/// path. The ramdisk has no failing case of its own.
pub unsafe extern "C" fn failing_open(
    _dev: *mut device,
    _name: *const c_char,
    _flags: c_int,
) -> c_int {
    -libc::EIO
}

/// A ramdisk device plus an unopened [`PageFtl`], for the `page_core`
/// lifecycle tests.
///
/// [`Fixture`] builds the FTL's state by hand because the read, write and GC
/// paths take it as input. `page_ftl_open` builds that state itself, so this
/// fixture owns only the device and the zeroed structure, and the allocations
/// the FTL makes belong to the lifecycle the test drives.
pub struct CoreFixture {
    pub dev: *mut device,
    pub pgftl: Box<PageFtl>,
    /// The operations installed for the fixture's life: the ramdisk's, with
    /// [`keep_geometry_open`] in place of its `open`.
    ops: Box<device_operations>,
    /// The operations to restore on drop.
    saved_ops: *const device_operations,
}

impl CoreFixture {
    /// A ramdisk of `nr_segments` segments with `pages_per_segment` pages each,
    /// and a zeroed page FTL pointing at it.
    ///
    /// The ramdisk's `open` installs the module's default geometry and
    /// `page_ftl_open` calls it again, which would undo the narrowing below.
    /// The fixture therefore installs [`keep_geometry_open`] before the FTL is
    /// opened, so the narrowed geometry and the store the ramdisk already
    /// allocated survive.
    pub fn new(nr_segments: usize, pages_per_segment: usize) -> Self {
        assert!(nr_segments > 0 && pages_per_segment > 0);

        let mut dev: *mut device = ptr::null_mut();
        unsafe {
            assert_eq!(
                ffi_device_module_init(DeviceModule::Ramdisk as u64, &mut dev, 0),
                0
            );
            assert_eq!(
                ffi_ramdisk_open(dev, ptr::null(), libc::O_CREAT | libc::O_RDWR),
                0
            );
            (*dev).info.nr_bus = 1;
            (*dev).info.nr_chips = 1;
            (*dev).info.package.nr_blocks = nr_segments;
            (*dev).info.package.block.nr_pages = pages_per_segment;
        }

        let mut pgftl =
            Box::new(unsafe { core::mem::MaybeUninit::<PageFtl>::zeroed().assume_init() });
        pgftl.dev = dev;

        let saved_ops = unsafe { (*dev).d_op };
        let real = unsafe { &*saved_ops };
        let ops = Box::new(device_operations {
            open: Some(keep_geometry_open),
            write: real.write,
            read: real.read,
            erase: real.erase,
            close: real.close,
        });
        unsafe { (*dev).d_op = &*ops };

        CoreFixture {
            dev,
            pgftl,
            ops,
            saved_ops,
        }
    }

    /// Install `ops` as the device's operations, run `body`, then restore the
    /// fixture's own. `ops` must outlive the call.
    ///
    /// # Safety
    /// The body must not leave a request in flight, and it must leave the
    /// device in a state the fixture's teardown can release.
    pub unsafe fn with_ops<T>(
        &mut self,
        ops: &device_operations,
        body: impl FnOnce(&mut CoreFixture) -> T,
    ) -> T {
        let saved = (*self.dev).d_op;
        (*self.dev).d_op = ops;
        let ret = body(self);
        (*self.dev).d_op = saved;
        ret
    }

    /// Run `body` with the device's read operation failing, then restore.
    ///
    /// # Safety
    /// The body must not leave a request in flight.
    pub unsafe fn with_failing_read<T>(&mut self, body: impl FnOnce(&mut CoreFixture) -> T) -> T {
        let real = &*self.ops;
        let broken = device_operations {
            open: real.open,
            write: real.write,
            read: Some(failing_read),
            erase: real.erase,
            close: real.close,
        };
        self.with_ops(&broken, body)
    }
}

impl Drop for CoreFixture {
    fn drop(&mut self) {
        unsafe {
            // A test that opened and did not close leaves the FTL's state to
            // this teardown, which is also what releases the thread.
            if !self.pgftl.trans_map.is_null() || !self.pgftl.segments.is_null() {
                let _ = crate::ftl::page_core::close(&mut self.pgftl);
            }
            (*self.dev).d_op = self.saved_ops;
            assert_eq!(ffi_ramdisk_close(self.dev), 0);
            assert_eq!(ffi_device_module_exit(self.dev), 0);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            ffi_list_free(self.pgftl.gc_list);
            for segment in &mut self.segments {
                ffi_list_free(segment.lpn_list);
            }
            assert_eq!(
                libc::pthread_mutex_destroy(ptr::addr_of_mut!(self.pgftl.mutex)),
                0
            );
            assert_eq!(ffi_ramdisk_close(self.dev), 0);
            assert_eq!(ffi_device_module_exit(self.dev), 0);
        }
    }
}

/// Write `data.len()` bytes at absolute byte `sector` through
/// [`write_page`].
pub fn write_at(f: &mut Fixture, sector: usize, data: &mut [u8]) -> io::Result<usize> {
    let mut request = Request::new().expect("allocate a request");
    request.flag = DeviceDirection::Write.into();
    request.sector = sector;
    request.data_len = data.len();
    request.data = data.as_mut_ptr() as *mut core::ffi::c_void;
    request.end_rq = None;
    write_page(&mut f.pgftl, &mut request)
}

/// Read `buf.len()` bytes at absolute byte `sector` through the read path.
pub fn read_at(f: &mut Fixture, sector: usize, buf: &mut [u8]) -> io::Result<usize> {
    let mut request = Request::new().expect("allocate a request");
    request.flag = DeviceDirection::Read.into();
    request.sector = sector;
    request.data_len = buf.len();
    request.data = buf.as_mut_ptr() as *mut core::ffi::c_void;
    read_page(&mut f.pgftl, &mut request)
}

/// Read physical page `ppn` straight from the device.
pub fn read_physical_page(f: &mut Fixture, ppn: u32, buf: &mut [u8]) {
    let request = physical_request(DeviceDirection::Read, ppn, buf);
    unsafe {
        assert_eq!(
            ffi_ramdisk_read(f.dev, request.as_ptr()),
            DEVICE_PAGE_SIZE as isize
        );
    }
}

/// Program physical page `ppn` straight on the device, behind the FTL's back.
/// The ramdisk refuses to write a page twice, so this is how a test makes a
/// later FTL write to that page fail.
pub fn write_physical_page(f: &mut Fixture, ppn: u32, buf: &mut [u8]) {
    let request = physical_request(DeviceDirection::Write, ppn, buf);
    unsafe {
        assert_eq!(
            ffi_ramdisk_write(f.dev, request.as_ptr()),
            DEVICE_PAGE_SIZE as isize
        );
    }
}

fn physical_request(direction: DeviceDirection, ppn: u32, buf: &mut [u8]) -> Request {
    assert_eq!(buf.len(), DEVICE_PAGE_SIZE);
    let mut request = Request::new().expect("allocate a request");
    request.flag = direction.into();
    request.paddr = device_address::from(ppn);
    request.data_len = DEVICE_PAGE_SIZE;
    request.data = buf.as_mut_ptr() as *mut core::ffi::c_void;
    request.end_rq = None;
    request
}
