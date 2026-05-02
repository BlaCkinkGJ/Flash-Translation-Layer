#include <string.h>
#include "crc32.h"
#include "unity.h"

void setUp(void) {}
void tearDown(void) {}

void test_crc32_empty(void)
{
	uint32_t crc;
	crc = crc32("", 0, 0x00000000u);
	TEST_ASSERT_EQUAL_UINT32((0x00000000u ^ 0xFFFFFFFFu), crc);

	crc = crc32("", 0, 0xFFFFFFFFu);
	TEST_ASSERT_EQUAL_UINT32((0xFFFFFFFFu ^ 0xFFFFFFFFu), crc);
}

void test_crc32_standard(void)
{
	const char *s = "123456789";
	uint32_t crc;
	crc = crc32(s, strlen(s), CRC32_INIT);
	TEST_ASSERT_EQUAL_UINT32(0xCBF43926u, crc);
}

void test_crc32_incremental(void)
{
	const char *s = "123456789";
	uint32_t crc_full;
	uint32_t crc_inc;

	crc_full = crc32(s, strlen(s), CRC32_INIT);

	crc_inc = crc32(s, 4, CRC32_INIT);
	/* Next chunk must undo the final XOR of the previous step */
	crc_inc = crc32(s + 4, strlen(s) - 4, crc_inc ^ 0xFFFFFFFFu);

	TEST_ASSERT_EQUAL_UINT32(crc_full, crc_inc);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_crc32_empty);
	RUN_TEST(test_crc32_standard);
	RUN_TEST(test_crc32_incremental);
	return UNITY_END();
}
