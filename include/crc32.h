/**
 * @file crc32.h
 * @brief the CRC32 checksum
 * @author Gijun Oh
 * @version 0.1
 * @date 2026-05-02
 */
#ifndef CRC32_H
#define CRC32_H

// cppcheck-suppress missingIncludeSystem
#include <stdint.h>
// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#define CRC32_INIT (0xFFFFFFFF)

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Calculate CRC32 checksum
 * @param buf Data buffer
 * @param size Data size
 * @param initial Initial value (usually CRC32_INIT)
 * @return Calculated CRC32
 */
uint32_t crc32(const void *buf, size_t size, uint32_t initial);

#ifdef __cplusplus
}
#endif

#endif
