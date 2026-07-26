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
pub const BITS_PER_UINT64: usize = 64;

/// Set the bit at `index` (a bit position, not a bucket position).
///
/// The caller must ensure `index / 64` is in range for `bits`.
pub fn set_bit(bits: &mut [u64], index: u64) {
    bits[index as usize / BITS_PER_UINT64] |= 1u64 << (index % BITS_PER_UINT64 as u64);
}

/// Returns `true` if the bit at `index` is set.
pub fn get_bit(bits: &[u64], index: u64) -> bool {
    (bits[index as usize / BITS_PER_UINT64] & (1u64 << (index % BITS_PER_UINT64 as u64))) > 0
}

/// Clear the bit at `index`.
pub fn reset_bit(bits: &mut [u64], index: u64) {
    bits[index as usize / BITS_PER_UINT64] &= !(1u64 << (index % BITS_PER_UINT64 as u64));
}

/// Find the position of the first bit matching `want` (set or clear) at
/// or after `idx`, within `[idx, size)`. Returns `BITS_NOT_FOUND` if no
/// such bit exists or if the candidate position falls outside `[0, size)`.
///
/// `bits` must have at least `size.div_ceil(BITS_PER_UINT64)` elements
/// (callers using the C `BITS_TO_UINT64_ALIGN` macro satisfy this).
fn find_first_bit_matching(bits: &[u64], size: u64, idx: u64, want: bool) -> u64 {
    if idx >= size {
        return BITS_NOT_FOUND;
    }
    let mut start = idx;
    while start < size {
        let bucket_idx = start as usize / BITS_PER_UINT64;
        let bucket = bits[bucket_idx];
        // Bits strictly below `idx` (in the first iteration) are
        // masked off so the returned position is always `>= idx`.
        let lo = if start == idx {
            (start % BITS_PER_UINT64 as u64) as u32
        } else {
            0
        };
        let mask = if lo == 0 { u64::MAX } else { u64::MAX << lo };
        let probe = if want {
            bucket & mask
        } else {
            (!bucket) & mask
        };
        if probe > 0 {
            let pos = (bucket_idx as u64) * BITS_PER_UINT64 as u64 + probe.trailing_zeros() as u64;
            return if pos < size { pos } else { BITS_NOT_FOUND };
        }
        start = (bucket_idx as u64 + 1) * BITS_PER_UINT64 as u64;
    }
    BITS_NOT_FOUND
}

/// Find the position of the first `1` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when no such bit exists within `[idx, size)`. Unlike the C inline
/// in `include/bits.h`, this function never returns a position `>= size`.
///
/// `bits` must have at least `size.div_ceil(BITS_PER_UINT64)` elements.
pub fn find_first_one_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    find_first_bit_matching(bits, size, idx, true)
}

/// Find the position of the first `0` bit at or after `idx`.
///
/// `size` is the number of bits in the bitmap. Returns `BITS_NOT_FOUND`
/// when every bit in `[idx, size)` is `1`. Unlike the C inline in
/// `include/bits.h`, this function never returns a position `>= size`.
///
/// `bits` must have at least `size.div_ceil(BITS_PER_UINT64)` elements.
pub fn find_first_zero_bit(bits: &[u64], size: u64, idx: u64) -> u64 {
    find_first_bit_matching(bits, size, idx, false)
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
    *bits.add(index as usize / BITS_PER_UINT64) |= 1u64 << (index % BITS_PER_UINT64 as u64);
}

/// # Safety
/// `bits` must be non-null and point to at least `index / 64 + 1`
/// readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_get_bit(bits: *const u64, index: u64) -> c_int {
    let bucket = *bits.add(index as usize / BITS_PER_UINT64);
    let mask = 1u64 << (index % BITS_PER_UINT64 as u64);
    (bucket & mask > 0) as c_int
}

/// # Safety
/// `bits` must be non-null and point to at least `index / 64 + 1`
/// writable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_reset_bit(bits: *mut u64, index: u64) {
    *bits.add(index as usize / BITS_PER_UINT64) &= !(1u64 << (index % BITS_PER_UINT64 as u64));
}

/// # Safety
/// `bits` must be non-null and point to at least
/// `size.div_ceil(BITS_PER_UINT64)` readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_find_first_zero_bit(bits: *const u64, size: u64, idx: u64) -> u64 {
    let len = size.div_ceil(BITS_PER_UINT64 as u64) as usize;
    // SAFETY: caller guarantees `bits` is valid for `len` `u64`s.
    let slice = unsafe { std::slice::from_raw_parts(bits, len) };
    find_first_zero_bit(slice, size, idx)
}

/// # Safety
/// `bits` must be non-null and point to at least
/// `size.div_ceil(BITS_PER_UINT64)` readable `u64`s.
#[no_mangle]
pub unsafe extern "C" fn ffi_find_first_one_bit(bits: *const u64, size: u64, idx: u64) -> u64 {
    let len = size.div_ceil(BITS_PER_UINT64 as u64) as usize;
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

    #[test]
    fn find_first_one_bit_size_zero() {
        let bits: &[u64] = &[];
        assert_eq!(find_first_one_bit(bits, 0, 0), BITS_NOT_FOUND);
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

    #[test]
    fn find_first_zero_bit_size_zero() {
        let bits: &[u64] = &[];
        assert_eq!(find_first_zero_bit(bits, 0, 0), BITS_NOT_FOUND);
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
        // size=128 needs ceil(128/64) = 2 buckets
        let mut storage = [0u64; 2];
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_set_bit(ptr, 70);
        }
        let r = unsafe { ffi_find_first_one_bit(ptr, 128, 0) };
        assert_eq!(r, 70);
    }

    #[test]
    fn ffi_find_first_zero_bit_basic() {
        // size=128 needs ceil(128/64) = 2 buckets
        let storage = [u64::MAX, u64::MAX];
        let ptr = storage.as_ptr();
        let r = unsafe { ffi_find_first_zero_bit(ptr, 128, 0) };
        assert_eq!(r, BITS_NOT_FOUND);
    }

    #[test]
    fn ffi_find_first_one_bit_unaligned_idx() {
        // size=128 needs 2 buckets
        let mut storage = [0u64; 2];
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_set_bit(ptr, 70);
        }
        // idx=66 must find bit 70
        let r = unsafe { ffi_find_first_one_bit(ptr, 128, 66) };
        assert_eq!(r, 70);
    }

    #[test]
    fn ffi_find_first_zero_bit_unaligned_idx() {
        // size=128 needs 2 buckets; bits[1] has bit 6 cleared (position 70)
        let mut storage = [u64::MAX, u64::MAX];
        let ptr = storage.as_mut_ptr();
        unsafe {
            ffi_reset_bit(ptr, 70);
        }
        // idx=66 must find bit 70
        let r = unsafe { ffi_find_first_zero_bit(ptr, 128, 66) };
        assert_eq!(r, 70);
    }
}
