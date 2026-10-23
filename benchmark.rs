//! Performance benchmark over the page FTL, the Rust port of `benchmark.c`.
//!
//! The program drives C's ABI in C's order. [`ffi_module_init`] brings the page
//! FTL up over the device module `-d` names, then `flash->f_op->open` opens it.
//! Every transfer goes through [`ffi_flash_write`] / [`ffi_flash_read`], the
//! dispatch pair over the same two hooks. Nothing here mirrors a C struct, and
//! the handle comes from `ftl_rust::flash`.
//!
//! What the port keeps, read off `benchmark.c` and checked against it:
//!
//! - The command line: `-m`, `-d`, `-t`, `-j`, `-b`, `-n`, `-p`, `-w` and
//!   `-h`, the usage text, every default, the `M` / `K` size suffix on `-b` and
//!   `-n`, and the exit status of each error path.
//! - The two phases: the warm-up that fills the store with its `fill data
//!   start!` / `ready to read!` lines, then the measured run.
//! - The thread model: one thread per job, the same work split (every job
//!   sweeps the whole sequence), the same per core pinning, the progress line,
//!   and the `finish thread N` lines in join order.
//! - The output: the `[parameters]` echo before the run and after the teardown,
//!   the `[job information]` table with the running maximum and minimum
//!   latency C keeps across the rows, and the `[crc status]` verdict.
//!
//! Deliberate deviations from `benchmark.c`:
//!
//! - The parsed options are a [`Parameters`] value and the run state is one
//!   [`Shared`]. C hands one `struct benchmark_parameter *` to every thread.
//!   Both are Rust types with no C layout, so a field costs no header change.
//!   The counters a job publishes are atomics instead of C's unsynchronised
//!   `size_t` writes.
//! - The latency samples of a job are a `Vec<u64>` the job returns. C prepends
//!   fake pointers to an intrusive list and frees it afterwards. The report
//!   reads the same three things off them: the count, the sum and the extremes.
//! - A failed C `assert` reports through [`check`] and aborts, so the exit
//!   stays SIGABRT where a Rust panic would exit 101. C's
//!   `assert(parm->crc32_list != NULL)` has no equivalent. `vec!` aborts on
//!   allocation failure, as `src/list.rs` documents.
//! - `argv` is read through `args_os` and `to_string_lossy`, so a byte that is
//!   not UTF-8 becomes U+FFFD instead of the raw byte C passes to `strcmp` and
//!   `isprint`.
//! - `-j 0` indexes the per job slots out of bounds whenever the warm-up runs,
//!   which is the default write workload and every read workload. The sweep
//!   that fills the store runs on this thread before any job starts, and it
//!   publishes into `wp[0]` of an empty table. C writes the same slot out of
//!   bounds (its `wp` comes from `malloc(0)`) and corrupts the heap. The port
//!   panics on its bounds check instead. Only a write workload with `-w` skips
//!   the warm-up, where neither program runs a job and both print the empty
//!   table.
//! - `-n 0` walks the same underflowed condition C walks. The progress loop
//!   compares against `nr_blocks - 1`, which wraps to `usize::MAX`, and the
//!   job index never reaches it. Both loops then print a progress line per
//!   second and never finish.

use core::ffi::{c_char, c_int};
use core::mem::size_of;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::io::{self, Write};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use ftl_rust::crc32::crc32;
use ftl_rust::device::DeviceModule;
use ftl_rust::flash::flash_device;
use ftl_rust::ftl::page_interface::{ffi_flash_read, ffi_flash_write};
use ftl_rust::module::{ffi_module_exit, ffi_module_init, PAGE_FTL_MODULE};
use libc::{off_t, ssize_t};

/// `printf` to a stream, which C calls for its side effect and never checks.
macro_rules! print_out {
    ($out:expr, $($arg:tt)*) => {
        let _ = writeln!($out, $($arg)*);
    };
}

// glibc's `pthread_setaffinity_np`, which the `libc` crate does not declare for
// `gnu` targets. glibc has exported it since 2.3.4, and it is what C's
// `USE_PER_CORE` block calls. A plain comment, because rustdoc has no place to
// put a doc comment on an extern block.
#[cfg(target_os = "linux")]
extern "C" {
    fn pthread_setaffinity_np(
        thread: libc::pthread_t,
        cpuset_size: libc::size_t,
        cpuset: *const libc::cpu_set_t,
    ) -> libc::c_int;
}

/// C's `PAGE_SIZE`: the default block size, and the size of the `-p` buffer.
const PAGE_SIZE: usize = 0x1 << 12;

/// C's `SEC_TO_NS`.
const SEC_TO_NS: u64 = 1_000_000_000;

/// C's `NS_PER_MS`.
const NS_PER_MS: u64 = 1_000_000;

/// C's `DEVICE_PATH_SIZE`.
const DEVICE_PATH_SIZE: usize = PAGE_SIZE;

/// C's `CRC32_INIT` (`include/crc32.h`), the seed of every block checksum.
const CRC32_INIT: u32 = 0xFFFF_FFFF;

/// C's `INT32_MAX`: the initial minimum latency, and the initial position the
/// progress scan compares every job against.
const INT32_MAX: usize = i32::MAX as usize;

/// C's `0x1 << 20`, the MiB the bandwidth columns and the io size line divide
/// by.
const MIB: usize = 0x1 << 20;

/// C's `module_str` and `module_list` as one table, in C's order: the `-m` name
/// and the module number [`ffi_module_init`] takes.
const MODULES: [(&str, c_int); 1] = [("pgftl", PAGE_FTL_MODULE)];

/// C's `device_str` and `device_list` as one table, in C's order: the `-d` name
/// and the device module number, which is C's `RAMDISK_MODULE` numbering.
const DEVICES: [(&str, DeviceModule); 4] = [
    ("ramdisk", DeviceModule::Ramdisk),
    ("bluedbm", DeviceModule::Bluedbm),
    ("zone", DeviceModule::Zone),
    ("raspberry", DeviceModule::Raspberry),
];

/// C's `workload_str`, in the order of [`Workload`].
const WORKLOAD_STR: [&str; 4] = ["write", "read", "randwrite", "randread"];

/// C's `enum { WRITE = 0, READ, RAND_WRITE, RAND_READ }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Workload {
    Write,
    Read,
    RandWrite,
    RandRead,
}

impl Workload {
    /// C's `get_index_from_list(workload_str)`, as a value.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "write" => Some(Self::Write),
            "read" => Some(Self::Read),
            "randwrite" => Some(Self::RandWrite),
            "randread" => Some(Self::RandRead),
            _ => None,
        }
    }

    /// The `workload_str` entry of this workload, C's
    /// `workload_str[parm->workload_idx]`.
    fn name(self) -> &'static str {
        match self {
            Self::Write => WORKLOAD_STR[0],
            Self::Read => WORKLOAD_STR[1],
            Self::RandWrite => WORKLOAD_STR[2],
            Self::RandRead => WORKLOAD_STR[3],
        }
    }

    /// C's `workload_idx == RAND_WRITE || workload_idx == WRITE`.
    fn is_write(self) -> bool {
        matches!(self, Self::Write | Self::RandWrite)
    }

    /// C's `workload_idx == RAND_READ || workload_idx == READ`.
    fn is_read(self) -> bool {
        matches!(self, Self::Read | Self::RandRead)
    }

    /// C's `workload_idx == RAND_WRITE || workload_idx == RAND_READ`.
    fn is_random(self) -> bool {
        matches!(self, Self::RandWrite | Self::RandRead)
    }
}

/// C's `struct benchmark_parameter`, without the run state that lives in
/// [`Shared`]. Every field is one `init_parameters` assigns.
struct Parameters {
    module_idx: usize,
    device_idx: usize,
    nr_jobs: i32,
    workload: Workload,
    block_sz: usize,
    nr_blocks: usize,
    device_path: String,
    do_warm_up: bool,
}

/// What C's `init_parameters` decides, as a value, so the parser can be tested
/// without exiting the test process.
enum Outcome {
    /// The parameters a run needs.
    Run(Parameters),
    /// `-h`: C prints the usage on stderr and exits 0.
    Help,
    /// A bad flag or a bad name: C prints the reason, then the usage, and
    /// exits 1.
    Failed(String),
}

/// The option string C hands to `getopt`, character for character.
const OPTSTRING: &[u8] = b"m:d:t:j:b:n:p:hw";

/// What [`OPTSTRING`] says about an option letter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Argument {
    /// The letter is followed by `:` and takes the next element as its value.
    Required,
    /// The letter stands alone.
    Free,
}

/// The option string's verdict on `flag`, or `None` for a byte it does not
/// name.
///
/// `getopt` reads `:` as the argument marker rather than as an option letter,
/// so it is not in the option set.
fn option(flag: u8) -> Option<Argument> {
    if flag == b':' {
        return None;
    }
    let mut index = 0;
    while index < OPTSTRING.len() {
        if OPTSTRING[index] == flag {
            return Some(if OPTSTRING.get(index + 1) == Some(&b':') {
                Argument::Required
            } else {
                Argument::Free
            });
        }
        index += 1;
    }
    None
}

/// Which failure C's `getopt` `?` reports.
#[derive(Clone, Copy)]
enum OptError {
    /// The byte is not one of the option letters.
    Unknown,
    /// The letter takes an argument and none is left.
    Missing,
}

/// One step of C's `getopt`, over [`OPTSTRING`].
enum Opt {
    /// An option that takes an argument, with the argument. `getopt` reads the
    /// rest of the element when the letter is not at its end, and the next
    /// element otherwise.
    Value(u8, String),
    /// An option that takes no argument.
    Flag(u8),
    /// C's `?` with `optopt` set.
    Error(u8, OptError),
}

/// The scanning half of C's `getopt`.
///
/// glibc permutes operands out of the way, so an operand is skipped and the
/// options after it are still read. A `--` ends the options and a lone `-` is
/// an operand. `next` walks bytes, as `getopt` does, so an element that is not
/// UTF-8 is reported byte by byte like C reports it.
struct Getopt<'a> {
    args: &'a [String],
    /// The element the scan is at.
    index: usize,
    /// The byte offset inside that element, while a cluster is being read.
    offset: usize,
}

impl<'a> Getopt<'a> {
    fn new(args: &'a [String]) -> Self {
        Self {
            args,
            index: 0,
            offset: 0,
        }
    }

    /// The next option, or `None` once the options run out.
    fn next(&mut self) -> Option<Opt> {
        loop {
            if self.index >= self.args.len() {
                return None;
            }
            let element = self.args[self.index].as_str();
            if self.offset == 0 {
                // C's `--`, and its operands, which are not options.
                if element == "--" {
                    self.index = self.args.len();
                    return None;
                }
                if !element.starts_with('-') || element.len() == 1 {
                    self.index += 1;
                    continue;
                }
                self.offset = 1;
            }

            // The element starts with `-`, so the byte at the offset begins a
            // character, and a match on it is a character boundary.
            let flag = element.as_bytes()[self.offset];
            match option(flag) {
                None => {
                    self.index += 1;
                    self.offset = 0;
                    return Some(Opt::Error(flag, OptError::Unknown));
                }
                Some(Argument::Free) => {
                    self.offset += 1;
                    if self.offset >= element.len() {
                        self.index += 1;
                        self.offset = 0;
                    }
                    return Some(Opt::Flag(flag));
                }
                Some(Argument::Required) => {
                    let attached = &element[self.offset + 1..];
                    self.offset = 0;
                    self.index += 1;
                    if !attached.is_empty() {
                        return Some(Opt::Value(flag, attached.to_string()));
                    }
                    // `getopt` takes the next element as the argument even
                    // when it looks like an option.
                    if self.index >= self.args.len() {
                        return Some(Opt::Error(flag, OptError::Missing));
                    }
                    let argument = self.args[self.index].clone();
                    self.index += 1;
                    return Some(Opt::Value(flag, argument));
                }
            }
        }
    }
}

/// C's `init_parameters` with the `getopt` loop inside it.
///
/// `argv[0]` is the program name the usage line and glibc's `getopt` diagnostic
/// print, the rest are options. C's help prints the counts the parser has
/// filled in so far, so the usage text it emits carries the zero state rather
/// than the defaults a run uses. That is C's behaviour (`parm` is assigned only
/// after the option loop) and it is kept as is.
fn parse_arguments(argv: &[String]) -> Outcome {
    let argv0 = argv.first().map(String::as_str).unwrap_or("benchmark");
    // C's locals, with C's initial values, before its option loop.
    let mut module_idx = 0;
    let mut device_idx = 0;
    // Safety: `sysconf` takes a name from its own list.
    let mut nr_jobs = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } as i32;
    let mut workload = Workload::Write;
    let mut block_sz = PAGE_SIZE;
    let mut nr_blocks = 1;
    let mut do_warm_up = true;
    let mut device_path = String::new();

    let mut options = Getopt::new(argv.get(1..).unwrap_or(&[]));
    while let Some(option) = options.next() {
        match option {
            Opt::Flag(b'w') => do_warm_up = false,
            Opt::Flag(b'h') => return Outcome::Help,
            Opt::Value(b'm', name) => match MODULES.iter().position(|(n, _)| *n == name) {
                Some(index) => module_idx = index,
                None => return Outcome::Failed(unexpected_argument(&name)),
            },
            Opt::Value(b'd', name) => match DEVICES.iter().position(|(n, _)| *n == name) {
                Some(index) => device_idx = index,
                None => return Outcome::Failed(unexpected_argument(&name)),
            },
            Opt::Value(b't', name) => match Workload::from_name(&name) {
                Some(parsed) => workload = parsed,
                None => return Outcome::Failed(unexpected_argument(&name)),
            },
            Opt::Value(b'j', value) => nr_jobs = c_atoi(&value),
            Opt::Value(b'b', value) => {
                block_sz = c_atoi(&value) as usize * size_suffix(&value)
            }
            Opt::Value(b'n', value) => {
                nr_blocks = c_atoi(&value) as usize * size_suffix(&value)
            }
            Opt::Value(b'p', value) => device_path = truncated_path(&value),
            Opt::Error(flag, kind) => {
                return Outcome::Failed(flag_failure(argv0, flag, kind))
            }
            // Every option letter is either argument free and handled above or
            // argument taking and handled above, which is C's `default:
            // abort();`.
            Opt::Value(flag, _) | Opt::Flag(flag) => {
                unreachable!("option -{} is not handled", flag as char)
            }
        }
    }

    Outcome::Run(Parameters {
        module_idx,
        device_idx,
        nr_jobs,
        workload,
        block_sz,
        nr_blocks,
        device_path,
        do_warm_up,
    })
}

/// C's `print_list`: the names of a [`MODULES`] or [`DEVICES`] style table,
/// separated by `, `.
fn names<K>(table: &[(&str, K)]) -> String {
    table
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// C's `help_message`, which C calls `-h` and every error path print.
///
/// The counts it echoes are the ones the parser has filled in, which at that
/// point is nothing. C assigns `parm->nr_jobs`, `parm->nr_blocks` and
/// `parm->device_path` only after the option loop, so the usage text always
/// reports `0`, `0` and `(null)`.
fn print_usage(argv0: &str, out: &mut impl Write) {
    print_out!(
        out,
        "{argv0} -m <module name> -d <device name> -t <workload> -j <# of jobs> -b <block size(bytes)> -n <# of blocks> -p <device path> [-w]"
    );
    print_out!(out, "\t- modules     [{}]", names(&MODULES));
    print_out!(out, "\t- devices     [{}]", names(&DEVICES));
    print_out!(out, "\t- workloads   [{}]", WORKLOAD_STR.join(", "));
    print_out!(out, "\t- jobs        (default: {})", 0);
    print_out!(out, "\t- block size  (default: {})", PAGE_SIZE);
    print_out!(out, "\t- # of block  (default: {})", 0);
    print_out!(out, "\t- path        (default: (null))");
    print_out!(
        out,
        "\t- w           disable warm-up phase (default: enabled)"
    );
}

/// C's `error: unexpected argument detected (%s)`, for a `-m`, `-d` or `-t`
/// name that is not in the list.
fn unexpected_argument(value: &str) -> String {
    format!("error: unexpected argument detected ({value})")
}

/// C's `processing_parameters_error`, the message for its `?` arm. An option
/// letter that takes an argument reports the missing argument, anything else is
/// an unknown option.
fn processing_parameters_error(flag: u8) -> String {
    match flag {
        b'm' | b'd' | b't' | b'j' | b'n' | b'b' | b'p' => {
            format!("option -{} requires an arguments", flag as char)
        }
        // C prints a printable character as itself and anything else as the
        // unsigned value of the sign extended `char` it passes to `%x`.
        _ if (0x20..0x7f).contains(&flag) => {
            format!("unknown option character '-{}'", flag as char)
        }
        _ => format!(
            "unknown option character '0x{:x}'",
            flag as i8 as i32 as u32
        ),
    }
}

/// glibc's own `getopt` message, which C prints because it never clears
/// `opterr`. C's `processing_parameters_error` follows it, and the usage
/// follows both.
///
/// C's `%c` writes the byte it was given, which is what a `char` cast back to a
/// byte does here. For a byte above `0x7f` C's `(char)optopt` is implementation
/// defined, and the sign extending read that `%x` then prints is the x86-64
/// one, which is the target the `build-and-test-x86_64` job builds.
fn flag_failure(argv0: &str, flag: u8, kind: OptError) -> String {
    let text = match kind {
        OptError::Missing => "option requires an argument",
        OptError::Unknown => "invalid option",
    };
    format!(
        "{argv0}: {text} -- '{}'\n{}",
        flag as char,
        processing_parameters_error(flag)
    )
}

/// C's `atoi`: leading spaces, an optional sign, digits, and a stop at the
/// first byte that is none of those. An empty result is zero.
///
/// C's overflow is undefined. This truncates to `c_int` the way the implicit
/// `int` return would on a two's complement target.
fn c_atoi(text: &str) -> i32 {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let negative = bytes.get(index) == Some(&b'-');
    if negative || bytes.get(index) == Some(&b'+') {
        index += 1;
    }
    let mut value: i64 = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        value = value
            .wrapping_mul(10)
            .wrapping_add(i64::from(bytes[index] - b'0'));
        index += 1;
    }
    if negative {
        value = -value;
    }
    value as i32
}

/// C's `get_size_from_character`: the first `M` or `K` in the text picks the
/// multiplier, and a text with neither means one.
fn size_suffix(text: &str) -> usize {
    for byte in text.bytes() {
        match byte {
            b'M' | b'm' => return 1 << 20,
            b'K' | b'k' => return 1 << 10,
            _ => {}
        }
    }
    1
}

/// C's `strncpy(dst, optarg, DEVICE_PATH_SIZE - 1)`: the path is cut to the
/// buffer size. C's byte copy can cut a character in half. This one backs up to
/// the character boundary before it.
fn truncated_path(path: &str) -> String {
    let mut end = path.len().min(DEVICE_PATH_SIZE - 1);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    path[..end].to_string()
}

/// C's `xorshift64_next`.
fn xorshift64_next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// C's `get_random_seed`: the kernel entropy source when the build has it, the
/// monotonic clock and the thread id otherwise. The `legacy-random` feature is
/// C's `USE_LEGACY_RANDOM`, which drops the entropy source.
fn get_random_seed() -> u64 {
    #[cfg(not(feature = "legacy-random"))]
    {
        let mut seed: u64 = 0;
        // Safety: `seed` is eight writable bytes, which is inside the 256 byte
        // limit `getentropy` accepts.
        let rc = unsafe {
            libc::getentropy(
                &mut seed as *mut u64 as *mut core::ffi::c_void,
                size_of::<u64>(),
            )
        };
        if rc == 0 && seed != 0 {
            return seed;
        }
    }

    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // Safety: `now` is writable and `CLOCK_MONOTONIC` is a clock this process
    // can read.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    // Safety: `pthread_self` takes no argument and returns the calling thread.
    let thread = unsafe { libc::pthread_self() } as u64;
    let mut seed = (now.tv_sec as u64)
        .wrapping_mul(SEC_TO_NS)
        ^ (now.tv_nsec as u64)
        ^ thread;
    if seed == 0 {
        seed = 1;
    }
    seed
}

/// C's `shuffling`: one draw per block picks the swap partner. A draw is not a
/// permutation algorithm. It is C's two index shuffle, which leaves the
/// sequence in a different order than Fisher-Yates would.
fn shuffling(sequence: &mut [off_t]) {
    let mut state = get_random_seed();
    let nr_blocks = sequence.len();
    for index in 0..nr_blocks {
        let swap_pos = (xorshift64_next(&mut state) as usize) % nr_blocks;
        sequence.swap(index, swap_pos);
    }
}

/// C's `fill_buffer_random`: the payload of a block, drawn from the caller's
/// stream. C keeps the stream in a thread local. A job owns one here.
fn fill_buffer_random(buffer: &mut [u8], seed: &mut u64) {
    if *seed == 0 {
        *seed = get_random_seed();
    }
    let mut pos = 0;
    while pos + size_of::<u64>() <= buffer.len() {
        let word = xorshift64_next(seed);
        buffer[pos..pos + size_of::<u64>()].copy_from_slice(&word.to_ne_bytes());
        pos += size_of::<u64>();
    }
    if pos < buffer.len() {
        let mut word = xorshift64_next(seed);
        while pos < buffer.len() {
            buffer[pos] = (word & 0xFF) as u8;
            word >>= 8;
            pos += 1;
        }
    }
}

/// C's `(double)nanoseconds / (NS_PER_MS * 1000L)`, the seconds the report and
/// the progress line divide by. The conversion lives here so both call sites
/// carry the same unit.
fn seconds(nanoseconds: u64) -> f64 {
    nanoseconds as f64 / (NS_PER_MS * 1000) as f64
}

/// `printf("%.*f")` for the shapes this program prints.
///
/// C's glibc spells a NaN `nan` and Rust's `Display` spells it `NaN`, so the
/// two spellings are pinned here.
fn fmt_f64(value: f64, precision: usize) -> String {
    if value.is_nan() {
        "nan".to_string()
    } else {
        format!("{value:.precision$}")
    }
}

/// C's `assert`: report the check and abort, which is the SIGABRT a failed C
/// assertion gives. A Rust panic would exit 101 instead.
fn check(condition: bool, message: &str) {
    if !condition {
        eprintln!("benchmark: assertion failed: {message}");
        std::process::abort();
    }
}

/// The state every job shares, the Rust form of the one
/// `struct benchmark_parameter *` C hands to each of them.
struct Shared {
    flash: *mut flash_device,
    block_sz: usize,
    nr_blocks: usize,
    /// C's `offset_sequence`: the offsets in visit order, shuffled in place for
    /// the random workloads.
    sequence: Vec<off_t>,
    /// C's `crc32_list`: the checksum of every block the warm-up wrote.
    crc32_list: Vec<AtomicU32>,
    /// C's `crc32_is_match`: cleared by a read that returned other bytes.
    crc32_is_match: Vec<AtomicBool>,
    /// C's `wp`: the block each job is at.
    wp: Vec<AtomicUsize>,
    /// C's `total_time`: the nanoseconds each job has spent in transfers.
    total_time: Vec<AtomicU64>,
    /// C's `thread_id_allocator`.
    next_thread_id: AtomicI32,
}

// Safety: every job reaches the flash handle through the page FTL's hooks, each
// of which holds the FTL lock across the whole transfer. One handle is callable
// from every thread, which is the contract `integration.rs` records for the
// same handle. A job writes only its own `wp` and `total_time` slot and its own
// latency vector, and the counters are atomics.
unsafe impl Sync for Shared {}

impl Shared {
    /// C's `__atomic_fetch_add(&parm->thread_id_allocator, 1, ...)`.
    fn alloc_thread_id(&self) -> usize {
        self.next_thread_id.fetch_add(1, Ordering::SeqCst) as usize
    }
}

/// Pin the calling job to `thread_id % cores`, C's `USE_PER_CORE` block, so two
/// jobs do not share a core.
#[cfg(target_os = "linux")]
fn pin_to_core(thread_id: usize) {
    // Safety: `sysconf` takes a name from its own list.
    let cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if cores <= 0 {
        return;
    }
    let mut set: libc::cpu_set_t = unsafe { core::mem::zeroed() };
    // Safety: `set` is a `cpu_set_t` of the size the call expects, and the core
    // index is below the core count `sysconf` reported.
    let rc = unsafe {
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(thread_id % cores as usize, &mut set);
        pthread_setaffinity_np(
            libc::pthread_self(),
            size_of::<libc::cpu_set_t>(),
            &set,
        )
    };
    if rc != 0 {
        eprintln!("Warning: pthread_setaffinity_np failed for thread {thread_id}");
    }
}

/// C's `USE_PER_CORE` block is behind `#ifndef __APPLE__`, where the call does
/// not exist.
#[cfg(not(target_os = "linux"))]
fn pin_to_core(_thread_id: usize) {}

/// C's `write_data` and `read_data`: sweep the whole sequence once, time every
/// transfer, and return the latencies in visit order.
fn sweep(shared: &Shared, workload: Workload, thread_id: usize) -> Vec<u64> {
    pin_to_core(thread_id);

    let mut buffer = vec![0u8; shared.block_sz];
    let mut latencies = Vec::with_capacity(shared.nr_blocks);
    let mut seed = 0u64;

    for index in 0..shared.nr_blocks {
        let offset = shared.sequence[index];
        // The buffer is filled outside the timed window on both paths, as in C.
        if workload.is_write() {
            fill_buffer_random(&mut buffer, &mut seed);
            let sum =
                crc32(buffer.as_ptr().cast(), shared.block_sz, CRC32_INIT);
            shared.crc32_list[offset as usize / shared.block_sz]
                .store(sum, Ordering::SeqCst);
        } else {
            buffer.fill(0);
        }

        let start = Instant::now();
        // Safety: `flash` is the live, opened handle, and `buffer` is
        // `block_sz` bytes.
        let ret = unsafe {
            if workload.is_write() {
                ffi_flash_write(
                    shared.flash,
                    buffer.as_mut_ptr().cast(),
                    shared.block_sz,
                    offset,
                )
            } else {
                ffi_flash_read(
                    shared.flash,
                    buffer.as_mut_ptr().cast(),
                    shared.block_sz,
                    offset,
                )
            }
        };
        let interval = start.elapsed().as_nanos() as u64;
        check(
            ret == shared.block_sz as ssize_t,
            "the transfer returned a short count",
        );

        shared.total_time[thread_id].fetch_add(interval, Ordering::SeqCst);
        latencies.push(interval);
        shared.wp[thread_id].store(index, Ordering::SeqCst);

        if workload.is_read() {
            let sum =
                crc32(buffer.as_ptr().cast(), shared.block_sz, CRC32_INIT);
            let block = offset as usize / shared.block_sz;
            if sum != shared.crc32_list[block].load(Ordering::SeqCst) {
                shared.crc32_is_match[block].store(false, Ordering::SeqCst);
            }
        }
    }

    latencies
}

/// C's `print_parameters`, the echo before the run and after the teardown.
fn print_parameters(parm: &Parameters, out: &mut impl Write) {
    print_out!(out, "[parameters]");
    print_out!(out, "\t- modules     {}", MODULES[parm.module_idx].0);
    print_out!(out, "\t- devices     {}", DEVICES[parm.device_idx].0);
    print_out!(out, "\t- workloads   {}", parm.workload.name());
    print_out!(out, "\t- jobs        {}", parm.nr_jobs);
    print_out!(out, "\t- block size  {}", parm.block_sz);
    print_out!(out, "\t- # of block  {}", parm.nr_blocks);
    print_out!(
        out,
        "\t- io size     {}MiB",
        parm.nr_blocks.wrapping_mul(parm.block_sz) / MIB
    );
    // C hands a NULL to `%s` for an unset path, which glibc prints as `(null)`.
    print_out!(
        out,
        "\t- path        {}",
        if parm.device_path.is_empty() {
            "(null)"
        } else {
            parm.device_path.as_str()
        }
    );
    print_out!(
        out,
        "\t- warm-up     {}",
        if parm.do_warm_up { "enabled" } else { "disabled" }
    );
}

/// C's `report_result`: the per job table, then the crc verdict.
///
/// C keeps one running maximum and one running minimum across every row, so
/// row N reports the extremes of rows 0 through N rather than its own. Kept as
/// is, the README's sample output shows the same growth.
fn report_result(
    parm: &Parameters,
    latencies: &[Vec<u64>],
    crc32_is_match: &[AtomicBool],
    out: &mut impl Write,
) {
    let write_size = parm.block_sz.wrapping_mul(parm.nr_blocks);
    let mut max_latency = 0usize;
    let mut min_latency = INT32_MAX;

    print_out!(out, "[job information]");
    print_out!(
        out,
        "{:<4}{:<10}{:<10}{:<10}{:<10}{:<10}{:<10}",
        "id",
        "time(s)",
        "bw(MiB/s)",
        "iops",
        "avg(ms)",
        "max(ms)",
        "min(ms)"
    );
    print_out!(out, "=====");

    for (index, samples) in latencies.iter().enumerate() {
        let mut total_time = 0u64;
        for interval in samples {
            total_time = total_time.wrapping_add(*interval);
            max_latency = max_latency.max(*interval as usize);
            min_latency = min_latency.min(*interval as usize);
        }
        let iops = samples.len();
        let seconds = seconds(total_time);
        print_out!(
            out,
            "{:<4}{:<10}{:<10}{:<10}{:<10}{:<10}{:<10}",
            index,
            fmt_f64(seconds, 4),
            fmt_f64(write_size as f64 / (seconds * MIB as f64), 4),
            iops,
            fmt_f64(total_time as f64 / NS_PER_MS as f64 / iops as f64, 4),
            fmt_f64(max_latency as f64 / NS_PER_MS as f64, 4),
            fmt_f64(min_latency as f64 / NS_PER_MS as f64, 4)
        );
    }

    print_out!(out, "[crc status]");
    // C checks the blocks only for a read workload, so a write run always
    // reports success.
    let mut is_valid = true;
    if parm.workload.is_read() {
        for flag in crc32_is_match {
            if !flag.load(Ordering::SeqCst) {
                is_valid = false;
                break;
            }
        }
    }
    print_out!(
        out,
        "{}",
        if is_valid {
            "crc check success"
        } else {
            "crc check failed"
        }
    );
}

/// C's `main`, from the module init down to the second parameter echo.
fn run(parm: Parameters) {
    let mut flash: *mut flash_device = ptr::null_mut();
    let module = MODULES[parm.module_idx].1;
    let device = DEVICES[parm.device_idx].1;
    // Safety: `flash` is a writable out parameter, the module number is a table
    // index, and the device number is the flag word C passes.
    let rc = unsafe { ffi_module_init(module, &mut flash, device as u64) };
    check(rc == 0, "module init failed");
    check(!flash.is_null(), "module init left no handle");

    let mut path = parm.device_path.clone().into_bytes();
    path.push(0);
    // Safety: `flash` is the handle the module init returned.
    let open = unsafe { (*(*flash).f_op).open }.expect("the FTL installs open");
    // Safety: `flash` carries the page FTL and `path` is NUL terminated.
    let rc = unsafe {
        open(flash, path.as_ptr().cast::<c_char>(), libc::O_CREAT | libc::O_RDWR)
    };
    check(rc == 0, "open failed");

    let jobs = parm.nr_jobs as usize;
    let mut shared = Shared {
        flash,
        block_sz: parm.block_sz,
        nr_blocks: parm.nr_blocks,
        sequence: (0..parm.nr_blocks)
            .map(|index| index.wrapping_mul(parm.block_sz) as off_t)
            .collect(),
        crc32_list: (0..parm.nr_blocks).map(|_| AtomicU32::new(0)).collect(),
        crc32_is_match: (0..parm.nr_blocks)
            .map(|_| AtomicBool::new(true))
            .collect(),
        wp: (0..jobs).map(|_| AtomicUsize::new(0)).collect(),
        total_time: (0..jobs).map(|_| AtomicU64::new(0)).collect(),
        next_thread_id: AtomicI32::new(0),
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();

    print_parameters(&parm, &mut out);

    // The warm-up is the write sweep on this thread, which C calls for a read
    // workload whatever `-w` says, because a read needs the data it wrote.
    if parm.do_warm_up || parm.workload.is_read() {
        print_out!(out, "fill data start!");
        let thread_id = shared.alloc_thread_id();
        let latencies = sweep(&shared, Workload::Write, thread_id);
        // C frees the warm-up's latency list and rewinds the job it belonged
        // to, so the run and the report start over from block zero.
        drop(latencies);
        shared.wp[thread_id].store(0, Ordering::SeqCst);
        print_out!(out, "ready to read!");
    }

    if parm.workload.is_random() {
        shuffling(&mut shared.sequence);
    }

    shared.next_thread_id.store(0, Ordering::SeqCst);

    let latencies = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            handles.push(scope.spawn(|| {
                let thread_id = shared.alloc_thread_id();
                sweep(&shared, parm.workload, thread_id)
            }));
        }

        // C's progress loop: one line per second until the slowest job reaches
        // the last block. Its scan takes the last job holding the smallest
        // position, which is the one whose total_time the line reports.
        loop {
            let mut wp = INT32_MAX;
            let mut total_time = 0;
            for (position, elapsed) in shared.wp.iter().zip(&shared.total_time) {
                let candidate = position.load(Ordering::SeqCst);
                if wp < candidate {
                    continue;
                }
                wp = candidate;
                total_time = elapsed.load(Ordering::SeqCst);
            }
            let seconds = seconds(total_time);
            let percent = wp as f64 / parm.nr_blocks.wrapping_sub(1) as f64 * 100.0;
            let bandwidth =
                wp as f64 * parm.block_sz as f64 / (seconds * MIB as f64);
            let _ = write!(
                out,
                "\rProcessing: {}% [{} MiB/s]",
                fmt_f64(percent, 2),
                fmt_f64(bandwidth, 2)
            );
            // C makes stdout unbuffered, so the line shows up as it is written.
            let _ = out.flush();
            thread::sleep(Duration::from_secs(1));
            if wp >= parm.nr_blocks.wrapping_sub(1) {
                break;
            }
        }
        let _ = writeln!(out);

        let mut latencies = Vec::with_capacity(jobs);
        for (index, handle) in handles.into_iter().enumerate() {
            latencies.push(handle.join().expect("a job thread panicked"));
            print_out!(out, "finish thread {index}");
        }
        latencies
    });

    report_result(&parm, &latencies, &shared.crc32_is_match, &mut out);

    // Safety: every job has joined, so the handle is idle again.
    let close = unsafe { (*(*flash).f_op).close }.expect("the FTL installs close");
    check(unsafe { close(flash) } == 0, "close failed");
    // Safety: `flash` came from the module init and the device is closed.
    check(unsafe { ffi_module_exit(flash) } == 0, "module exit failed");

    print_parameters(&parm, &mut out);
}

/// C's `main`, as the status it returns. `out` is where the usage goes, which
/// is stderr in a run and a buffer in a test.
fn entry(argv: &[String], out: &mut impl Write) -> i32 {
    let program = argv.first().map(String::as_str).unwrap_or("benchmark");
    match parse_arguments(argv) {
        Outcome::Run(parm) => {
            run(parm);
            0
        }
        Outcome::Help => {
            print_usage(program, out);
            0
        }
        Outcome::Failed(reason) => {
            print_out!(out, "{reason}");
            print_usage(program, out);
            1
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let mut err = io::stderr();
    std::process::exit(entry(&argv, &mut err));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command line whose first entry is the program name, as C's `argv` has
    /// it.
    fn argv(line: &[&str]) -> Vec<String> {
        line.iter().map(|entry| (*entry).to_string()).collect()
    }

    /// `parse_arguments` over a line whose first entry is the program name, as
    /// C's `argv` has it.
    fn outcome(line: &[&str]) -> Outcome {
        parse_arguments(&argv(line))
    }

    /// The parameters of a line the parser accepts.
    fn parsed(line: &[&str]) -> Parameters {
        match outcome(line) {
            Outcome::Run(parm) => parm,
            Outcome::Help => panic!("the line asked for the usage"),
            Outcome::Failed(reason) => panic!("the line failed: {reason}"),
        }
    }

    /// The reason a line the parser rejects reports.
    fn failure(line: &[&str]) -> String {
        match outcome(line) {
            Outcome::Failed(reason) => reason,
            Outcome::Help => panic!("the line asked for the usage"),
            Outcome::Run(_) => panic!("the line parsed"),
        }
    }

    /// The report text of a run.
    fn report(
        parm: &Parameters,
        latencies: &[Vec<u64>],
        crc32_is_match: &[AtomicBool],
    ) -> String {
        let mut text = Vec::new();
        report_result(parm, latencies, crc32_is_match, &mut text);
        String::from_utf8(text).expect("the report is UTF-8")
    }

    #[test]
    fn the_defaults_are_the_c_defaults() {
        let parm = parsed(&["benchmark.out"]);
        assert_eq!(parm.module_idx, 0);
        assert_eq!(parm.device_idx, 0);
        assert_eq!(parm.workload, Workload::Write);
        assert_eq!(parm.block_sz, PAGE_SIZE);
        assert_eq!(parm.nr_blocks, 1);
        assert!(parm.do_warm_up);
        assert!(parm.device_path.is_empty());
        // C's `sysconf(_SC_NPROCESSORS_ONLN)`.
        assert!(parm.nr_jobs > 0);
    }

    #[test]
    fn every_flag_sets_its_parameter() {
        let parm = parsed(&[
            "benchmark.out",
            "-m",
            "pgftl",
            "-d",
            "ramdisk",
            "-t",
            "randread",
            "-j",
            "4",
            "-b",
            "1M",
            "-n",
            "8K",
            "-p",
            "/dev/ftl",
            "-w",
        ]);
        assert_eq!(parm.module_idx, 0);
        assert_eq!(parm.device_idx, 0);
        assert_eq!(parm.workload, Workload::RandRead);
        assert_eq!(parm.nr_jobs, 4);
        assert_eq!(parm.block_sz, 1 << 20);
        assert_eq!(parm.nr_blocks, 8 << 10);
        assert_eq!(parm.device_path, "/dev/ftl");
        assert!(!parm.do_warm_up);
    }

    #[test]
    fn a_device_name_picks_its_table_entry() {
        for (index, (name, _)) in DEVICES.iter().enumerate() {
            assert_eq!(parsed(&["b", "-d", name]).device_idx, index);
        }
    }

    #[test]
    fn an_argument_can_be_attached() {
        assert_eq!(parsed(&["b", "-j4"]).nr_jobs, 4);
        assert_eq!(parsed(&["b", "-mpgftl"]).module_idx, 0);
        assert_eq!(parsed(&["b", "-p/dev/ftl"]).device_path, "/dev/ftl");
    }

    #[test]
    fn argument_free_letters_cluster() {
        let parm = parsed(&["b", "-wj4"]);
        assert!(!parm.do_warm_up);
        assert_eq!(parm.nr_jobs, 4);
        assert!(matches!(outcome(&["b", "-wh"]), Outcome::Help));
    }

    #[test]
    fn help_wins_where_it_appears() {
        assert!(matches!(outcome(&["b", "-h"]), Outcome::Help));
        assert!(matches!(outcome(&["b", "-j", "2", "-h"]), Outcome::Help));
    }

    #[test]
    fn a_name_outside_the_list_reports_the_c_message() {
        for flag in ["-m", "-d", "-t"] {
            assert_eq!(
                failure(&["b", flag, "nope"]),
                "error: unexpected argument detected (nope)"
            );
        }
    }

    #[test]
    fn a_bad_flag_reports_the_c_messages() {
        // glibc's `getopt` diagnostic first, then C's own message.
        assert_eq!(
            failure(&["b", "-z"]),
            "b: invalid option -- 'z'\nunknown option character '-z'"
        );
        assert_eq!(
            failure(&["b", "-\u{1}"]),
            "b: invalid option -- '\u{1}'\nunknown option character '0x1'"
        );
        assert_eq!(
            failure(&["b", "-\u{7f}"]),
            "b: invalid option -- '\u{7f}'\n\
             unknown option character '0x7f'"
        );
        assert_eq!(
            failure(&["b", "-j"]),
            "b: option requires an argument -- 'j'\n\
             option -j requires an arguments"
        );
        assert_eq!(
            failure(&["b", "-m"]),
            "b: option requires an argument -- 'm'\n\
             option -m requires an arguments"
        );
        // A byte above `0x7f` is printed as the sign extended `char` x86-64
        // passes to `%x`.
        assert_eq!(
            processing_parameters_error(0x80),
            "unknown option character '0xffffff80'"
        );
    }

    #[test]
    fn operands_skip_the_way_glibc_permutes_them() {
        // `getopt` permutes operands out of the way, so an option after one is
        // still read.
        assert_eq!(parsed(&["b", "operand", "-j", "2"]).nr_jobs, 2);
        // A lone `-` is an operand too.
        assert_eq!(parsed(&["b", "-", "-j", "3"]).nr_jobs, 3);
        // `--` ends the options, so the default survives.
        let after = parsed(&["b", "--", "-j", "2"]);
        assert_eq!(after.nr_jobs, parsed(&["b"]).nr_jobs);
    }

    #[test]
    fn the_argument_is_taken_even_when_it_looks_like_an_option() {
        // C's `getopt` hands the next element over as the argument either way.
        assert_eq!(parsed(&["b", "-j", "-4"]).nr_jobs, -4);
    }

    #[test]
    fn the_size_suffix_is_the_first_letter_of_the_text() {
        assert_eq!(parsed(&["b", "-b", "2M"]).block_sz, 2 << 20);
        assert_eq!(parsed(&["b", "-b", "2m"]).block_sz, 2 << 20);
        assert_eq!(parsed(&["b", "-b", "4K"]).block_sz, 4 << 10);
        assert_eq!(parsed(&["b", "-b", "4k"]).block_sz, 4 << 10);
        // A text with neither letter means one.
        assert_eq!(parsed(&["b", "-b", "4096"]).block_sz, 4096);
        // The first `M` or `K` wins.
        assert_eq!(parsed(&["b", "-n", "1kM"]).nr_blocks, 1 << 10);
        assert_eq!(parsed(&["b", "-n", "1Mk"]).nr_blocks, 1 << 20);
    }

    #[test]
    fn atoi_follows_c() {
        assert_eq!(c_atoi("1048576"), 1048576);
        assert_eq!(c_atoi(""), 0);
        assert_eq!(c_atoi("  42"), 42);
        assert_eq!(c_atoi("-3"), -3);
        assert_eq!(c_atoi("+3"), 3);
        assert_eq!(c_atoi("12abc"), 12);
        assert_eq!(c_atoi("abc"), 0);
    }

    #[test]
    fn the_option_string_is_the_c_string() {
        assert_eq!(option(b'm'), Some(Argument::Required));
        assert_eq!(option(b'j'), Some(Argument::Required));
        assert_eq!(option(b'p'), Some(Argument::Required));
        assert_eq!(option(b'h'), Some(Argument::Free));
        assert_eq!(option(b'w'), Some(Argument::Free));
        assert_eq!(option(b'z'), None);
        // `:` is the argument marker, not an option letter.
        assert_eq!(option(b':'), None);
    }

    #[test]
    fn the_status_matches_c() {
        // C's `exit(0)` for `-h`.
        let mut text = Vec::new();
        assert_eq!(entry(&argv(&["b", "-h"]), &mut text), 0);
        let text = String::from_utf8(text).expect("the usage is UTF-8");
        assert!(text.starts_with("b -m <module name>"));

        // C's `exit(1)` for a name outside the list, with the reason and the
        // usage on the same stream.
        let mut text = Vec::new();
        assert_eq!(entry(&argv(&["b", "-d", "nope"]), &mut text), 1);
        let text = String::from_utf8(text).expect("the failure is UTF-8");
        assert!(text.starts_with("error: unexpected argument detected (nope)\n"));
        assert!(text.contains("\t- w           disable warm-up phase"));

        // A bad flag prints glibc's diagnostic first.
        let mut text = Vec::new();
        assert_eq!(entry(&argv(&["b", "-z"]), &mut text), 1);
        let text = String::from_utf8(text).expect("the failure is UTF-8");
        assert!(text.starts_with("b: invalid option -- 'z'\n"));
    }

    #[test]
    fn the_usage_text_is_the_c_text() {
        let mut text = Vec::new();
        print_usage("./benchmark.out", &mut text);
        let text = String::from_utf8(text).expect("the usage is UTF-8");
        assert!(text.starts_with(
            "./benchmark.out -m <module name> -d <device name> -t <workload> \
             -j <# of jobs> -b <block size(bytes)> -n <# of blocks> \
             -p <device path> [-w]\n"
        ));
        assert!(text.contains("\t- modules     [pgftl]\n"));
        assert!(text.contains(
            "\t- devices     [ramdisk, bluedbm, zone, raspberry]\n"
        ));
        assert!(text.contains(
            "\t- workloads   [write, read, randwrite, randread]\n"
        ));
        assert!(text.contains("\t- jobs        (default: 0)\n"));
        assert!(text.contains("\t- block size  (default: 4096)\n"));
        assert!(text.contains("\t- # of block  (default: 0)\n"));
        assert!(text.contains("\t- path        (default: (null))\n"));
        assert!(text.contains(
            "\t- w           disable warm-up phase (default: enabled)\n"
        ));
    }

    #[test]
    fn the_parameter_echo_is_the_readme_echo() {
        let parm = parsed(&["b", "-j", "4", "-b", "1M", "-n", "100"]);
        let mut text = Vec::new();
        print_parameters(&parm, &mut text);
        assert_eq!(
            String::from_utf8(text).expect("the echo is UTF-8"),
            "[parameters]\n\
             \t- modules     pgftl\n\
             \t- devices     ramdisk\n\
             \t- workloads   write\n\
             \t- jobs        4\n\
             \t- block size  1048576\n\
             \t- # of block  100\n\
             \t- io size     100MiB\n\
             \t- path        (null)\n\
             \t- warm-up     enabled\n"
        );
    }

    #[test]
    fn a_short_path_is_kept_and_a_long_one_is_cut() {
        assert_eq!(truncated_path("/dev/ftl"), "/dev/ftl");
        let long = "a".repeat(DEVICE_PATH_SIZE + 100);
        assert_eq!(truncated_path(&long).len(), DEVICE_PATH_SIZE - 1);
        // C cuts bytes. This one backs up to the character boundary.
        let wide = "\u{e9}".repeat(DEVICE_PATH_SIZE);
        assert_eq!(truncated_path(&wide).len(), DEVICE_PATH_SIZE - 2);
    }

    #[test]
    fn a_long_path_reaches_the_parser_cut() {
        let path = "x".repeat(DEVICE_PATH_SIZE + 100);
        let parm = parsed(&["b", "-p", &path]);
        assert_eq!(parm.device_path.len(), DEVICE_PATH_SIZE - 1);
        assert!(path.starts_with(&parm.device_path));
    }

    #[test]
    fn the_workload_names_round_trip() {
        for name in WORKLOAD_STR {
            let workload =
                Workload::from_name(name).expect("a listed name parses");
            assert_eq!(workload.name(), name);
        }
        assert!(Workload::from_name("write ").is_none());
    }

    #[test]
    fn the_workload_buckets_match_c() {
        assert!(Workload::Write.is_write() && !Workload::Write.is_read());
        assert!(Workload::Read.is_read() && !Workload::Read.is_random());
        assert!(Workload::RandWrite.is_write() && Workload::RandWrite.is_random());
        assert!(Workload::RandRead.is_read() && Workload::RandRead.is_random());
    }

    #[test]
    fn the_generator_is_the_c_xorshift64() {
        let mut state = 1u64;
        assert_eq!(xorshift64_next(&mut state), 0x4082_2041);
        // The draw is the new state, as in C.
        assert_eq!(state, 0x4082_2041);
        let mut again = 1u64;
        assert_eq!(xorshift64_next(&mut again), 0x4082_2041);
    }

    #[test]
    fn the_seed_is_never_zero() {
        assert_ne!(get_random_seed(), 0);
    }

    #[test]
    fn shuffling_is_a_permutation() {
        let mut sequence: Vec<off_t> =
            (0..64).map(|index| index * 4096).collect();
        let original = sequence.clone();
        shuffling(&mut sequence);
        sequence.sort_unstable();
        assert_eq!(sequence, original);
        // An empty sequence has nothing to shuffle.
        shuffling(&mut []);
    }

    #[test]
    fn the_fill_is_the_stream_over_the_whole_block() {
        let mut first = [0u8; 40];
        let mut second = [0u8; 40];
        let mut seed_a = 0x1234_5678_9abc_def0;
        let mut seed_b = seed_a;
        fill_buffer_random(&mut first, &mut seed_a);
        fill_buffer_random(&mut second, &mut seed_b);
        assert_eq!(first, second);
        assert_eq!(seed_a, seed_b);

        // A zero seed is replaced by a fresh one, as in the thread local C
        // seeds on first use.
        let mut block = [0u8; 16];
        let mut seed = 0;
        fill_buffer_random(&mut block, &mut seed);
        assert_ne!(seed, 0);

        // A block smaller than one word takes the tail path alone, and a block
        // that is not a multiple of eight is filled to its end.
        let mut small = [0u8; 3];
        let mut seed = 1u64;
        fill_buffer_random(&mut small, &mut seed);
        let mut expected = 1u64;
        let word = xorshift64_next(&mut expected);
        assert_eq!(small, [word as u8, (word >> 8) as u8, (word >> 16) as u8]);
        assert_eq!(seed, expected);

        for len in [1usize, 7, 9, 40] {
            let mut buffer = vec![0u8; len];
            let mut seed = 0xfeed_face_dead_beef;
            fill_buffer_random(&mut buffer, &mut seed);
            assert_ne!(seed, 0xfeed_face_dead_beef);
        }
    }

    #[test]
    fn a_filled_block_checksums_the_same_twice() {
        let mut buffer = [0u8; 128];
        let mut seed = 0xabcd_ef01_2345_6789;
        fill_buffer_random(&mut buffer, &mut seed);
        let sum = crc32(buffer.as_ptr().cast(), buffer.len(), CRC32_INIT);

        let mut same = [0u8; 128];
        let mut seed = 0xabcd_ef01_2345_6789;
        fill_buffer_random(&mut same, &mut seed);
        assert_eq!(crc32(same.as_ptr().cast(), same.len(), CRC32_INIT), sum);

        // One changed byte is what the match flag has to catch.
        same[0] ^= 0xFF;
        assert_ne!(crc32(same.as_ptr().cast(), same.len(), CRC32_INIT), sum);
    }

    #[test]
    fn the_report_table_is_the_c_table() {
        // Two jobs, one with two samples: the minimum and maximum columns keep
        // running across the rows, as they do in C.
        let parm = parsed(&["b", "-b", "1M", "-n", "4"]);
        let latencies = vec![vec![1_000_000, 3_000_000], vec![2_000_000]];
        let flags = [AtomicBool::new(true), AtomicBool::new(true)];
        assert_eq!(
            report(&parm, &latencies, &flags),
            "[job information]\n\
             id  time(s)   bw(MiB/s) iops      avg(ms)   max(ms)   min(ms)   \n\
             =====\n\
             0   0.0040    1000.0000 2         2.0000    3.0000    1.0000    \n\
             1   0.0020    2000.0000 1         2.0000    3.0000    1.0000    \n\
             [crc status]\n\
             crc check success\n"
        );
    }

    #[test]
    fn an_empty_job_reports_c_floats() {
        // C divides by zero here: the bandwidth is `inf`, the average is
        // `nan`, and the minimum stays at `INT32_MAX` nanoseconds.
        let parm = parsed(&["b", "-n", "1"]);
        let text = report(&parm, &[Vec::new()], &[AtomicBool::new(true)]);
        assert!(
            text.contains(
                "0   0.0000    inf       0         nan       0.0000    \
                 2147.4836 "
            ),
            "{text}"
        );
    }

    #[test]
    fn the_crc_verdict_follows_the_running_workload() {
        let latencies = vec![vec![1_000_000], vec![1_000_000]];
        let mixed = [AtomicBool::new(true), AtomicBool::new(false)];

        let read = parsed(&["b", "-t", "read", "-n", "2"]);
        assert!(report(&read, &latencies, &mixed).contains("crc check failed"));
        let ok = [AtomicBool::new(true), AtomicBool::new(true)];
        assert!(report(&read, &latencies, &ok).contains("crc check success"));

        // A write workload never reads the flags back, so it always passes.
        let write = parsed(&["b", "-t", "write", "-n", "2"]);
        assert!(
            report(&write, &latencies, &mixed).contains("crc check success")
        );
    }

    #[test]
    fn floats_print_the_way_c_prints_them() {
        assert_eq!(fmt_f64(0.0935, 4), "0.0935");
        assert_eq!(fmt_f64(1069.8505, 4), "1069.8505");
        assert_eq!(fmt_f64(0.0, 2), "0.00");
        // glibc spells these in lower case, Rust's `Display` does not.
        assert_eq!(fmt_f64(f64::NAN, 2), "nan");
        assert_eq!(fmt_f64(f64::INFINITY, 4), "inf");
    }
}
