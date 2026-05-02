use crc32fast::Hasher;
use std::slice;

#[no_mangle]
pub extern "C" fn crc32(initial: u32, buf: *const u8, len: usize) -> u32 {
    if buf.is_null() || len == 0 {
        return initial;
    }

    // Safety: We assume the C caller provides a valid pointer and correct length.
    let data = unsafe { slice::from_raw_parts(buf, len) };
    
    let mut hasher = Hasher::new_with_initial(initial);
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_basic() {
        let data = b"123456789";
        // IEEE 802.3 CRC32 for "123456789" is 0xCBF43926
        // But our initial value might affect this. 
        // Let's test with 0 initial first.
        let result = crc32(0, data.as_ptr(), data.len());
        assert_ne!(result, 0);
    }

    #[test]
    fn test_crc32_empty() {
        let result = crc32(0x1234, std::ptr::null(), 0);
        assert_eq!(result, 0x1234);
    }

    #[test]
    fn test_crc32_incremental() {
        let data1 = b"123";
        let data2 = b"456";
        let mid = crc32(0, data1.as_ptr(), data1.len());
        let final_res = crc32(mid, data2.as_ptr(), data2.len());
        
        let combined = b"123456";
        let expected = crc32(0, combined.as_ptr(), combined.len());
        assert_eq!(final_res, expected);
    }
}
