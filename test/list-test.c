#include <assert.h>
#include <stdlib.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include "list.h"
#include "unity.h"

void setUp(void) {}
void tearDown(void) {}

static int int_cmp(const void *a, const void *b)
{
	const int *ia = (const int *)a;
	const int *ib = (const int *)b;
	if (*ia < *ib)
		return -1;
	if (*ia > *ib)
		return 1;
	return 0;
}

static list_node_t *make_int_list(const int *values, size_t count)
{
	list_node_t *list = NULL;
	/*
	 * Build the list in the same order as the array.
	 * We prepend elements in reverse so that the head of the list
	 * corresponds to values[0].
	 */
	if (count == 0)
		return NULL;
	for (size_t i = 0; i < count; i++) {
		list = list_prepend(list, (void *)&values[count - 1 - i]);
	}
	return list;
}

static void assert_sorted_and_linked(list_node_t *list)
{
	if (!list)
		return;
	list_node_t *prev = NULL;
	int *last_value = NULL;
	while (list) {
		/* Check backward link */
		TEST_ASSERT_EQUAL_PTR(prev, list->prev);
		/* Check non-decreasing order */
		if (last_value)
			TEST_ASSERT_TRUE(int_cmp(last_value, list->data) <= 0);
		last_value = (int *)list->data;
		prev = list;
		list = list->next;
	}
}

void test_list_sort_empty(void)
{
	list_node_t *list = NULL;
	list = list_sort(list, int_cmp);
	TEST_ASSERT_NULL(list);
}

void test_list_sort_single(void)
{
	int v = 42;
	list_node_t *list = NULL;
	list = list_prepend(list, &v);
	list = list_sort(list, int_cmp);
	TEST_ASSERT_NOT_NULL(list);
	TEST_ASSERT_NULL(list->next);
	TEST_ASSERT_EQUAL_PTR(&v, list->data);
	TEST_ASSERT_NULL(list->prev);
	list_free(list);
}

void test_list_sort_already_sorted(void)
{
	int values[] = {1, 2, 3, 4, 5};
	list_node_t *list = make_int_list(values, sizeof(values) / sizeof(values[0]));
	list = list_sort(list, int_cmp);
	assert_sorted_and_linked(list);
	list_free(list);
}

void test_list_sort_reverse_sorted(void)
{
	int values[] = {5, 4, 3, 2, 1};
	list_node_t *list = make_int_list(values, sizeof(values) / sizeof(values[0]));
	list = list_sort(list, int_cmp);
	assert_sorted_and_linked(list);
	list_free(list);
}

void test_list_sort_duplicates(void)
{
	int values[] = {3, 1, 2, 3, 2, 1};
	list_node_t *list = make_int_list(values, sizeof(values) / sizeof(values[0]));
	list = list_sort(list, int_cmp);
	assert_sorted_and_linked(list);
	list_free(list);
}

void test_list_prepend_length_last(void)
{
	list_node_t *list = NULL;
	int v1 = 10, v2 = 20, v3 = 30;

	TEST_ASSERT_EQUAL_UINT32(0, list_length(list));
	TEST_ASSERT_NULL(list_last(list));

	list = list_prepend(list, &v1);
	TEST_ASSERT_EQUAL_UINT32(1, list_length(list));
	TEST_ASSERT_EQUAL_PTR(list, list_last(list));

	list = list_prepend(list, &v2);
	list = list_prepend(list, &v3);
	TEST_ASSERT_EQUAL_UINT32(3, list_length(list));

	/* Order should be v3 -> v2 -> v1 */
	TEST_ASSERT_EQUAL_PTR(&v3, list->data);
	TEST_ASSERT_EQUAL_PTR(&v1, list_last(list)->data);

	list_free(list);
}

void test_list_remove(void)
{
	list_node_t *list = NULL;
	int v1 = 10, v2 = 20, v3 = 30, v4 = 40;

	list = list_prepend(list, &v1);
	list = list_prepend(list, &v2);
	list = list_prepend(list, &v3);

	/* Remove head */
	list = list_remove(list, &v3);
	TEST_ASSERT_EQUAL_UINT32(2, list_length(list));
	TEST_ASSERT_EQUAL_PTR(&v2, list->data);
	TEST_ASSERT_NULL(list->prev);

	/* Remove non-existent */
	list = list_remove(list, &v4);
	TEST_ASSERT_EQUAL_UINT32(2, list_length(list));

	/* Remove tail */
	list = list_remove(list, &v1);
	TEST_ASSERT_EQUAL_UINT32(1, list_length(list));
	TEST_ASSERT_EQUAL_PTR(&v2, list_last(list)->data);

	/* Remove last element */
	list = list_remove(list, &v2);
	TEST_ASSERT_NULL(list);

	list_free(list);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_list_sort_empty);
	RUN_TEST(test_list_sort_single);
	RUN_TEST(test_list_sort_already_sorted);
	RUN_TEST(test_list_sort_reverse_sorted);
	RUN_TEST(test_list_sort_duplicates);
	RUN_TEST(test_list_prepend_length_last);
	RUN_TEST(test_list_remove);
	return UNITY_END();
}
