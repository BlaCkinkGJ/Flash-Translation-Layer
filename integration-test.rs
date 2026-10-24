//! End to end test of the C ABI over the ramdisk backend. It is the Rust port
//! of `integration-test.c`.
//!
//! C brings the ramdisk and the page FTL up with
//! `ffi_module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE)`
//! (`integration-test.c:184-190`), then opens the device with
//! `O_CREAT | O_RDWR` (`:192`). It runs two writers, two readers, one
//! overwriter and one forced erase thread over that one handle (`:193-235`). It
//! closes the handle and tears the module down (`:237-238`), then reports on
//! the blocks the readers got back (`:240-252`). This program keeps that
//! scenario in that order.
//!
//! The point of a binary is the consumer side of the exported symbols.
//! `tests/integration.rs` drives the same entry points from inside the crate,
//! while `make integration-test` links this program against the crate the way a
//! C consumer does and runs it. The entry points are `ffi_module_init` and
//! `ffi_module_exit` for the module pair, and `ffi_flash_read` and
//! `ffi_flash_write` for the data path. The `flash->f_op` slots cover `open`,
//! `ioctl` and `close`, which have no `ffi_` wrapper. C spells the data path
//! `flash->f_op->read` (`:58`) and `flash->f_op->write` (`:95`) rather than the
//! wrapper pair. That is the one place this port calls a different symbol to
//! reach the same operation.
//!
//! Deliberate deviations from `integration-test.c`:
//!
//! - The compile time device selection is gone. C picks `ZONE_MODULE`,
//!   `BLUEDBM_MODULE` or `RASPBERRY_MODULE` out of `DEVICE_USE_*`
//!   (`integration-test.c:183-190`) and halves `WRITE_SIZE` for the raspberry.
//!   This program is the ramdisk one the `make integration-test` target and CI
//!   run, so it passes `DeviceModule::Ramdisk` alone. That is also the only
//!   backend an in-repo run can use.
//! - The `USE_DEBUG_PRINT` and `USE_FORCE_ERASE` blocks are gone with the
//!   switches. The debug prints were commented in, so they never printed. The
//!   erase step they guarded is kept.
//! - The pthread plumbing is `std::thread`. C reports a failed
//!   `pthread_create` with `perror` and `exit(errno)` (`:193-228`). `std`
//!   panics from inside `spawn` instead, which still ends the program non zero.
//! - The wait jitter is deterministic. C seeds `srand()` from `time(NULL)`
//!   inside every loop (`:55`, `:91`, `:129`) and then draws `rand() % 500` for
//!   a 100, 1000 or 500 microsecond base (`:74`, `:107`, `:145`). Each worker
//!   here owns a fixed stream seeded from a constant and draws the same ranges.
//!   The workers still drift apart the way C's wall clock seeds made them,
//!   without depending on the clock. Nothing outside those sleeps is random and
//!   no verification depends on them.
//! - A reader gives up after `READ_DEADLINE` on one offset. C spins forever
//!   (`:60-65`), which for a CI step means a job that hangs until it times out.
//!   A healthy offset advances in milliseconds, so the deadline only fires on
//!   real data loss. It then fails the target instead of stalling it.
//! - A block value outside the swept range is rejected. C indexes `is_check`
//!   with whatever the device returned (`:71`), which is out of bounds for a
//!   stray one.
//! - A missing block fails the program. C prints every unread offset and
//!   returns `0` either way (`:240-252`), so the CI step that runs
//!   `make integration-test` cannot fail on data the FTL lost. The report and
//!   the `all data exist => FINISH` success line stay as C wrote them.

use core::ffi::{c_char, c_int, c_uint};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ftl_rust::device::DeviceModule;
use ftl_rust::flash::flash_device;
use ftl_rust::ftl::page_interface::{ffi_flash_read, ffi_flash_write};
use ftl_rust::ftl::PAGE_FTL_IOCTL_TRIM;
use ftl_rust::module::{ffi_module_exit, ffi_module_init, PAGE_FTL_MODULE};
use libc::{off_t, ssize_t};

/// C's `BLOCK_SIZE`, the transfer size of every request
/// (`integration-test.c:37`).
const BLOCK_SIZE: usize = 4 * 1024;

/// C's `WRITE_SIZE`, the range the writers and readers sweep
/// (`integration-test.c:33`). It stays at 128 MiB, an eighth of the ramdisk's
/// 1 GiB store, so the run needs the garbage collector the same way the C
/// program does.
const WRITE_SIZE: usize = 128 * 1024 * 1024;

/// One entry per block, C's `is_check` (`integration-test.c:42`).
const NR_BLOCKS: usize = WRITE_SIZE / BLOCK_SIZE;

/// C's `NR_ERASE` (`integration-test.c:35`).
const NR_ERASE: usize = 10;

/// How long a reader may wait for one offset before the run fails. See the
/// deviations above.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// C's `DEVICE_PATH` (`integration-test.c:29`). The ramdisk ignores the name,
/// exactly as C's `ramdisk_open` does.
const DEVICE_PATH: &[u8] = b"/dev/nvme0n2\0";

/// The open flags of `integration-test.c:192`.
const OPEN_FLAGS: c_int = libc::O_CREAT | libc::O_RDWR;

/// One stream per worker, so the two readers and the two writers do not run in
/// lockstep with each other. C's wall clock seeds did the de-correlating
/// (`integration-test.c:55`, `:91`, `:129`).
const SEED_WRITE_0: u64 = 0x5f37_5678_9abc_def1;
const SEED_WRITE_1: u64 = 0x5f37_5678_9abc_def2;
const SEED_READ_0: u64 = 0x5f37_5678_9abc_def3;
const SEED_READ_1: u64 = 0x5f37_5678_9abc_def4;
const SEED_OVERWRITE: u64 = 0x5f37_5678_9abc_def5;

/// C's `is_check`: the blocks a reader has seen with valid content
/// (`integration-test.c:71`, `:240-248`).
static IS_CHECK: [AtomicBool; NR_BLOCKS] = [const { AtomicBool::new(false) }; NR_BLOCKS];

/// C's `is_overwrite`, the handshake between the overwriter and the erase
/// thread (`integration-test.c:113`, `:124`, `:157`).
static IS_OVERWRITE: AtomicBool = AtomicBool::new(false);

/// The flash handle every worker shares, the Rust form of the `void *data` C
/// hands to `pthread_create`.
///
/// The page FTL hooks take `pgftl->mutex` for the whole transfer. One handle is
/// therefore callable from every thread, which is the contract C relies on when
/// six threads share one `struct flash_device *`.
#[derive(Clone, Copy)]
struct SharedFlash(*mut flash_device);

// Safety: every worker reaches the handle through the page FTL's hooks, and
// each of those holds `pgftl->mutex` across the whole operation. No worker
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

/// Record the block a reader read back, C's `is_check[buffer[0] / BLOCK_SIZE]
/// = 1` (`integration-test.c:71`).
///
/// Returns `false` when the value names no block of the swept range, which C
/// would index out of bounds.
fn record_block(value: i64) -> bool {
    match usize::try_from(value) {
        Ok(value) if value / BLOCK_SIZE < NR_BLOCKS => {
            IS_CHECK[value / BLOCK_SIZE].store(true, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

/// C's sweep over `is_check` (`integration-test.c:240-248`): report every block
/// that never came back and say whether the run was complete.
fn report_missing_blocks() -> bool {
    let mut is_all_valid = true;

    for (block, seen) in IS_CHECK.iter().enumerate() {
        if !seen.load(Ordering::Relaxed) {
            println!("read failed offset: {:<20}", block * BLOCK_SIZE);
            is_all_valid = false;
        }
    }

    is_all_valid
}

/// C's `read_thread` (`integration-test.c:44-77`): sweep the range block by
/// block, spin on the current offset until it reads content back, and record
/// the block the content came from.
fn read_thread(flash: SharedFlash, seed: u64) {
    let flash = flash.0;
    let mut rng = Rng::new(seed);
    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut offset = 0usize;
    let mut attempt_started = Instant::now();

    while offset < WRITE_SIZE {
        buffer.fill(0);
        // Safety: `flash` is the live handle the module init returned and stays
        // open until every worker has joined, and `buffer` is `BLOCK_SIZE`
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

        // C retries the same offset on a device error, and on a zero first word
        // past offset 0. That retry is how a reader waits for a writer
        // (`integration-test.c:60-62`).
        if ret < 0 || (offset > 0 && first == 0) {
            assert!(
                attempt_started.elapsed() < READ_DEADLINE,
                "no data for offset {offset} after {READ_DEADLINE:?}"
            );
            continue;
        }
        // The fill of a page that was never programmed
        // (`integration-test.c:63-65`).
        if first == -1 {
            assert!(
                attempt_started.elapsed() < READ_DEADLINE,
                "offset {offset} still reads the erase fill after {READ_DEADLINE:?}"
            );
            continue;
        }
        assert_eq!(ret, BLOCK_SIZE as ssize_t, "short read (offset: {offset})");
        // The block came back, so the wait for this offset is over.
        attempt_started = Instant::now();

        assert!(
            record_block(first),
            "the read at offset {offset} returned {first}, which is no block"
        );

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
fn main() {
    let mut flash: *mut flash_device = core::ptr::null_mut();

    // Safety: `flash` is a writable out parameter and `DeviceModule::Ramdisk`
    // is C's `RAMDISK_MODULE` (`integration-test.c:190`).
    let ret = unsafe {
        ffi_module_init(PAGE_FTL_MODULE, &mut flash, DeviceModule::Ramdisk as u64)
    };
    assert_eq!(ret, 0, "module init failed");
    assert!(!flash.is_null(), "module init left no handle");

    // Safety: `flash` is the handle the module init just returned.
    let open = unsafe { (*(*flash).f_op).open }.expect("the page FTL installs open");
    // Safety: `flash` carries the page FTL and `DEVICE_PATH` is NUL terminated.
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

    if !report_missing_blocks() {
        // C exits `0` here as well, which is the one behaviour of the C program
        // a CI step cannot use. See the deviations at the top.
        std::process::exit(1);
    }
    println!("all data exist => FINISH");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The records the run cannot make on its own: a block value the sweep
    /// never produces, a wait outside the range C draws, and a lost block.
    #[test]
    fn record_block_accepts_the_blocks_of_the_swept_range() {
        assert!(record_block(0));
        assert!(record_block(((NR_BLOCKS - 1) * BLOCK_SIZE) as i64));
        assert!(IS_CHECK[0].load(Ordering::Relaxed));
        assert!(IS_CHECK[NR_BLOCKS - 1].load(Ordering::Relaxed));
    }

    #[test]
    fn record_block_rejects_a_value_outside_the_range() {
        assert!(!record_block(-1));
        assert!(!record_block((NR_BLOCKS * BLOCK_SIZE) as i64));
        assert!(!record_block(i64::MAX));
    }

    #[test]
    fn the_wait_stays_inside_the_range_c_draws() {
        let mut rng = Rng::new(SEED_READ_0);

        for _ in 0..1000 {
            let wait = rng.wait(1000, 500);
            assert!(wait >= Duration::from_micros(1000));
            assert!(wait < Duration::from_micros(1500));
        }
    }

    #[test]
    fn the_sweep_reports_the_first_block_that_never_came_back() {
        for seen in IS_CHECK.iter() {
            seen.store(true, Ordering::Relaxed);
        }
        assert!(report_missing_blocks());

        IS_CHECK[1].store(false, Ordering::Relaxed);
        assert!(!report_missing_blocks());

        IS_CHECK[1].store(true, Ordering::Relaxed);
    }
}
