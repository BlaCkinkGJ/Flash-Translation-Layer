//! Bitmap utilities ported from `include/bits.h`.
//!
//! Two layers:
//! 1. Pure-Rust safe API on `&mut [u64]` / `&[u64]`.
//! 2. `unsafe extern "C"` FFI wrappers (`ffi_*`) on raw pointers for C interop.
//!
//! The `ffi_` prefix on the C-ABI functions avoids collision with the
//! `static inline` symbols still exposed by `include/bits.h` to other
//! C translation units. The FFI wrappers do **not** null-check; passing
//! `null` is undefined behaviour, matching the C contract.

use std::os::raw::c_int;

/// Sentinel returned when no matching bit is found.
pub const BITS_NOT_FOUND: u64 = u64::MAX;

/// Number of bits packed in one `u64` bucket.
pub const BITS_PER_UINT64: u64 = 64;

/// Set the bit at `index` (a bit position, not a bucket position).
///
/// The caller must ensure `index / 64` is in range for `bits`.
pub fn set_bit(bits: &mut [u64], index: u64) {
    bits[(index / BITS_PER_UINT64) as usize] |= 1u64 << (index % BITS_PER_UINT64);
}

/// Returns `true` if the bit at `index` is set.
pub fn get_bit(bits: &[u64], index: u64) -> bool {
    (bits[(index / BITS_PER_UINT64) as usize] & (1u64 << (index % BITS_PER_UINT64))) > 0
}

/// Clear the bit at `index`.
pub fn reset_bit(bits: &mut [u64], index: u64) {
    bits[(index / BITS_PER_UINT64) as usize] &= !(1u64 << (index % BITS_PER_UINT64));
}

/// Find the position of the first `1` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when no such bit exists within `[idx, size)`. Mirrors the C inline
/// in `include/bits.h` but correctly handles `idx` values that are
/// not multiples of 64, and clamps the result to `size`.
pub fn find_first_one_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    let mut start = idx;
    while start < size {
        let bucket_idx = (start / BITS_PER_UINT64) as usize;
        let bucket = bits[bucket_idx];
        // In the first iteration, mask off bits below `idx` so we never
        // return a position strictly less than `idx`. Subsequent
        // iterations are already aligned to bucket boundaries.
        let lo = if start == idx {
            start % BITS_PER_UINT64
        } else {
            0
        };
        for diff in lo..BITS_PER_UINT64 {
            if bucket & (1u64 << diff) > 0 {
                let pos = (bucket_idx as u64) * BITS_PER_UINT64 + diff;
                return if pos < size { pos } else { BITS_NOT_FOUND };
            }
        }
        start = (bucket_idx as u64 + 1) * BITS_PER_UINT64;
    }
    BITS_NOT_FOUND
}

/// Find the position of the first `0` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when every bit in `[idx, size)` is `1`. Mirror of `find_first_one_bit`.
pub fn find_first_zero_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    let mut start = idx;
    while start < size {
        let bucket_idx = (start / BITS_PER_UINT64) as usize;
        let bucket = bits[bucket_idx];
        let lo = if start == idx {
            start % BITS_PER_UINT64
        } else {
            0
        };
        for diff in lo..BITS_PER_UINT64 {
            if bucket & (1u64 << diff) == 0 {
                let pos = (bucket_idx as u64) * BITS_PER_UINT64 + diff;
                return if pos < size { pos } else { BITS_NOT_FOUND };
            }
        }
        start = (bucket_idx as u64 + 1) * BITS_PER_UINT64;
    }
    BITS_NOT_FOUND
}

// --- FFI wrappers --------------------------------------------------------
//
// Each wrapper requires the caller to uphold the safety contract (see
// per-function docs). These wrappers do NOT null-check, matching the
// C fail-loud contract.

/// # Safety
/// `bits` must be non-null and point to at least `index / 64 + 1`
/// writable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_set_bit(bits: *mut u64, index: u64) {
    *bits.add((index / BITS_PER_UINT64) as usize) |= 1u64 << (index % BITS_PER_UINT64);
}

/// # Safety
/// `bits` must be non-null and point to at least `index / 64 + 1`
/// readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_get_bit(bits: *const u64, index: u64) -> c_int {
    let bucket = *bits.add((index / BITS_PER_UINT64) as usize);
    let mask = 1u64 << (index % BITS_PER_UINT64);
    (bucket & mask > 0) as c_int
}

/// # Safety
/// `bits` must be non-null and point to at least `index / 64 + 1`
/// writable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_reset_bit(bits: *mut u64, index: u64) {
    *bits.add((index / BITS_PER_UINT64) as usize) &= !(1u64 << (index % BITS_PER_UINT64));
}

/// # Safety
/// `bits` must be non-null and point to at least `size / 64 + 1`
/// readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_find_first_zero_bit(bits: *const u64, size: u64, idx: u64) -> u64 {
    let len = (size / BITS_PER_UINT64 + 1) as usize;
    // SAFETY: caller guarantees `bits` is valid for `len` `u64`s.
    let slice = unsafe { std::slice::from_raw_parts(bits, len) };
    find_first_zero_bit(slice, size, idx)
}

/// # Safety
/// `bits` must be non-null and point to at least `size / 64 + 1`
/// readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_find_first_one_bit(bits: *const u64, size: u64, idx: u64) -> u64 {
    let len = (size / BITS_PER_UINT64 + 1) as usize;
    // SAFETY: caller guarantees `bits` is valid for `len` `u64`s.
    let slice = unsafe { std::slice::from_raw_parts(bits, len) };
    find_first_one_bit(slice, size, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- safe API: set/get/reset ----

    #[test]
    fn set_get_reset_bit_round_trip() {
        let mut bits = [0u64; 4]; // 256 bits
        for i in 0u64..256 {
            assert!(!get_bit(&bits, i), "bit {} should start cleared", i);
            set_bit(&mut bits, i);
            assert!(get_bit(&bits, i), "bit {} should be set after set_bit", i);
            reset_bit(&mut bits, i);
            assert!(
                !get_bit(&bits, i),
                "bit {} should be cleared after reset_bit",
                i
            );
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

    // ---- safe API: find_first_one_bit ----

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
    fn find_first_one_bit_respects_unaligned_idx() {
        let mut bits = [0u64; 2];
        set_bit(&mut bits, 3);
        // idx=3 finds bit 3
        assert_eq!(find_first_one_bit(&bits, 128, 3), 3);
        // idx=2 finds bit 3
        assert_eq!(find_first_one_bit(&bits, 128, 2), 3);
        // idx=4 must skip bit 3; bucket 1 is empty -> not found
        assert_eq!(find_first_one_bit(&bits, 128, 4), BITS_NOT_FOUND);
    }

    #[test]
    fn find_first_one_bit_idx_beyond_size() {
        let bits = [u64::MAX; 4];
        assert_eq!(find_first_one_bit(&bits, 256, 256), BITS_NOT_FOUND);
        assert_eq!(find_first_one_bit(&bits, 256, 1000), BITS_NOT_FOUND);
    }

    #[test]
    fn find_first_one_bit_bit_beyond_size_returns_not_found() {
        // bit 200 is set, but size=100. A bit set outside [0,size) must
        // never be returned; only bits within the requested range count.
        let mut bits = [0u64; 4];
        set_bit(&mut bits, 200);
        assert_eq!(find_first_one_bit(&bits, 100, 0), BITS_NOT_FOUND);
    }

    // ---- safe API: find_first_zero_bit ----

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

    #[test]
    fn find_first_zero_bit_respects_unaligned_idx() {
        let mut bits = [u64::MAX; 2];
        // bits[1] = u64::MAX with bit 6 cleared (position 70)
        reset_bit(&mut bits, 70);
        // idx=66 must find bit 70 in bucket 1
        assert_eq!(find_first_zero_bit(&bits, 128, 66), 70);
        // idx=70 finds bit 70 immediately
        assert_eq!(find_first_zero_bit(&bits, 128, 70), 70);
        // idx=71 must skip bit 70; no other zeros -> not found
        assert_eq!(find_first_zero_bit(&bits, 128, 71), BITS_NOT_FOUND);
    }

    #[test]
    fn find_first_zero_bit_idx_beyond_size() {
        let bits = [0u64; 4];
        assert_eq!(find_first_zero_bit(&bits, 256, 256), BITS_NOT_FOUND);
    }

    #[test]
    fn find_first_zero_bit_bit_beyond_size() {
        // size=100, all bits [0,100) are zero except... none set. Bit 200 set but out of range.
        let mut bits = [0u64; 4];
        set_bit(&mut bits, 200);
        assert_eq!(find_first_zero_bit(&bits, 100, 0), 0);
    }

    // ---- FFI wrappers ----

    #[test]
    fn ffi_set_get_reset_bit_round_trip() {
        let mut storage = [0u64; 2];
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_set_bit(ptr, 5);
            assert_eq!(ffi_get_bit(ptr, 5), 1);
            assert_eq!(ffi_get_bit(ptr, 6), 0);
            ffi_reset_bit(ptr, 5);
            assert_eq!(ffi_get_bit(ptr, 5), 0);
        }
        assert_eq!(storage[0], 0);
    }

    #[test]
    fn ffi_find_first_one_bit_basic() {
        let mut storage = [0u64; 3]; // size=128 needs 128/64+1 = 3 buckets
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_set_bit(ptr, 70);
        }
        let r = unsafe { ffi_find_first_one_bit(ptr, 128, 0) };
        assert_eq!(r, 70);
    }

    #[test]
    fn ffi_find_first_zero_bit_basic() {
        let storage = [u64::MAX, u64::MAX, u64::MAX];
        let ptr = storage.as_ptr();
        let r = unsafe { ffi_find_first_zero_bit(ptr, 128, 0) };
        assert_eq!(r, BITS_NOT_FOUND);
    }

    #[test]
    fn ffi_find_first_one_bit_unaligned_idx() {
        let mut storage = [0u64; 3];
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_set_bit(ptr, 70);
        }
        // idx=66 must find bit 70
        let r = unsafe { ffi_find_first_one_bit(ptr, 128, 66) };
        assert_eq!(r, 70);
    }
}
