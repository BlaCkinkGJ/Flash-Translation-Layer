/**
 * @file page.h
 * @brief declaration of data structures and macros for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#ifndef PAGE_H
#define PAGE_H

#include <stdint.h>
#include <pthread.h>

#include <limits.h>
#include <unistd.h>

#include "flash.h"
#include "atomic.h"
#include "lru.h"
#include "device.h"

#define PAGE_SIZE (_SC_PAGE_SIZE)
#define PAGE_FTL_NR_CACHE_BLOCK (2)
#define PADDR_EMPTY (UINT32_MAX)

/**
 * @brief hold the metadata and buffer of a cache
 * @note
 * You must lock the mutex when you access the metadata.
 * Do not access the `buffer` directly.
 */
struct page_ftl_cache {
	pthread_mutex_t mutex;
	uint64_t *free_block_bits;
	struct lru_cache *lru;
	char *buffer[PAGE_FTL_NR_CACHE_BLOCK];
};

/**
 * @brief segment information structure
 * @note
 * Segment number is same as block number
 */
struct page_ftl_segment {
	atomic64_t nr_invalid_blocks;
	uint64_t *valid_bits;
};

/**
 * @brief contain the page flash translation layer information
 */
struct page_ftl {
	uint32_t *trans_map; /**< page-level mapping table */
	struct page_ftl_cache *cache;
	struct page_ftl_segment *segments;
	struct device *dev;
	pthread_mutex_t mutex;
};

int page_ftl_open(struct page_ftl *);
int page_ftl_close(struct page_ftl *);

ssize_t page_ftl_submit_request(struct page_ftl *, struct device_request *);
ssize_t page_ftl_write(struct page_ftl *, struct device_request *);
ssize_t page_ftl_read(struct page_ftl *, struct device_request *);

int page_ftl_module_init(struct flash_device *, uint64_t flags);
int page_ftl_module_exit(struct flash_device *);

static inline size_t page_ftl_get_map_size(struct page_ftl *pgftl)
{
	struct device *dev = pgftl->dev;
	return ((device_get_total_size(dev) / PAGE_SIZE) + 1) *
	       sizeof(uint32_t);
}

#endif
