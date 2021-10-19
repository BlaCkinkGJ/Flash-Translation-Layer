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

/**
 * @brief set the index position bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param index set position (bit position NOT byte or uint64_t position)
 */
static inline void set_bit(uint64_t *bits, uint64_t index)
{
	bits[BITS_TO_UINT64(index)] |=
		((uint64_t)0x1 << (index % BITS_PER_UINT64));
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
	return (bits[BITS_TO_UINT64(index)] &
		((uint64_t)0x1 << (index % BITS_PER_UINT64))) > 0;
}

/**
 * @brief reset the index position bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param index reset position (bit position NOT byte or uint64_t position)
 */
static inline void reset_bit(uint64_t *bits, uint64_t index)
{
	bits[BITS_TO_UINT64(index)] &=
		~((uint64_t)0x1 << (index % BITS_PER_UINT64));
}

/**
 * @brief find first zero bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param size bitmap's size (the number of bits NOT bytes)
 * @param idx start position bit
 *
 * @return first zero bit position
 */
static inline uint64_t find_first_zero_bit(uint64_t *bits, uint64_t size,
					   uint64_t idx)
{
	while (idx < size) {
		uint64_t bucket = bits[BITS_TO_UINT64(idx)];
		if (bucket < (uint64_t)UINT64_MAX) {
			uint64_t diff = 0;
			for (diff = 0; diff < BITS_PER_UINT64; diff++) {
				if ((bucket & (uint64_t)((uint64_t)0x1
							 << diff)) == 0x0) {
					break;
				}
			}
			return idx + diff;
		}
		idx += BITS_PER_UINT64;
	}
	return BITS_NOT_FOUND;
}

/**
 * @brief find first one bit in the array(uint64_t)
 *
 * @param bits array which contains the bitmap
 * @param size bitmap's size (the number of bits NOT bytes)
 * @param idx start position bit
 *
 * @return first one bit position
 */
static inline uint64_t find_first_one_bit(uint64_t *bits, uint64_t size,
					  uint64_t idx)
{
	while (idx < size) {
		uint64_t bucket = bits[BITS_TO_UINT64(idx)];
		if (bucket > (uint64_t)0x0) {
			uint64_t diff = 0;
			for (diff = 0; diff < BITS_PER_UINT64; diff++) {
				if ((bucket &
				     (uint64_t)((uint64_t)0x1 << diff)) > 0) {
					break;
				}
			}
			return idx + diff;
		}
		idx += BITS_PER_UINT64;
	}
	return BITS_NOT_FOUND;
}

#endif
