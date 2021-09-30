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

#include "flash.h"
#include "atomic.h"
#include "lru.h"

/** follow the linux kernel's value */
#define PAGE_SHIFT (12)

/** define empty mapping information */
#define PADDR_EMPTY (UINT_MAX)

/**
 * @brief flash board information
 *
 * @note
 * You MUST NOT think this information based on the
 * chip datasheet.
 */
enum { FLASH_NR_SEGMENT = 4096,
       FLASH_PAGES_PER_BLOCK = 128,
       FLASH_PAGE_SIZE = 8192,
       FLASH_TOTAL_CHIPS = 8,
       FLASH_TOTAL_BUS = 8,
};

/**
 * @brief flash board I/O direction
 */
enum { FLASH_FTL_WRITE = 0,
       FLASH_FTL_READ,
       FLASH_FTL_ERASE,
};

/** segment information */
#define FLASH_BLOCKS_PER_SEGMENT (FLASH_TOTAL_CHIPS * FLASH_TOTAL_BUS)
#define FLASH_PAGES_PER_SEGMENT                                                \
	(FLASH_BLOCKS_PER_SEGMENT * FLASH_PAGES_PER_BLOCK)
/** total flash board size (bytes) */
#define FLASH_DISK_SIZE                                                        \
	((uint64_t)FLASH_NR_SEGMENT * FLASH_PAGES_PER_SEGMENT * FLASH_PAGE_SIZE)
/** mapping table size (bytes) */
#define FLASH_MAP_SIZE (FLASH_DISK_SIZE >> PAGE_SHIFT)
/** host I/O size (bytes) */
#define FLASH_HOST_PAGE_SIZE (4096)
/** number of the cache block */
#define FLASH_NR_CACHE_BLOCK (1024)

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
	char buffer[FLASH_NR_CACHE_BLOCK][FLASH_HOST_PAGE_SIZE];
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
	uint32_t trans_map[FLASH_MAP_SIZE]; /**< page-level mapping table */
	struct page_ftl_cache *cache;
	struct page_ftl_segment *segments;
	pthread_mutex_t mutex;
};

/**
 * @brief request for page flash translation layer
 */
struct page_ftl_request {
	unsigned int flag; /**< flag describes the bio's direction */

	size_t data_len; /**< data length (bytes) */
	uint64_t sector; /**< sector cursor (divide by sector size(1 << PAGE_SHIFT bytes)) */

	const void *data; /**< pointer of the data */
};

int page_ftl_open(struct page_ftl *);
int page_ftl_close(struct page_ftl *);

ssize_t page_ftl_submit_request(struct page_ftl *, struct page_ftl_request *);
ssize_t page_ftl_write(struct page_ftl *, struct page_ftl_request *);
ssize_t page_ftl_read(struct page_ftl *, struct page_ftl_request *);

int page_ftl_module_init(struct flash_device *, uint64_t flags);
int page_ftl_module_exit(struct flash_device *);

#endif
