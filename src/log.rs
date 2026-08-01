//! Logging facade over the [`log`] crate. Macros mirror the
//! `pr_info!` / `pr_warn!` / `pr_err!` / `pr_debug!` names from
//! `include/log.h` so Rust call sites match the C convention. When
//! the `silent` cargo feature is on, the macros expand to no-ops
//! (C `-DENABLE_LOG_SILENT` equivalent).
//!
//! Level filtering is handled by the `log` facade. The bundled
//! backend ([`env_logger`]) reads `RUST_LOG` — e.g. `RUST_LOG=debug`
//! to enable `pr_debug!` in release. The backend is installed
//! exactly once on first macro use; callers may also invoke
//! [`init()`] explicitly at startup.

use std::sync::Once;
static INIT: Once = Once::new();

/// Install `env_logger` exactly once. Safe to call repeatedly;
/// subsequent calls are no-ops.
pub fn init() {
    INIT.call_once(|| {
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("info"),
        )
        .try_init();
    });
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
macro_rules! pr_info { ($($arg:tt)*) => {}; }

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
macro_rules! pr_warn { ($($arg:tt)*) => {}; }

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
macro_rules! pr_err { ($($arg:tt)*) => {}; }

/// `pr_debug!` is filtered by the `log` crate via `RUST_LOG=debug`.
/// The macro itself is not gated by `debug_assertions` — release
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
    /// Output is unobservable here (no env_logger init in tests).
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
