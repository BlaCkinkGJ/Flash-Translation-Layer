#ifndef BITS_H
#define BITS_H

#include <stdint.h>
#include <limits.h>

#define BITS_NOT_FOUND ((uint64_t)UINT64_MAX)

#define BITS_PER_BYTE (8)
#define BITS_PER_UINT64 (BITS_PER_BYTE * sizeof(uint64_t))

#define BITS_TO_UINT64_ALIGN(x)                                                \
	(((uint64_t)x / BITS_PER_UINT64 + 1) * sizeof(uint64_t))
#define BITS_TO_UINT64(x) ((uint64_t)x / BITS_PER_UINT64)

/*
 * The bitmap helpers live in Rust (`src/bits.rs`), which has been the only
 * implementation since the cutover.
 * They are header-only in C, so there is no C symbol to drop and no call site
 * to rename: the helpers below are one-line forwards to the `ffi_` entry
 * points. The names stay because a C consumer of the installed headers calls
 * them.
 */
#ifdef __cplusplus
extern "C" {
#endif

void ffi_set_bit(uint64_t *bits, uint64_t index);
int ffi_get_bit(const uint64_t *bits, uint64_t index);
void ffi_reset_bit(uint64_t *bits, uint64_t index);
uint64_t ffi_find_first_zero_bit(const uint64_t *bits, uint64_t size,
				 uint64_t idx);
uint64_t ffi_find_first_one_bit(const uint64_t *bits, uint64_t size,
				uint64_t idx);

#ifdef __cplusplus
}
#endif

/**
 * @brief set the index position bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param index set position (bit position NOT byte or uint64_t position)
 */
static inline void set_bit(uint64_t *bits, uint64_t index)
{
	ffi_set_bit(bits, index);
}

/**
 * @brief get the value at the index position bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param index get position (bit position NOT byte or uint64_t position)
 *
 * @return bit status at the index position
 */
static inline int get_bit(uint64_t *bits, uint64_t index)
{
	return ffi_get_bit(bits, index);
}

/**
 * @brief reset the index position bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param index reset position (bit position NOT byte or uint64_t position)
 */
static inline void reset_bit(uint64_t *bits, uint64_t index)
{
	ffi_reset_bit(bits, index);
}

/**
 * @brief find first zero bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param size bitmap's size (the number of bits NOT bytes)
 * @param idx start position bit
 *
 * @return first zero bit position, BITS_NOT_FOUND when there is none
 */
static inline uint64_t find_first_zero_bit(uint64_t *bits, uint64_t size,
					   uint64_t idx)
{
	return ffi_find_first_zero_bit(bits, size, idx);
}

/**
 * @brief find first one bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param size bitmap's size (the number of bits NOT bytes)
 * @param idx start position bit
 *
 * @return first one bit position, BITS_NOT_FOUND when there is none
 */
static inline uint64_t find_first_one_bit(uint64_t *bits, uint64_t size,
					  uint64_t idx)
{
	return ffi_find_first_one_bit(bits, size, idx);
}

#endif
