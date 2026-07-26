//! Bitmap utilities ported from `include/bits.h`.
//!
//! Two layers:
//! 1. Pure-Rust safe API on `&mut [u64]` / `&[u64]`.
//! 2. `extern "C"` FFI wrappers (`ffi_*`) on raw pointers for C interop.

/// Sentinel returned when no matching bit is found.
pub const BITS_NOT_FOUND: u64 = u64::MAX;

/// Number of bits packed in one `u64` bucket.
pub const BITS_PER_UINT64: usize = 64;
