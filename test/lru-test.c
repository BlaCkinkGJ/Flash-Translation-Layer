#include "include/lru.h"
#include "unity.h"

#include <stdlib.h>
#include <time.h>
#include <stdint.h>

void setUp(void)
{
}

void tearDown(void)
{
}

static void lru_internal_print(struct lru_cache *cache)
{
	struct lru_node *head = cache->head;
	struct lru_node *it = head->next;
	while (it != head) {
		printf("%ld(%ld), ", it->key, it->value);
		it = it->next;
	}
	printf("\n");
}

void test_lru_init(void)
{
	struct lru_cache *cache;
	cache = lru_init(0, NULL);
	TEST_ASSERT_NULL(cache);
	cache = lru_init(10, NULL);
	TEST_ASSERT_NOT_NULL(cache);
	lru_free(cache);
}

void test_lru_fill(void)
{
	struct lru_cache *cache;
	cache = lru_init(10, NULL);
	for (int i = 1; i <= 10; i++) {
		TEST_ASSERT_EQUAL_INT(lru_put(cache, i, i * 2), 0);
	}
	lru_internal_print(cache);
	for (int i = 1; i <= 10; i++) {
		TEST_ASSERT_EQUAL_INT(lru_get(cache, i), i * 2);
		printf("=========================\n");
		lru_internal_print(cache);
	}
	lru_internal_print(cache);
	for (int i = 1; i <= 10; i++) {
		TEST_ASSERT_EQUAL_INT(lru_put(cache, i + 10, (i + 10) * 2), 0);
		printf("=========================\n");
		lru_internal_print(cache);
	}
	lru_free(cache);
}

static int dealloc_data(const uint64_t key, uintptr_t value)
{
	int *data = (int *)value;
	(void)key;
	TEST_ASSERT_NOT_NULL(data);
	free(data);
	return 0;
}

void test_lru_big_fill(void)
{
	const int cache_size = 1024;
	const int total_size = cache_size * 100;
	int counter = 0;
	int last_size = 0;
	struct lru_cache *cache;
	cache = lru_init(cache_size, dealloc_data);
	for (int i = 0; i < total_size; i++) {
		int *data;
		data = (int *)malloc(sizeof(int));
		*data = i;
		lru_put(cache, i, (uintptr_t)data);
	}
	last_size = cache->size;
	for (int i = total_size - 1; i >= 0; i--) {
		if (counter >= last_size) {
			TEST_ASSERT_NULL((void *)lru_get(cache, i));
		} else {
			uintptr_t data = lru_get(cache, i);
			TEST_ASSERT_NOT_NULL((void *)data);
			TEST_ASSERT_EQUAL_INT(*(int *)data, i);
		}
		counter += 1;
	}
	TEST_ASSERT_EQUAL_INT(lru_free(cache), 0);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_lru_init);
	RUN_TEST(test_lru_fill);
	RUN_TEST(test_lru_big_fill);
	return UNITY_END();
}
