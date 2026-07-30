//! Logging macros ported from `include/log.h`.
//!
//! Format mirrors the C version:
//!   `<LEVEL>:[<FILE>:<module_path>(<LINE>)] <fmt>`
//!
//! `pr_debug!` is compiled in only under `debug_assertions` (C `#ifdef
//! DEBUG` equivalent). `pr_info!` / `pr_warn!` / `pr_err!` are
//! compiled to no-ops when the `silent` cargo feature is enabled
//! (C `-DENABLE_LOG_SILENT` equivalent).

/// Print an info-level message to stdout.
#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_info {
    ($fmt:literal $(, $arg:expr)*) => {
        std::println!(
            concat!("INFO:[", file!(), ":", module_path!(), "(", line!(), ")] ", $fmt),
            $($arg),*
        )
    };
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_info { ($($arg:tt)*) => {}; }

/// Print a warning-level message to stderr.
#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_warn {
    ($fmt:literal $(, $arg:expr)*) => {
        std::eprintln!(
            concat!("WARNING:[", file!(), ":", module_path!(), "(", line!(), ")] ", $fmt),
            $($arg),*
        )
    };
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_warn { ($($arg:tt)*) => {}; }

/// Print an error-level message to stderr.
#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_err {
    ($fmt:literal $(, $arg:expr)*) => {
        std::eprintln!(
            concat!("ERROR:[", file!(), ":", module_path!(), "(", line!(), ")] ", $fmt),
            $($arg),*
        )
    };
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_err { ($($arg:tt)*) => {}; }

/// Print a debug-level message to stdout. Compiled in only when
/// `debug_assertions` is on (e.g. `cargo test` or non-`--release`
/// builds).
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! pr_debug {
    ($fmt:literal $(, $arg:expr)*) => {
        std::println!(
            concat!("DEBUG:[", file!(), ":", module_path!(), "(", line!(), ")] ", $fmt),
            $($arg),*
        )
    };
}

#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! pr_debug { ($($arg:tt)*) => {}; }

#[cfg(test)]
mod tests {
    /// All four macros must compile and run without panicking for
    /// both zero-arg and with-arg form. This is the entire contract —
    /// macros are side effects, output is unobservable here.
    #[test]
    fn all_macros_compile_and_run() {
        pr_info!("zero-arg info");
        pr_info!("with arg: {}", 42);
        pr_warn!("zero-arg warn");
        pr_warn!("with arg: {}", "x");
        pr_err!("zero-arg err");
        pr_err!("with arg: {}", -1i32);
        pr_debug!("zero-arg debug");
        pr_debug!("with arg: {}", 3.14_f32);
    }
}
