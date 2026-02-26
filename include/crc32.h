#ifndef CRC32_H
#define CRC32_H

// cppcheck-suppress missingIncludeSystem
#include <stdint.h>
// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

uint32_t crc32(const void *buf, size_t size, uint32_t initial);

#ifdef __cplusplus
}
#endif

#endif
