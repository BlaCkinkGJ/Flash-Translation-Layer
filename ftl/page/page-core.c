/**
 * @file page-core.c
 * @brief core logic for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <errno.h>
#ifdef __cplusplus
#include <atomic>
#else
#include <stdatomic.h>
#endif

#include <assert.h>
#include <string.h>

#include "include/page.h"
#include "include/log.h"
#include "include/bits.h"

/**
 * @brief initialize each segment's metadata
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return 0 to success, negative value to fail
 */
static int page_ftl_init_segment(struct page_ftl *pgftl)
{
	int compensator;
	size_t nr_segments;
	size_t nr_pages_per_segment;
	size_t page_size;

	struct page_ftl_segment *segments;

	assert(0 < PAGE_SIZE);

	nr_segments = device_get_nr_segments(pgftl->dev);
	nr_pages_per_segment = device_get_pages_per_segment(pgftl->dev);
	page_size = device_get_page_size(pgftl->dev);

	compensator = page_size / PAGE_SIZE;
	if (page_size < PAGE_SIZE) {
		pr_err("flash page size must larger than host page size (%zu >= %d)\n",
		       page_size, PAGE_SIZE);
		return -EINVAL;
	}

	segments = (struct page_ftl_segment *)malloc(
		sizeof(struct page_ftl_segment) * nr_segments);
	if (segments == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	for (size_t i = 0; i < nr_segments; i++) {
		segments[i].valid_bits = NULL;
	}
	for (size_t i = 0; i < nr_segments; i++) {
		uint64_t *valid_bits;
		valid_bits = (uint64_t *)malloc(
			BITS_TO_BYTES(nr_pages_per_segment * compensator));
		if (valid_bits == NULL) {
			pr_err("allocated failed(seq:%zu)", i);
			return -ENOMEM;
		}
		memset(valid_bits, 0,
		       BITS_TO_BYTES(nr_pages_per_segment * compensator));
		segments[i].valid_bits = valid_bits;
		atomic_store(&segments[i].nr_invalid_blocks, 0);
		pr_debug(
			"initialize the segment %zu (bits: %zu * %d, size: %lu)\n",
			i, nr_pages_per_segment, compensator,
			(uint64_t)(nr_pages_per_segment * compensator) / 8);
	}

	pgftl->segments = segments;
	return 0;
}

/**
 * @brief deallocate the page ftl's cache
 *
 * @param cache pointer of the cache
 *
 * @return deallocate status of the cache
 */
static int page_ftl_free_cache(struct page_ftl_cache *cache)
{
	int ret = 0;
	assert(NULL != cache);
	if (cache->lru) {
		ret = lru_free(cache->lru);
		cache->lru = NULL;
	}
	if (cache->free_block_bits) {
		free(cache->free_block_bits);
		cache->free_block_bits = NULL;
	}

	return ret;
}

/**
 * @brief initialize the cache data structure
 *
 * @param pgftl pointer of the page FTL's data structure
 *
 * @return 0 to success, -EINVAL to fail
 */
static int page_ftl_init_cache(struct page_ftl *pgftl)
{
	int ret = 0;
	struct page_ftl_cache *cache;
	uint64_t *free_block_bits = NULL;
	struct lru_cache *lru = NULL;

	cache = (struct page_ftl_cache *)malloc(sizeof(struct page_ftl_cache));
	if (cache == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}

	pthread_mutex_init(&cache->mutex, NULL);

	free_block_bits =
		(uint64_t *)malloc(BITS_TO_BYTES(PAGE_FTL_NR_CACHE_BLOCK));
	if (free_block_bits == NULL) {
		pr_err("free block bitmap construction failed (size: %lu)\n",
		       (uint64_t)PAGE_FTL_NR_CACHE_BLOCK);
		ret = -EINVAL;
		goto exception;
	}
	memset(free_block_bits, 0, BITS_TO_BYTES(PAGE_FTL_NR_CACHE_BLOCK));
	cache->free_block_bits = free_block_bits;

	lru = lru_init(PAGE_FTL_NR_CACHE_BLOCK, NULL);
	if (lru == NULL) {
		pr_err("creation of the LRU cache failed (size: %lu)\n",
		       (uint64_t)PAGE_FTL_NR_CACHE_BLOCK);
		ret = -EINVAL;
		goto exception;
	}
	cache->lru = lru;

	memset(cache->buffer, 0, sizeof(cache->buffer));

	pgftl->cache = cache;

	return 0;
exception:
	if (cache != NULL) {
		page_ftl_free_cache(cache);
	}
	return ret;
}

/**
 * @brief allocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 */
int page_ftl_open(struct page_ftl *pgftl)
{
	int err;
	size_t map_size;

	struct device *dev;

	assert(NULL != pgftl->dev);

	pthread_mutex_init(&pgftl->mutex, NULL);

	dev = pgftl->dev;
	err = dev->d_op->open(dev);
	if (err) {
		pr_err("device open failed\n");
		err = -EINVAL;
		goto exception;
	}

	map_size = page_ftl_get_map_size(pgftl);
	pgftl->trans_map = (uint32_t *)malloc(map_size);
	if (pgftl->trans_map == NULL) {
		pr_err("cannot allocate the memory for mapping table\n");
		goto exception;
	}
	/** initialize the mapping table */
	for (uint32_t lpn = 0; lpn < map_size / sizeof(uint32_t); lpn++) {
		pgftl->trans_map[lpn] = PADDR_EMPTY;
	}

	err = page_ftl_init_segment(pgftl);
	if (err) {
		goto exception;
	}

	err = page_ftl_init_cache(pgftl);
	if (err) {
		goto exception;
	}

	return 0;

exception:
	page_ftl_close(pgftl);
	return err;
}

/**
 * @brief submit the request to the valid function
 *
 * @param pgftl pointer of the page ftl structure
 * @param request pointer of the request
 *
 * @return read and write return the size of the submit,
 * fail to return the nugative value
 */
ssize_t page_ftl_submit_request(struct page_ftl *pgftl,
				struct device_request *request)
{
	if (pgftl == NULL || request == NULL) {
		pr_err("null detected (pgftl:%p, request:%p)\n", pgftl,
		       request);
		return -EINVAL;
	}
	switch (request->flag) {
	case DEVICE_WRITE:
		return page_ftl_write(pgftl, request);
	case DEVICE_ERASE:
		return page_ftl_read(pgftl, request);
	default:
		pr_err("invalid flag detected: %u\n", request->flag);
		return -EINVAL;
	}
}

/**
 * @brief deallocate the ftl's segments
 *
 * @param segments pointer of the segment array
 */
static void page_ftl_free_segments(struct page_ftl *pgftl)
{
	struct page_ftl_segment *segments = pgftl->segments;
	size_t nr_segments;
	size_t i;
	assert(NULL != segments);
	nr_segments = device_get_nr_segments(pgftl->dev);
	for (i = 0; i < nr_segments; i++) {
		uint64_t *valid_bits;
		valid_bits = segments[i].valid_bits;
		if (valid_bits == NULL) {
			continue;
		}
		free(valid_bits);
		segments[i].valid_bits = NULL;
	}
}

/**
 * @brief deallocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 */
int page_ftl_close(struct page_ftl *pgftl)
{
	int ret = 0;
	if (pgftl == NULL) {
		pr_err("null page ftl structure submitted\n");
		return ret;
	}

	if (pgftl->segments) {
		page_ftl_free_segments(pgftl);
		free(pgftl->segments);
		pgftl->segments = NULL;
	}

	if (pgftl->cache) {
		ret = page_ftl_free_cache(pgftl->cache);
		free(pgftl->cache);
		pgftl->cache = NULL;
	}

	if (pgftl->trans_map) {
		free(pgftl->trans_map);
		pgftl->trans_map = NULL;
	}

	if (pgftl->dev && pgftl->dev->d_op) {
		struct device *dev = pgftl->dev;
		ret = dev->d_op->close(dev);
	}
	return ret;
}
