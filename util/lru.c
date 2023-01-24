/**
 * @file lru.c
 * @brief implementation of the lru cache
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-09-30
 */

#include <stdlib.h>
#include <errno.h>
#include <inttypes.h>
#include <assert.h>

#include "log.h"
#include "lru.h"

/**
 * @brief initialize the LRU cache data strcture
 *
 * @param capacity number of the entries to insert the LRU
 * @param deallocate deallcation function for LRU's value
 *
 * @return initialized LRU cache data structure pointer
 */
struct lru_cache *lru_init(const size_t capacity, lru_dealloc_fn deallocate)
{
	struct lru_cache *cache = NULL;
	if (capacity == 0) {
		pr_err("capacity is zero\n");
		goto exception;
	}

	cache = (struct lru_cache *)malloc(sizeof(struct lru_cache));
	if (cache == NULL) {
		pr_err("memory allocation failed\n");
		goto exception;
	}

	cache->head = &cache->nil;
	cache->deallocate = deallocate;
	cache->capacity = capacity;
	cache->size = 0;

	cache->nil.next = &cache->nil;
	cache->nil.prev = &cache->nil;
	cache->nil.key = (uint64_t)-1;
	cache->nil.value = (uintptr_t)NULL;

	return cache;
exception:
	lru_free(cache);
	return NULL;
}

/**
 * @brief allocate the single node
 *
 * @param key key for identify the node
 * @param value value for data in the node
 *
 * @return allocated node structure pointer
 */
static struct lru_node *lru_alloc_node(const uint64_t key, uintptr_t value)
{
	struct lru_node *node;
	node = (struct lru_node *)malloc(sizeof(struct lru_node));
	if (node == NULL) {
		pr_err("node allocation failed\n");
		return NULL;
	}
	node->key = key;
	node->value = value;
	node->Dirty_Bit = 0;//This
	return node;
}

/**
 * @brief deallcate the allocated node
 *
 * @param node node which wants to deallocate
 */
static void lru_dealloc_node(struct lru_node *node)
{
	assert(NULL != node);
#if 0
	pr_debug("deallcate the node (key: %ld, value: %ld)\n", node->key,
		 node->value); /**< Not recommend to print this line */
#endif
	free(node);
}

/**
 * @brief delete the node from the list
 *
 * @param head list's head pointer
 * @param deleted target which wants to delete
 *
 * @return 0 for successfully delete, -EINVAL means there is no data in list
 *
 * @note
 * This function doesn't deallocate the node and its data
 */
static int lru_delete_node(struct lru_node *head, struct lru_node *deleted)
{
	assert(NULL != head);
	assert(NULL != deleted);

	if (head == deleted) {
		pr_debug(
			"deletion of head node is not permitted (node:%p, deleted:%p)\n",
			head, deleted);
		return -EINVAL;
	}
	deleted->prev->next = deleted->next;
	deleted->next->prev = deleted->prev;
	return 0;
}

/**
 * @brief implementation of the LRU eviction function
 *
 * @param cache LRU cache data structure pointer
 *
 * @return 0 for successfully evict
 */
static int __lru_do_evict(struct lru_cache *cache)
{
	struct lru_node *head = cache->head;
	struct lru_node *target = head->prev;
	int ret = 0;
	lru_delete_node(head, target);
	if (cache->deallocate) {
		ret = cache->deallocate(target->key, target->value, &(target->Dirty_Bit));
	}
	lru_dealloc_node(target);
	return ret;
}

/**
 * @brief interfaces for execute the eviction process
 *
 * @param cache LRU cache data structure pointer
 * @param nr_evict number of the entries to evict
 *
 * @return 0 for successfully evict
 */
static int lru_do_evict(struct lru_cache *cache, const uint64_t nr_evict)
{
	int ret = 0;
	uint64_t i;
	for (i = 0; i < nr_evict; i++) {
		ret = __lru_do_evict(cache);
		if (ret) {
			return ret;
		}
		cache->size -= 1;
	}
	return ret;
}

/**
 * @brief insert the lru node to the list
 *
 * @param node pointer of the node, node->next will indicate the newnode
 * @param newnode newly allocated node to insert
 */
static void lru_node_insert(struct lru_node *node, struct lru_node *newnode)
{
	assert(NULL != newnode);
	assert(NULL != node);

	newnode->prev = node;
	newnode->next = node->next;
	node->next->prev = newnode;
	node->next = newnode;
}

/**
 * @brief inser the key, value to the LRU cache
 *
 * @param cache LRU cache data structure pointer
 * @param key key which identifies the node
 * @param value value which contains the data
 *
 * @return 0 to success
 */
int lru_put(struct lru_cache *cache, const uint64_t key, uintptr_t value)
{
	struct lru_node *head = cache->head;
	struct lru_node *node = NULL;
	assert(NULL != head);

	if (cache->size >= cache->capacity) {
		pr_debug("eviction is called (size: %zu, cap: %zu)\n",
			 cache->size, cache->capacity);
		lru_do_evict(cache, lru_get_evict_size(cache));
	}

	node = lru_alloc_node(key, value);
	if (node == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	lru_node_insert(head, node);
	cache->size += 1;
	return 0;
}

/**
 * @brief find the node based on the key
 *
 * @param cache LRU cache data structure pointer
 * @param key key which identifies the node
 *
 * @return pointer of the node
 */
static struct lru_node *lru_find_node(struct lru_cache *cache,
				      const uint64_t key)
{
	struct lru_node *head = cache->head;
	struct lru_node *it = head->next;
	while (it != head) {
		if (it->key == key) {
			return it;
		}
		it = it->next;
	}
	return NULL;
}

/**
 * @brief get data from the LRU cache
 *
 * @param cache LRU cache data structrue pointer
 * @param key key which identifies the node
 *
 * @return data in the node's value 
 */
uintptr_t lru_get(struct lru_cache *cache, const uint64_t key)
{
	struct lru_node *head = cache->head;
	struct lru_node *node;

	uintptr_t value = (uintptr_t)NULL;

	node = lru_find_node(cache, key);
	if (node) {
		lru_delete_node(head, node);
		lru_node_insert(head, node);
		value = node->value;
	}
	return value;
}

/**
 * @brief deallocate the LRU cache structure
 *
 * @param cache LRU cache data structure pointer
 *
 * @return 0 to success
 */
int lru_free(struct lru_cache *cache)
{
	int ret = 0;
	struct lru_node *head = NULL;
	struct lru_node *node = NULL;
	struct lru_node *next = NULL;
	if (!cache) {
		return ret;
	}
	head = cache->head;
	node = head->next;
	while (node != head) {
		assert(NULL != node);
		next = node->next;
		if (cache->deallocate) {
			ret = cache->deallocate(node->key, node->value, &(node->Dirty_Bit));
			if (ret) {
				pr_err("deallocate failed (key: %" PRIu64
				       ", value: %" PRIuPTR ")\n",
				       node->key, node->value);
				return ret;
			}
		}
		free(node);
		node = next;
	}
	free(cache);
	return ret;
}
//This
uintptr_t lru_get_n_get_node(struct lru_cache *cache, const uint64_t key, struct lru_node **ret_node)
{
	struct lru_node *head = cache->head;
	struct lru_node *node;

	uintptr_t value = (uintptr_t)NULL;
	(void)ret_node;
	
	node = lru_find_node(cache, key);
	if (node) {//원래자리에서 뜯어와서 맨 앞(head바로 뒤)으로 옮기기
		lru_delete_node(head, node);
		lru_node_insert(head, node);
		value = node->value;
		*ret_node = node;
	}
	return value;
}

int lru_put_n_get_node(struct lru_cache *cache, const uint64_t key, uintptr_t value, struct lru_node**ret_node)
{
	struct lru_node *head = cache->head;
	struct lru_node *node = NULL;
	assert(NULL != head);
	if (cache->size >= cache->capacity) {//cache full
		pr_debug("eviction is called (size: %zu, cap: %zu)\n", cache->size, cache->capacity);
		lru_do_evict(cache, lru_get_evict_size(cache));//cache 전체의 약 30% 제거 ->> Write to disk 해야해
	}
	node = lru_alloc_node(key, value);
	if (node == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	lru_node_insert(head, node);
	cache->size += 1;
	*ret_node = node;
	return 0;
}
