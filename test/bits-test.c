#include "bits.h"
#include "unity.h"

#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

void setUp(void)
{
}

void tearDown(void)
{
}

void test_bits(void)
{
	const uint64_t nr_bits = 4096;
	uint64_t *bits;
	uint64_t zero, one;
	bits = (uint64_t *)malloc(BITS_TO_UINT64_ALIGN(nr_bits));
	memset(bits, 0, BITS_TO_UINT64_ALIGN(nr_bits));
	one = find_first_one_bit(bits, nr_bits, 0);
	zero = find_first_zero_bit(bits, nr_bits, 0);
	for (uint64_t i = 0; i < nr_bits; i++) {
		set_bit(bits, i);
		one = find_first_one_bit(bits, nr_bits, 0);
		zero = find_first_zero_bit(bits, nr_bits, 0);
		TEST_ASSERT_EQUAL_UINT64(0, one);
		if (i + 1 < nr_bits) {
			TEST_ASSERT_EQUAL_UINT64(i + 1, zero);
		} else {
			TEST_ASSERT_EQUAL_INT64(-1, (int64_t)zero);
		}
	}
	for (uint64_t i = 0; i < nr_bits; i++) {
		reset_bit(bits, i);
		one = find_first_one_bit(bits, nr_bits, 0);
		zero = find_first_zero_bit(bits, nr_bits, 0);
		if (i + 1 < nr_bits) {
			TEST_ASSERT_EQUAL_UINT64(i + 1, one);
		} else {
			TEST_ASSERT_EQUAL_INT64(-1, (int64_t)one);
		}
		TEST_ASSERT_EQUAL_UINT64(0, zero);
	}

	// For arm machine
	(void)zero;
	(void)one;

	free(bits);
}

void test_get_bits(void)
{
	int counter = 20;
	while (counter) {
		const uint64_t nr_bits = (0x1 << counter);
		uint64_t i;
		char *setbit;
		uint64_t *bits;
		setbit = (char *)malloc((size_t)nr_bits);
		memset(setbit, 0, (size_t)nr_bits);
		bits = (uint64_t *)malloc(
			(size_t)BITS_TO_UINT64_ALIGN(nr_bits));
		memset(bits, 0, (size_t)BITS_TO_UINT64_ALIGN(nr_bits));
		srand((unsigned int)time(NULL) + (counter * rand()) % INT_MAX);
		for (i = 0; i < nr_bits; i++) {
			setbit[i] = (char)(rand() % 2);
		}
		for (i = 0; i < nr_bits; i++) {
			if (setbit[i] == 0) {
				continue;
			}
			set_bit(bits, i);
		}
		for (i = 0; i < nr_bits; i++) {
			int bit = get_bit(bits, i);
			TEST_ASSERT_EQUAL_INT(setbit[i], bit);
		}
		free(bits);
		free(setbit);
		counter -= 1;
	}
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_bits);
	RUN_TEST(test_get_bits);
	return UNITY_END();
}
