#ifndef BITS_H
#define BITS_H

#include <stdint.h>
#include <limits.h>

#define BITS_NOT_FOUND (UINT64_MAX)

#define BITS_PER_BYTE (8)
#define BITS_PER_UINT64 (BITS_PER_BYTE * sizeof(uint64_t))

#define BITS_TO_BYTES(x) (x / BITS_PER_BYTE)
#define BITS_TO_UINT64(x) (x / BITS_PER_UINT64)

static inline void set_bit(uint64_t *bits, uint64_t index)
{
	bits[BITS_TO_UINT64(index)] |=
		((uint64_t)0x1 << (index % BITS_PER_UINT64));
}

static inline void reset_bit(uint64_t *bits, uint64_t index)
{
	bits[BITS_TO_UINT64(index)] &=
		~((uint64_t)0x1 << (index % BITS_PER_UINT64));
}

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
