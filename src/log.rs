//! Logging facade over the [`log`] crate. The macros keep the `pr_info!`,
//! `pr_warn!`, `pr_err!` and `pr_debug!` names from `include/log.h`, so Rust
//! call sites match the C convention. With the `silent` cargo feature on, they
//! expand to no-ops, C's `-DENABLE_LOG_SILENT` equivalent.
//!
//! `RUST_LOG` sets the level, e.g. `RUST_LOG=debug` enables `pr_debug!` in
//! release. Output is one JSON object per line, Loki-compatible. The
//! `env_logger` sink can be swapped for a Loki / OpenTelemetry exporter later
//! without touching call sites.

use std::io::Write;

/// Install `env_logger` with JSON output. Safe to call repeatedly.
/// `try_init` no-ops after the first successful install.
pub fn init() {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format(json_format)
    .try_init();
}

fn json_format(
    buf: &mut env_logger::fmt::Formatter,
    record: &log::Record,
) -> std::io::Result<()> {
    writeln!(
        buf,
        r#"{{"ts":"{}","level":"{}","target":"{}","msg":"{}"}}"#,
        buf.timestamp(),
        record.level(),
        record.target(),
        json_escape(&record.args().to_string()),
    )
}

/// ponytail: inline JSON escape. Stdlib has no equivalent without
/// pulling serde_json. Handles the chars that break JSON parsers:
/// quote, backslash, CR, LF, TAB. Upgrade to a single-pass loop if
/// log throughput ever matters.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_info {
    ($($arg:tt)*) => {{
        $crate::log::init();
        log::info!($($arg)*);
    }};
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_info { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }

#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_warn {
    ($($arg:tt)*) => {{
        $crate::log::init();
        log::warn!($($arg)*);
    }};
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_warn { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }

#[cfg(not(feature = "silent"))]
#[macro_export]
macro_rules! pr_err {
    ($($arg:tt)*) => {{
        $crate::log::init();
        log::error!($($arg)*);
    }};
}

#[cfg(feature = "silent")]
#[macro_export]
macro_rules! pr_err { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }

/// `pr_debug!` is filtered by the `log` crate via `RUST_LOG=debug`.
/// The macro itself is not gated by `debug_assertions`, so release
/// builds with the right filter will see debug output.
#[macro_export]
macro_rules! pr_debug {
    ($($arg:tt)*) => {{
        $crate::log::init();
        log::debug!($($arg)*);
    }};
}

#[cfg(test)]
mod tests {
    /// All four macros must compile and run without panicking.
    /// JSON output is emitted to stderr (one object per line).
    #[test]
    fn all_macros_compile_and_run() {
        pr_info!("zero-arg info");
        pr_info!("with arg: {}", 42);
        pr_warn!("zero-arg warn");
        pr_warn!("with arg: {}", "x");
        pr_err!("zero-arg err");
        pr_err!("with arg: {}", -1i32);
        pr_debug!("zero-arg debug");
        pr_debug!("with arg: {}", 0.5_f32);
    }
}
