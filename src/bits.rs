//! Bitmap utilities ported from `include/bits.h`.
//!
//! Two layers:
//! 1. Pure-Rust safe API on `&mut [u64]` / `&[u64]`.
//! 2. `extern "C"` FFI wrappers (`ffi_*`) on raw pointers for C interop.

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
}
