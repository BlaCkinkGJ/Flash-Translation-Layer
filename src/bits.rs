//! Bitmap utilities ported from `include/bits.h`.
//!
//! Two layers:
//! 1. Pure-Rust safe API on `&mut [u64]` / `&[u64]`.
//! 2. `extern "C"` FFI wrappers (`ffi_*`) on raw pointers for C interop.

use std::os::raw::c_int;

/// Sentinel returned when no matching bit is found.
pub const BITS_NOT_FOUND: u64 = u64::MAX;

/// Number of bits packed in one `u64` bucket.
pub const BITS_PER_UINT64: usize = 64;

/// Set the bit at `index` (bit position, not bucket position) in `bits`.
///
/// The caller must ensure that `index / 64` is in range for `bits`.
pub fn set_bit(bits: &mut [u64], index: u64) {
    bits[(index / BITS_PER_UINT64 as u64) as usize] |=
        1u64 << (index % BITS_PER_UINT64 as u64);
}

/// Returns `true` if the bit at `index` is set.
pub fn get_bit(bits: &[u64], index: u64) -> bool {
    (bits[(index / BITS_PER_UINT64 as u64) as usize]
        & (1u64 << (index % BITS_PER_UINT64 as u64)))
        > 0
}

/// Clear the bit at `index`.
pub fn reset_bit(bits: &mut [u64], index: u64) {
    bits[(index / BITS_PER_UINT64 as u64) as usize] &=
        !(1u64 << (index % BITS_PER_UINT64 as u64));
}

/// Find the position of the first `1` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when no such bit exists within `[idx, size)`. Semantics mirror the
/// C inline in `include/bits.h`: the search bucket-walks in `u64`
/// strides and within each non-empty bucket scans from bit 0 upward.
pub fn find_first_one_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    let mut idx = idx;
    while idx < size {
        let bucket = bits[(idx / BITS_PER_UINT64 as u64) as usize];
        if bucket > 0 {
            for diff in 0..BITS_PER_UINT64 as u64 {
                if (bucket & (1u64 << diff)) > 0 {
                    return idx + diff;
                }
            }
        }
        idx += BITS_PER_UINT64 as u64;
    }
    BITS_NOT_FOUND
}

/// Find the position of the first `0` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when every bit in `[idx, size)` is `1`. Mirror of `find_first_one_bit`.
pub fn find_first_zero_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    let mut idx = idx;
    while idx < size {
        let bucket = bits[(idx / BITS_PER_UINT64 as u64) as usize];
        if bucket < u64::MAX {
            for diff in 0..BITS_PER_UINT64 as u64 {
                if (bucket & (1u64 << diff)) == 0 {
                    return idx + diff;
                }
            }
        }
        idx += BITS_PER_UINT64 as u64;
    }
    BITS_NOT_FOUND
}

// --- FFI wrappers --------------------------------------------------------
//
// These mirror the C inline functions in `include/bits.h` so that C code
// can call into the Rust implementation. They are prefixed with `ffi_`
// to avoid symbol collision with the C inline functions still in
// `bits.h`. Each function requires the caller to uphold the listed
// safety invariants; they intentionally perform no bounds checks
// (matching C behaviour).

/// # Safety
/// `bits` must point to at least `index / 64 + 1` writable `u64`s.
#[no_mangle]
pub extern "C" fn ffi_set_bit(bits: *mut u64, index: u64) {
    if bits.is_null() {
        return;
    }
    // SAFETY: caller guarantees the bucket at `index / 64` is writable.
    unsafe {
        (*bits.add((index / BITS_PER_UINT64 as u64) as usize)) |=
            1u64 << (index % BITS_PER_UINT64 as u64);
    }
}

/// # Safety
/// `bits` must point to at least `index / 64 + 1` readable `u64`s.
#[no_mangle]
pub extern "C" fn ffi_get_bit(bits: *const u64, index: u64) -> c_int {
    if bits.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees the bucket at `index / 64` is readable.
    let v = unsafe {
        (*bits.add((index / BITS_PER_UINT64 as u64) as usize))
            & (1u64 << (index % BITS_PER_UINT64 as u64))
    };
    (v > 0) as c_int
}

/// # Safety
/// `bits` must point to at least `index / 64 + 1` writable `u64`s.
#[no_mangle]
pub extern "C" fn ffi_reset_bit(bits: *mut u64, index: u64) {
    if bits.is_null() {
        return;
    }
    // SAFETY: caller guarantees the bucket at `index / 64` is writable.
    unsafe {
        (*bits.add((index / BITS_PER_UINT64 as u64) as usize)) &=
            !(1u64 << (index % BITS_PER_UINT64 as u64));
    }
}

/// # Safety
/// `bits` must point to at least `(size / 64) + 1` readable `u64`s.
#[no_mangle]
pub extern "C" fn ffi_find_first_zero_bit(
    bits: *const u64,
    size: u64,
    idx: u64,
) -> u64 {
    if bits.is_null() {
        return BITS_NOT_FOUND;
    }
    // Length must cover the highest bucket we may index: `size / 64`,
    // plus 1 because the C version also reads the bucket containing
    // bit `size - 1`.
    let len = (size / BITS_PER_UINT64 as u64 + 1) as usize;
    // SAFETY: caller guarantees `bits` is valid for `len` `u64`s.
    let slice = unsafe { std::slice::from_raw_parts(bits, len) };
    find_first_zero_bit(slice, size, idx)
}

/// # Safety
/// `bits` must point to at least `(size / 64) + 1` readable `u64`s.
#[no_mangle]
pub extern "C" fn ffi_find_first_one_bit(
    bits: *const u64,
    size: u64,
    idx: u64,
) -> u64 {
    if bits.is_null() {
        return BITS_NOT_FOUND;
    }
    let len = (size / BITS_PER_UINT64 as u64 + 1) as usize;
    // SAFETY: caller guarantees `bits` is valid for `len` `u64`s.
    let slice = unsafe { std::slice::from_raw_parts(bits, len) };
    find_first_one_bit(slice, size, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_reset_bit_round_trip() {
        let mut bits = [0u64; 4]; // 256 bits
        for i in 0u64..256 {
            assert!(!get_bit(&bits, i), "bit {} should start cleared", i);
            set_bit(&mut bits, i);
            assert!(get_bit(&bits, i), "bit {} should be set after set_bit", i);
            reset_bit(&mut bits, i);
            assert!(!get_bit(&bits, i), "bit {} should be cleared after reset_bit", i);
        }
    }

    #[test]
    fn set_bit_does_not_touch_neighbours() {
        let mut bits = [0u64; 2];
        set_bit(&mut bits, 5);
        assert_eq!(bits[0], 1u64 << 5);
        set_bit(&mut bits, 63);
        assert_eq!(bits[0], (1u64 << 5) | (1u64 << 63));
        set_bit(&mut bits, 64); // crosses into bits[1]
        assert_eq!(bits[0], (1u64 << 5) | (1u64 << 63));
        assert_eq!(bits[1], 1u64);
    }

    #[test]
    fn reset_bit_does_not_touch_neighbours() {
        let mut bits = [u64::MAX; 2];
        reset_bit(&mut bits, 0);
        reset_bit(&mut bits, 64);
        assert_eq!(bits[0], u64::MAX - 1);
        assert_eq!(bits[1], u64::MAX - 1);
    }

    #[test]
    fn find_first_one_bit_empty_map_returns_not_found() {
        let bits = [0u64; 4];
        assert_eq!(find_first_one_bit(&bits, 256, 0), BITS_NOT_FOUND);
    }

    #[test]
    fn find_first_one_bit_returns_first_set_bit() {
        let mut bits = [0u64; 1];
        set_bit(&mut bits, 3);
        set_bit(&mut bits, 7);
        set_bit(&mut bits, 40);
        assert_eq!(find_first_one_bit(&bits, 64, 0), 3);
    }

    #[test]
    fn find_first_zero_bit_empty_map_returns_zero() {
        let bits = [0u64; 4];
        assert_eq!(find_first_zero_bit(&bits, 256, 0), 0);
    }

    #[test]
    fn find_first_zero_bit_skips_full_buckets() {
        // bucket 0 all ones, bucket 1 has zero at position 70 (= 64 + 6)
        let mut bits = [u64::MAX, u64::MAX];
        reset_bit(&mut bits, 70);
        assert_eq!(find_first_zero_bit(&bits, 128, 0), 70);
    }
}
