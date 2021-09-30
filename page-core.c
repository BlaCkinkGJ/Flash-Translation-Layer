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
	struct page_ftl_segment *segments = NULL;

	assert(0 < FLASH_HOST_PAGE_SIZE);
	assert(0 < FLASH_PAGE_SIZE);

	compensator = FLASH_PAGE_SIZE / FLASH_HOST_PAGE_SIZE;
	if (FLASH_PAGE_SIZE < FLASH_HOST_PAGE_SIZE) {
		pr_err("flash page size must larger than host page size (%d >= %d)\n",
		       FLASH_PAGE_SIZE, FLASH_HOST_PAGE_SIZE);
		return -EINVAL;
	}

	segments = (struct page_ftl_segment *)malloc(
		sizeof(struct page_ftl_segment) * FLASH_NR_SEGMENT);
	if (segments == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	for (uint64_t i = 0; i < FLASH_NR_SEGMENT; i++) {
		segments[i].valid_bits = NULL;
	}
	for (uint64_t i = 0; i < FLASH_NR_SEGMENT; i++) {
		uint64_t *valid_bits = NULL;
		valid_bits = (uint64_t *)malloc(
			BITS_TO_BYTES(FLASH_PAGES_PER_SEGMENT * compensator));
		if (valid_bits == NULL) {
			pr_err("allocated failed(seq:%lu)", i);
			return -ENOMEM;
		}
		memset(valid_bits, 0,
		       sizeof(BITS_TO_BYTES(FLASH_NR_CACHE_BLOCK)));
		segments[i].valid_bits = valid_bits;
		atomic_store(&segments[i].nr_invalid_blocks, 0);
		pr_debug(
			"initialize the segment %ld (bits: %d * %d, size: %ld)\n",
			i, FLASH_PAGES_PER_SEGMENT, compensator,
			(uint64_t)(FLASH_PAGES_PER_SEGMENT * compensator) / 8);
	}

	pgftl->segments = segments;
	return 0;
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
	struct page_ftl_cache *cache = NULL;
	uint64_t *free_block_bits = NULL;
	struct lru_cache *lru = NULL;

	cache = (struct page_ftl_cache *)malloc(sizeof(struct page_ftl_cache));
	if (cache == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}

	pthread_mutex_init(&cache->mutex, NULL);

	free_block_bits =
		(uint64_t *)malloc(BITS_TO_BYTES(FLASH_NR_CACHE_BLOCK));
	if (free_block_bits == NULL) {
		pr_err("van-emde-boas tree construction failed (size: %ld)\n",
		       (uint64_t)FLASH_NR_CACHE_BLOCK);
		return -EINVAL;
	}
	memset(free_block_bits, 0, sizeof(BITS_TO_BYTES(FLASH_NR_CACHE_BLOCK)));

	lru = lru_init(FLASH_NR_CACHE_BLOCK, NULL);
	if (lru == NULL) {
		pr_err("creation of the LRU cache failed (size: %ld)\n",
		       (uint64_t)FLASH_NR_SEGMENT);
		return -EINVAL;
	}

	cache->free_block_bits = free_block_bits;
	cache->lru = lru;

	memset(cache->buffer, 0, sizeof(cache->buffer));

	pgftl->cache = cache;

	return 0;
}

/**
 * @brief allocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 *
 * @note
 * Each segment's van-emde-boas tree size is
 * FLASH_BLOCKS_PER_SEGMENT * FLASH_PAGES_PER_BLOCK * (FLASH_PAGE_SIZE / FLASH_HOST_IO_SIZE).
 *
 * This means we adjust the page io size to the host io size. For example,
 * the Linux's normal io size is 4096 bytes. However, our flash boards's page size
 * is 8192 bytes (double of the Linux's io size). Therefore, we must bitmap to adjust
 * that size. As a result, we use the `compensator`.
 *
 * Van-emde-boas tree's overhead is following below. (adopt the default option)
 * 1. bits per describe segment = (8192(pages per segment) * 2(compensator))/8(bits per byte) = 2048
 * 2. segments per flash = 4096
 * 3. segments per flash * bits per describe segment = 4096 * 2048 = 8388608 = 8MiB
 *
 * Mapping table overhead is following below. (adopt the default option)
 * 1. flash board size = 4096(# of segments) * 8192(pages per segment) * 8192(page size) = 256GiB
 * 2. entries per mapping table = 256GiB(flash board size) / 4KiB(host page size) = 67108864
 * 3. mapping table size = 4(entry size) * 67108864(entries per mapping table) = 256MiB
 * 
 */
int page_ftl_open(struct page_ftl *pgftl)
{
	int err;

	pthread_mutex_init(&pgftl->mutex, NULL);

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
				struct page_ftl_request *request)
{
	if (pgftl == NULL || request == NULL) {
		pr_err("null detected (pgftl:%p, request:%p)\n", pgftl,
		       request);
		return -EINVAL;
	}
	switch (request->flag) {
	case FLASH_FTL_WRITE:
		return page_ftl_write(pgftl, request);
	case FLASH_FTL_READ:
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
static void page_ftl_free_segments(struct page_ftl_segment *segments)
{
	assert(NULL != segments);
	for (int i = 0; i < FLASH_NR_SEGMENT; i++) {
		uint64_t *valid_bits = NULL;
		valid_bits = segments[i].valid_bits;
		if (valid_bits == NULL) {
			continue;
		}
		free(valid_bits);
		segments[i].valid_bits = NULL;
	}
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
		page_ftl_free_segments(pgftl->segments);
		free(pgftl->segments);
		pgftl->segments = NULL;
	}

	if (pgftl->cache) {
		ret = page_ftl_free_cache(pgftl->cache);
		free(pgftl->cache);
		pgftl->cache = NULL;
	}
	return ret;
}
