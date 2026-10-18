//! End to end integration test over the ramdisk backend, the Rust port of
//! `integration-test.c`.
//!
//! The C test brings the ramdisk and the page FTL up through
//! `ffi_module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE)`
//! (`integration-test.c:190`), opens the device (`:192`), runs two writers, two
//! readers, one overwriter and one forced erase thread over the same handle,
//! then closes and tears the module down (`:237-238`). This test drives the
//! same C ABI in the same order: [`ffi_module_init`], the `open` / `close` /
//! `ioctl` hooks through `flash->f_op` exactly as C reaches them
//! (`integration-test.c:162`, `:192`, `:237`), and [`ffi_flash_write`] /
//! [`ffi_flash_read`], the Rust dispatch pair over the same two hooks.
//!
//! The scenario and its verification are C's:
//!
//! - One block is `BLOCK_SIZE` bytes and the run covers `WRITE_SIZE` of them,
//!   the same 4 KiB and 128 MiB the C test uses. The readers' jitter alone is
//!   32768 waits of 1 to 1.5 ms, so a run takes a couple of minutes.
//! - Each writer stamps its offset into the first word of every block
//!   (`integration-test.c:94`, `:132`), so a reader can tell which block it
//!   read by the value it finds, and every block a reader ever sees is recorded
//!   in `IS_CHECK` (`:71`). After all six threads join (`:230-235`) every
//!   entry must be set, or the read of that offset failed (`:241-248`).
//! - A reader spins on the same offset when the device reports an error, a zero
//!   first word past offset 0 or the `-1` fill of an erased page (`:60-65`), so
//!   it never skips a block. The device side of an unmapped logical page is a
//!   zero fill, which is the case the writers unblock.
//! - The erase thread waits for the overwriter to start (`:157-158`) and then
//!   forces `NR_ERASE` collections through `PAGE_FTL_IOCTL_TRIM` (`:160-163`).
//!
//! Deliberate deviations from `integration-test.c`:
//!
//! - The wait jitter is deterministic. C seeds `srand()` from `time(NULL)`
//!   inside every loop (`integration-test.c:55`, `:91`, `:129`) and then draws
//!   `rand() % 500` for a 100, 1000 or 500 microsecond base (`:74`, `:107`,
//!   `:145`). The port gives each worker its own fixed xorshift stream seeded
//!   from a constant and draws the same ranges, so the sequence of interleavings
//!   it puts the FTL under is the same on every run while the workers still
//!   drift apart the way C's wall-clock seeds made them. Nothing outside those
//!   sleeps is random, and none of the verification depends on them.
//! - The reader bounds the block index it records against `NR_BLOCKS`, and the
//!   writers and readers report a mismatch instead of aborting on C's `assert`.
//!   C indexes `is_check` with whatever the device returned
//!   (`integration-test.c:71`), which is out of bounds for a stray value.
//! - The `IS_CHECK` sweep fails the test on a missing block. C prints every
//!   unread offset and returns 0 either way (`integration-test.c:240-252`), so
//!   the C integration test cannot fail its build on lost data.
//! - The ioctl's return value is ignored, as in C (`integration-test.c:162`).
//!   A collection that lost data would fail the `IS_CHECK` sweep instead, and
//!   `gc_from_list` reports one fewer segment than it erased, so its count is
//!   not a value to assert on.

use core::ffi::{c_char, c_int, c_uint};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use ftl_rust::device::DeviceModule;
use ftl_rust::flash::flash_device;
use ftl_rust::ftl::page_interface::{ffi_flash_read, ffi_flash_write};
use ftl_rust::ftl::PAGE_FTL_IOCTL_TRIM;
use ftl_rust::module::{ffi_module_exit, ffi_module_init, PAGE_FTL_MODULE};
use libc::{off_t, ssize_t};

/// C's `BLOCK_SIZE`, the transfer size of every request (`integration-test.c:37`).
const BLOCK_SIZE: usize = 4 * 1024;

/// C's `WRITE_SIZE`, the range the writers and readers sweep
/// (`integration-test.c:33`). It stays at 128 MiB, an eighth of the ramdisk's
/// 1 GiB store, so the run needs the garbage collector the same way the C test
/// does.
const WRITE_SIZE: usize = 128 * 1024 * 1024;

/// One entry per block, C's `is_check` (`integration-test.c:42`).
const NR_BLOCKS: usize = WRITE_SIZE / BLOCK_SIZE;

/// C's `NR_ERASE` (`integration-test.c:35`).
const NR_ERASE: usize = 10;

/// How long a reader may wait for one offset before the test fails. C spins
/// forever here, because a program that never finishes is its worst case, but a
/// lost block has to fail this suite rather than hang the CI job. The whole run
/// takes about two and a half minutes, and a healthy offset advances in
/// milliseconds, so this only fires on a real loss.
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// C's `DEVICE_PATH` (`integration-test.c:29`). The ramdisk ignores the name,
/// exactly as C's `ramdisk_open` does.
const DEVICE_PATH: &[u8] = b"/dev/nvme0n2\0";

/// The open flags of `integration-test.c:192`.
const OPEN_FLAGS: c_int = libc::O_CREAT | libc::O_RDWR;

/// One stream per worker, so the two readers and the two writers do not run in
/// lockstep with each other. C's wall-clock seeds did the de-correlating
/// (`integration-test.c:55`, `:91`, `:129`).
const SEED_WRITE_0: u64 = 0x5f37_5678_9abc_def1;
const SEED_WRITE_1: u64 = 0x5f37_5678_9abc_def2;
const SEED_READ_0: u64 = 0x5f37_5678_9abc_def3;
const SEED_READ_1: u64 = 0x5f37_5678_9abc_def4;
const SEED_OVERWRITE: u64 = 0x5f37_5678_9abc_def5;

/// C's `is_check`: the blocks a reader has seen with valid content
/// (`integration-test.c:71`, `:241-248`).
static IS_CHECK: [AtomicBool; NR_BLOCKS] = [const { AtomicBool::new(false) }; NR_BLOCKS];

/// C's `is_overwrite`, the handshake between the overwriter and the erase
/// thread (`integration-test.c:113`, `:124`, `:157`).
static IS_OVERWRITE: AtomicBool = AtomicBool::new(false);

/// The flash handle every worker shares, the Rust form of the `void *data` C
/// passes to `pthread_create`.
///
/// The page FTL hooks take `pgftl->mutex` for the whole transfer, so one handle
/// is callable from every thread, which is the contract C relies on when six
/// threads share one `struct flash_device *`.
#[derive(Clone, Copy)]
struct SharedFlash(*mut flash_device);

// Safety: every worker reaches the handle through the page FTL's five hooks,
// and each of those holds `pgftl->mutex` across the whole operation. No worker
// reads or writes the handle itself, and each spawn closure captures its own
// copy, so only `Send` is needed.
unsafe impl Send for SharedFlash {}

/// A fixed stream standing in for C's `srand(time(NULL) * offset)` + `rand()`
/// (`integration-test.c:55`, `:91`, `:129`). xorshift64 draws are cheap and
/// repeat run to run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// C's `usleep((rand() % span) + base)` (`integration-test.c:74`, `:107`,
    /// `:145`).
    fn wait(&mut self, base: u64, span: u64) -> Duration {
        Duration::from_micros(base + self.next() % span)
    }
}

/// C's `read_thread` (`integration-test.c:44-77`): sweep the range block by
/// block, spin on the current offset until it reads content back, and record
/// the block the content came from.
fn read_thread(flash: SharedFlash, seed: u64) {
    let flash = flash.0;
    let mut rng = Rng::new(seed);
    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut offset = 0usize;
    // C spins on the current offset forever waiting for a writer, which is
    // fine for a program whose only failure mode is not finishing. This test is
    // the CI gate, so a lost block has to fail instead of hanging: the deadline
    // is per offset and resets whenever the read advances.
    let mut attempt_started = std::time::Instant::now();

    while offset < WRITE_SIZE {
        buffer.fill(0);
        // Safety: `flash` is the live handle the module init returned and stays
        // opened until every thread has joined, and `buffer` is `BLOCK_SIZE`
        // writable bytes.
        let ret = unsafe {
            ffi_flash_read(
                flash,
                buffer.as_mut_ptr().cast(),
                BLOCK_SIZE,
                offset as off_t,
            )
        };
        let first = i64::from_ne_bytes(buffer[..8].try_into().expect("8 bytes"));

        // C retries the same offset on a device error and on a zero first word
        // past offset 0, which is how a reader waits for a writer
        // (`integration-test.c:60-62`).
        if ret < 0 || (offset > 0 && first == 0) {
            assert!(
                attempt_started.elapsed() < READ_DEADLINE,
                "no data for offset {offset} after {:?}",
                READ_DEADLINE
            );
            continue;
        }
        // The fill of a page that was never programmed (`integration-test.c:63-65`).
        if first == -1 {
            assert!(
                attempt_started.elapsed() < READ_DEADLINE,
                "offset {offset} still reads the erase fill after {:?}",
                READ_DEADLINE
            );
            continue;
        }
        assert_eq!(ret, BLOCK_SIZE as ssize_t, "short read (offset: {offset})");
        attempt_started = std::time::Instant::now();

        let block = usize::try_from(first).expect("the read returned a negative block index")
            / BLOCK_SIZE;
        assert!(
            block < NR_BLOCKS,
            "the read at offset {offset} returned block {block}, past the range"
        );
        IS_CHECK[block].store(true, Ordering::Relaxed);

        offset += BLOCK_SIZE;
        thread::sleep(rng.wait(1000, 500));
    }
}

/// C's `write_thread` (`integration-test.c:80-109`): stamp each block's offset
/// into its first word and write it.
fn write_thread(flash: SharedFlash, seed: u64) {
    let flash = flash.0;
    let mut rng = Rng::new(seed);
    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut offset = 0usize;

    while offset < WRITE_SIZE {
        buffer.fill(0);
        buffer[..8].copy_from_slice(&(offset as i64).to_ne_bytes());

        // Safety: `flash` is the live, opened handle, and `buffer` is
        // `BLOCK_SIZE` readable bytes.
        let ret = unsafe {
            ffi_flash_write(
                flash,
                buffer.as_mut_ptr().cast(),
                BLOCK_SIZE,
                offset as off_t,
            )
        };
        assert_eq!(ret, BLOCK_SIZE as ssize_t, "write failed (offset: {offset})");

        offset += BLOCK_SIZE;
        thread::sleep(rng.wait(100, 500));
    }
}

/// C's `overwrite_thread` (`integration-test.c:115-149`): hand the erase thread
/// its start signal, wait for it to arm, then rewrite every block.
fn overwrite_thread(flash: SharedFlash) {
    let flash = flash.0;
    let mut rng = Rng::new(SEED_OVERWRITE);
    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut offset = 0usize;

    IS_OVERWRITE.store(true, Ordering::SeqCst);
    thread::sleep(Duration::from_secs(2));

    while offset < WRITE_SIZE {
        buffer.fill(0);
        buffer[..8].copy_from_slice(&(offset as i64).to_ne_bytes());

        // Safety: same as [`write_thread`].
        let ret = unsafe {
            ffi_flash_write(
                flash,
                buffer.as_mut_ptr().cast(),
                BLOCK_SIZE,
                offset as off_t,
            )
        };
        assert_eq!(
            ret,
            BLOCK_SIZE as ssize_t,
            "overwrite failed (offset: {offset})"
        );

        offset += BLOCK_SIZE;
        thread::sleep(rng.wait(100, 500));
    }
}

/// C's `erase_thread` (`integration-test.c:151-172`): once the overwriter runs,
/// force `NR_ERASE` collections a second apart.
fn erase_thread(flash: SharedFlash) {
    let flash = flash.0;

    while !IS_OVERWRITE.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_micros(100));
    }

    // Safety: `flash` is the live, opened handle, and the ioctl the page FTL
    // installed takes the FTL lock itself.
    let ioctl = unsafe { (*(*flash).f_op).ioctl }.expect("the page FTL installs ioctl");

    for _ in 0..NR_ERASE {
        thread::sleep(Duration::from_millis(1000));
        // The return value is ignored, as in C (`integration-test.c:162`).
        let _ = unsafe { ioctl(flash, PAGE_FTL_IOCTL_TRIM as c_uint) };
    }
}

/// C's `main` (`integration-test.c:173-253`) over the ramdisk backend.
#[test]
fn integration_over_ramdisk() {
    for seen in IS_CHECK.iter() {
        seen.store(false, Ordering::Relaxed);
    }
    IS_OVERWRITE.store(false, Ordering::SeqCst);

    let mut flash: *mut flash_device = core::ptr::null_mut();
    // Safety: `flash` is a writable out parameter and `DeviceModule::Ramdisk`
    // is C's `RAMDISK_MODULE` (`integration-test.c:190`).
    let ret = unsafe { ffi_module_init(PAGE_FTL_MODULE, &mut flash, DeviceModule::Ramdisk as u64) };
    assert_eq!(ret, 0, "module init failed");
    assert!(!flash.is_null(), "module init left no handle");

    // Safety: `flash` is the handle the module init just returned.
    let open = unsafe { (*(*flash).f_op).open }.expect("the page FTL installs open");
    // Safety: `flash` carries the page FTL, and `DEVICE_PATH` is NUL terminated.
    let ret = unsafe { open(flash, DEVICE_PATH.as_ptr().cast::<c_char>(), OPEN_FLAGS) };
    assert_eq!(ret, 0, "open failed");

    let shared = SharedFlash(flash);
    let workers = [
        thread::spawn(move || write_thread(shared, SEED_WRITE_0)),
        thread::spawn(move || write_thread(shared, SEED_WRITE_1)),
        thread::spawn(move || read_thread(shared, SEED_READ_0)),
        thread::spawn(move || read_thread(shared, SEED_READ_1)),
        thread::spawn(move || overwrite_thread(shared)),
        thread::spawn(move || erase_thread(shared)),
    ];
    for worker in workers {
        worker.join().expect("a worker thread panicked");
    }

    // Safety: every worker has joined, so the handle is idle again.
    let close = unsafe { (*(*flash).f_op).close }.expect("the page FTL installs close");
    assert_eq!(unsafe { close(flash) }, 0, "close failed");
    // Safety: `flash` came from the module init and is now closed.
    assert_eq!(unsafe { ffi_module_exit(flash) }, 0, "module exit failed");

    // C's sweep over `is_check` (`integration-test.c:240-250`).
    for (block, seen) in IS_CHECK.iter().enumerate() {
        assert!(
            seen.load(Ordering::Relaxed),
            "read failed offset: {}",
            block * BLOCK_SIZE
        );
    }
    println!("all data exist => FINISH");
}
