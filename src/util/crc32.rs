use crc32fast::Hasher;
use std::slice;

#[no_mangle]
pub extern "C" fn crc32(buf: *const u8, len: usize, initial: u32) -> u32 {
    // The previous table-based experiment confirmed:
    // C expectation for "123456789" with initial 0xFFFFFFFF is 0xCBF43926.
    
    // In Rust crc32fast:
    // Hasher::new() starts with an internal state that yields 0xCBF43926 for "123456789".
    // Hasher::new_with_initial(0) IS EQUIVALENT to Hasher::new().
    
    // Therefore, C's initial 0xFFFFFFFF (CRC32_INIT) maps to Rust's initial 0.
    // C's initial 0 maps to Rust's initial 0xFFFFFFFF.
    // Mapping: Rust_initial = C_initial ^ 0xFFFFFFFF
    
    let mut hasher = Hasher::new_with_initial(initial ^ 0xFFFFFFFF);
    if !buf.is_null() && len > 0 {
        let data = unsafe { slice::from_raw_parts(buf, len) };
        hasher.update(data);
    }
    
    // The table-based experiment showed that C result = (final_internal_state ^ 0xFFFFFFFF)
    // Rust's hasher.finalize() ALREADY returns (internal_state ^ 0xFFFFFFFF).
    
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_parity_with_c() {
        let data = b"123456789";
        // C expects 0xCBF43926 when initial is 0xFFFFFFFF
        let result = crc32(data.as_ptr(), data.len(), 0xFFFFFFFF);
        assert_eq!(result, 0xCBF43926);
    }

    #[test]
    fn test_crc32_empty_parity() {
        // C expects 0xFFFFFFFF when buf is "" and initial is 0
        // (0 ^ 0xFFFFFFFF) -> loop nothing -> return (0 ^ 0xFFFFFFFF) = 0xFFFFFFFF
        let result = crc32(std::ptr::null(), 0, 0);
        assert_eq!(result, 0xFFFFFFFF);
    }
    
    #[test]
    fn test_crc32_incremental_parity() {
        let data = b"123456789";
        let crc_full = crc32(data.as_ptr(), data.len(), 0xFFFFFFFF);
        
        let crc_part1 = crc32(data.as_ptr(), 4, 0xFFFFFFFF);
        // In C incremental: next_initial = last_result ^ 0xFFFFFFFF (to get raw state)
        let crc_part2 = crc32(unsafe { data.as_ptr().add(4) }, 5, crc_part1 ^ 0xFFFFFFFF);
        
        assert_eq!(crc_full, crc_part2);
    }
}
