#include "include/bits.h"
#include "unity.h"

#include <stdio.h>
#include <stdlib.h>

void setUp(void)
{
}

void tearDown(void)
{
}

static void bits_print(uint64_t *bits, uint64_t size)
{
	uint64_t i;
	for (i = 0; i < size; i += BITS_PER_UINT64) {
		printf("%016lx ",
		       bits[BITS_TO_UINT64(size) - BITS_TO_UINT64(i) - 1]);
		if ((i + 1) % 4 == 0) {
			printf("\n");
		}
	}
	printf("\n");
}

void test_bits(void)
{
	uint64_t *bits;
	uint64_t nr_bits = (256);
	uint64_t zero, one;
	bits = (uint64_t *)malloc(BITS_TO_BYTES(nr_bits));
	bits_print(bits, nr_bits);
	one = find_first_one_bit(bits, nr_bits, 0);
	zero = find_first_zero_bit(bits, nr_bits, 0);
	printf("%ld %ld\n", zero, one);
	for (uint64_t i = 0; i < nr_bits; i++) {
		set_bit(bits, i);
		bits_print(bits, nr_bits);
		one = find_first_one_bit(bits, nr_bits, 0);
		zero = find_first_zero_bit(bits, nr_bits, 0);
		printf("%ld %ld\n", zero, one);
	}
	for (uint64_t i = 0; i < nr_bits; i++) {
		reset_bit(bits, i);
		bits_print(bits, nr_bits);
		one = find_first_one_bit(bits, nr_bits, 0);
		zero = find_first_zero_bit(bits, nr_bits, 0);
		printf("%ld %ld\n", zero, one);
	}
	free(bits);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_bits);
	return UNITY_END();
}
