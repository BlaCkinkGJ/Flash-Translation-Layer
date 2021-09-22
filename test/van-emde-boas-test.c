#include "include/van-emde-boas.h"
#include "unity.h"

struct vEB *g_bitmap;

void setUp(void)
{
	g_bitmap = vEB_init(1024);
	TEST_ASSERT_EQUAL_INT(vEB_tree_insert(g_bitmap, 0), 0);
	for (int i = 0; i < 10; i++) {
		TEST_ASSERT_EQUAL_INT(vEB_tree_insert(g_bitmap, 0x1 << i), 0);
	}
	TEST_ASSERT_EQUAL_INT(vEB_tree_insert(g_bitmap, 0x1 << 10), -1);
}

void tearDown(void)
{
	vEB_free(g_bitmap);
}

void test_vEB_init(void)
{
	struct vEB *bitmap;
	bitmap = vEB_init(-1);
	TEST_ASSERT_NULL(bitmap);

	bitmap = vEB_init(0);
	TEST_ASSERT_NULL(bitmap);

	bitmap = vEB_init(10);
	TEST_ASSERT_NOT_NULL(bitmap);
	vEB_free(bitmap);

	bitmap = NULL;
	bitmap = vEB_init(4096 * 2048);
	TEST_ASSERT_NOT_NULL(bitmap);
	vEB_free(bitmap);
}

void test_vEB_member(void)
{
	TEST_ASSERT_EQUAL_INT(vEB_tree_member(g_bitmap, 0), 1);
	for (int i = 0; i < 10; i++) {
		TEST_ASSERT_EQUAL_INT(vEB_tree_member(g_bitmap, 0x1 << i), 1);
	}
	TEST_ASSERT_EQUAL_INT(vEB_tree_member(g_bitmap, 0x1 << 10), NIL);
	TEST_ASSERT_EQUAL_INT(vEB_tree_member(g_bitmap, -1), NIL);
}

void test_vEB_successor(void)
{
	for (int i = 0; i < 10; i++) {
		int j = (i == 0 ? 0 : 0x1 << (i - 1));
		while (j < (0x1 << i)) {
			TEST_ASSERT_EQUAL_INT(vEB_tree_successor(g_bitmap, j),
					      0x1 << i);
			j++;
		}
	}
	for (int i = (0x1 << 9); i < (0x1 << 10); i++) {
		TEST_ASSERT_EQUAL_INT(vEB_tree_successor(g_bitmap, i), NIL);
	}
	TEST_ASSERT_EQUAL_INT(vEB_tree_successor(g_bitmap, -1), NIL);
	TEST_ASSERT_EQUAL_INT(vEB_tree_successor(g_bitmap, 0x1 << 10), NIL);
}

void test_vEB_delete(void)
{
	vEB_tree_delete(g_bitmap, 0);
	for (int i = 0; i < 10; i++) {
		vEB_tree_delete(g_bitmap, 0x1 << i);
	}

	for (int i = 0; i < (0x1 << 10); i++) {
		TEST_ASSERT_EQUAL_INT(vEB_tree_member(g_bitmap, i), 0);
	}
}

void test_vEB_predecessor(void)
{
	for (int i = 10; i > 0; i--) {
		int j = (i == 10 ? (0x1 << i) - 1 : 0x1 << i);
		while (j > (0x1 << (i - 1))) {
			TEST_ASSERT_EQUAL_INT(vEB_tree_predecessor(g_bitmap, j),
					      0x1 << (i - 1));
			j--;
		}
	}
	TEST_ASSERT_EQUAL_INT(vEB_tree_predecessor(g_bitmap, 0), NIL);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_vEB_init);
	RUN_TEST(test_vEB_member);
	RUN_TEST(test_vEB_successor);
	RUN_TEST(test_vEB_delete);
	RUN_TEST(test_vEB_predecessor);
	return UNITY_END();
}
