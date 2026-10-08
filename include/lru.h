/**
 * @file lru.h
 * @brief data structures and interfaces for the lru cache
 * @author Gijun Oh
 * @version 0.1
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

// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#include "log.h"

/**
 * @brief deallocate the value for eviction function type. 0 means successfully evicted
 */
typedef int (*lru_dealloc_fn)(const uint64_t, uintptr_t);

/**
 * @brief doubly-linked list data structure
 */
struct lru_node {
	uint64_t key;
	uintptr_t value;
	struct lru_node *next;
	struct lru_node *prev;
};

/**
 * @brief main LRU cache data structure
 */
struct lru_cache {
	size_t capacity; /**< total number of the lru_node */
	size_t size; /**< current number of the lru_node */
	struct lru_node *head;
	lru_dealloc_fn deallocate;
	struct lru_node nil; /**< don't access this directly */
};

/*
 * Guard the shared layout: `src/lru.rs` declares both structs as
 * `#[repr(C)]` and asserts these numbers in its `layout_matches_c` test.
 * `lru_get_evict_size` below and `test/lru-test.c` read `capacity` / `size`
 * straight out of the struct, so the offsets are part of the ABI.
 *
 * The Rust `lru_cache` carries a Rust-only field after `nil`, so its size is
 * larger than this one: never `sizeof(struct lru_cache)` for allocation, only
 * `lru_init` ever did.
 */
#if defined(__cplusplus)
#define LRU_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#else
#define LRU_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

LRU_STATIC_ASSERT(offsetof(struct lru_node, key) == 0,
		  "lru_node.key must be first");
LRU_STATIC_ASSERT(offsetof(struct lru_node, value) == sizeof(uint64_t),
		  "lru_node.value must follow key");
LRU_STATIC_ASSERT(offsetof(struct lru_node, next) ==
			  sizeof(uint64_t) + sizeof(uintptr_t),
		  "lru_node.next must follow value");
LRU_STATIC_ASSERT(offsetof(struct lru_node, prev) ==
			  sizeof(uint64_t) + 2 * sizeof(uintptr_t),
		  "lru_node.prev must follow next");
LRU_STATIC_ASSERT(sizeof(struct lru_node) ==
			  sizeof(uint64_t) + 3 * sizeof(uintptr_t),
		  "lru_node must be key plus three words");

LRU_STATIC_ASSERT(offsetof(struct lru_cache, capacity) == 0,
		  "lru_cache.capacity must be first");
LRU_STATIC_ASSERT(offsetof(struct lru_cache, size) == sizeof(size_t),
		  "lru_cache.size must follow capacity");
LRU_STATIC_ASSERT(offsetof(struct lru_cache, head) == 2 * sizeof(size_t),
		  "lru_cache.head must follow size");
LRU_STATIC_ASSERT(offsetof(struct lru_cache, deallocate) ==
			  3 * sizeof(size_t),
		  "lru_cache.deallocate must follow head");
LRU_STATIC_ASSERT(offsetof(struct lru_cache, nil) == 4 * sizeof(size_t),
		  "lru_cache.nil must follow deallocate");

#undef LRU_STATIC_ASSERT

struct lru_cache *lru_init(const size_t capacity, lru_dealloc_fn deallocate);
int lru_put(struct lru_cache *cache, const uint64_t key, uintptr_t value);
uintptr_t lru_get(struct lru_cache *cache, const uint64_t key);
int lru_free(struct lru_cache *cache);

/**
 * @brief get evict size of the LRU cache
 *
 * @param cache LRU cache structrue pointer
 *
 * @return number of the eviction entries
 *
 * @note
 * Default LRU cache's eviction size is 30% of its capacity
 */
static inline size_t lru_get_evict_size(struct lru_cache *cache)
{
	pr_debug("evict size ==> %zu\n", (size_t)(cache->capacity));
	return cache->capacity;
}

#ifdef __cplusplus
}
#endif

#endif
