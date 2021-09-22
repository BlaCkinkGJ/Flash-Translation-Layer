/**
 * @file lru.h
 * @brief data structures and interfaces for the lru cache
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-30
 * @note
 * This is not thread-safe.
 */
#ifndef LRU_H
#define LRU_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>
#include <stdlib.h>

#include "log.h"

/**
 * @brief deallocate the value for eviction function type. 0 means successfully evicted
 */
typedef int (*lru_dealloc_fn)(const uint64_t, uintptr_t);

struct lru_node {
	uint64_t key;
	uintptr_t value;
	struct lru_node *next;
	struct lru_node *prev;
};

struct lru_cache {
	size_t capacity; /**< total number of the lru_node */
	size_t size; /**< current number of the lru_node */
	struct lru_node *head;
	lru_dealloc_fn deallocate;
	struct lru_node nil; /**< don't access this directly */
};

struct lru_cache *lru_init(const size_t capacity, lru_dealloc_fn deallocate);
int lru_put(struct lru_cache *cache, const uint64_t key, uintptr_t value);
uintptr_t lru_get(struct lru_cache *cache, const uint64_t key);
int lru_free(struct lru_cache *cache);

static inline size_t lru_get_evict_size(struct lru_cache *cache)
{
	pr_debug("evict size ==> %ld\n", (size_t)(cache->capacity * 3 / 10));
	return cache->capacity * 3 / 10;
}

#ifdef __cplusplus
}
#endif

#endif
