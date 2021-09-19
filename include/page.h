#ifndef PAGE_H
#define PAGE_H

#include <stdint.h>
#include <pthread.h>
#ifdef __cplusplus
#include <atomic>
#endif

#include "flash.h"
#include "van-emde-boas.h"

/** follow the linux kernel's value */
#define SECTOR_SHIFT (9)

/**
 * @brief flash board information
 */
enum { FLASH_NR_SEGMENT = 4096,
       FLASH_PAGES_PER_BLOCK = 128,
       FLASH_PAGE_SIZE = 8192,
       FLASH_TOTAL_CHIPS = 8,
       FLASH_TOTAL_BUS = 8,
       FLASH_BLOCKS_PER_SEGMENT = FLASH_TOTAL_CHIPS * FLASH_TOTAL_BUS,
};

/**
 * @brief flash board I/O direction
 */
enum { FLASH_FTL_WRITE = 0,
       FLASH_FTL_READ,
       FLASH_FTL_ERASE,
};

/** total flash board size */
#define FLASH_DISK_SIZE                                                        \
	(FLASH_NR_SEGMENT * FLASH_PAGES_PER_BLOCK * FLASH_TOTAL_CHIPS *        \
	 FLASH_TOTAL_BUS)
/** mapping table size */
#define FLASH_MAP_SIZE (FLASH_DISK_SIZE >> SECTOR_SHIFT)

/**
 * @brief segment information structure
 * @note
 * segment number is same as block number
 */
struct page_ftl_segment {
#ifdef __cplusplus
	std::atomic<uint64_t> nr_invalid_blocks;
#else
	_Atomic uint64_t nr_invalid_blocks;
#endif
	struct vEB *valid_bits;
};

/**
 * @brief contain the page flash translation layer information
 */
struct page_ftl {
	uint32_t trans_map[FLASH_MAP_SIZE]; /**< page-level mapping table */
	struct page_ftl_segment segment[FLASH_NR_SEGMENT];
	pthread_mutex_t mutex;
};

/**
 * @brief request for page flash translation layer
 */
struct page_ftl_request {
	unsigned int flag; /**< flag describes the bio's direction */

	size_t data_len; /**< data length (bytes) */
	uint64_t sector; /**< sector cursor (divide by sector size(1 << SECTOR_SHIFT bytes)) */

	const void *data; /**< pointer of the data */
};

int page_ftl_open(struct page_ftl *);
int page_ftl_close(struct page_ftl *);

ssize_t page_ftl_submit_request(struct page_ftl *, struct page_ftl_request *);
ssize_t page_ftl_write(struct page_ftl *, struct page_ftl_request *);
ssize_t page_ftl_read(struct page_ftl *, struct page_ftl_request *);

int page_ftl_module_init(struct flash_device *, uint64_t flags);
int page_ftl_module_exit(struct flash_device *);

#ifdef __cplusplus
template <typename T>
/**
 * @brief compatible function for stdbool.h
 *
 */
static inline void atomic_store(std::atomic<T> *v, int arg)
{
	*v = arg;
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T> static inline T atomic_load(std::atomic<T> *v)
{
	return *v;
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T>
static inline void atomic_fetch_add(std::atomic<T> *v, int arg)
{
	std::atomic_fetch_add(v, static_cast<T>(arg));
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T>
static inline void atomic_fetch_sub(std::atomic<T> *v, int arg)
{
	std::atomic_fetch_sub(v, static_cast<T>(arg));
}
#endif

#endif
