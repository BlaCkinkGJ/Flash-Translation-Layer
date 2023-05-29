/**
 * @file device.h
 * @brief contain the device information header
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-01
 */
#ifndef DEVICE_H
#define DEVICE_H

#include <glib.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>

#include "log.h" /**< to use the TOSTRING */

#define PADDR_EMPTY ((uint32_t)UINT32_MAX)

struct device_request;
struct device_operations;

#ifndef DEVICE_PAGE_SIZE
#define DEVICE_PAGE_SIZE (8192)
#endif

/**
 * @brief request allocation flags
 */
enum {
	DEVICE_DEFAULT_REQUEST = 0,
};

/**
 * @brief flash board I/O direction
 */
enum {
	DEVICE_WRITE = 0 /**< write flag */,
	DEVICE_READ /**< read flag */,
	DEVICE_ERASE /**< erase flag */,
};

/**
 * @brief support module list
 */
enum {
	RAMDISK_MODULE = 0 /**< select the ramdisk module */,
	BLUEDBM_MODULE /**< select the bluedbm module */,
	ZONE_MODULE /**< select the zone module */,
	RASPBERRY_MODULE /**< select the raspberry module */,
};

/**
 * @brief device address information
 *
 * @note
 * If you want to use the ZONE_MODULE, you must change the
 * DEVICE_NR_PAGES_BITS and DEVICE_NR_BLOCKS_BITS based on the zone's
 * size.
 */
#ifndef DEVICE_NR_BUS_BITS
#define DEVICE_NR_BUS_BITS (3)
#endif

#ifndef DEVICE_NR_CHIPS_BITS
#define DEVICE_NR_CHIPS_BITS (3)
#endif

#ifndef DEVICE_NR_PAGES_BITS
#define DEVICE_NR_PAGES_BITS (7)
#endif

#ifndef DEVICE_NR_BLOCKS_BITS
#define DEVICE_NR_BLOCKS_BITS (19)
#endif

#if 0
#pragma message("DEVICE_NR_BUS_BITS = " TOSTRING(DEVICE_NR_BUS_BITS))
#pragma message("DEVICE_NR_CHIPS_BITS = " TOSTRING(DEVICE_NR_CHIPS_BITS))
#pragma message("DEVICE_NR_PAGES_BITS = " TOSTRING(DEVICE_NR_PAGES_BITS))
#pragma message("DEVICE_NR_BLOCKS_BITS = " TOSTRING(DEVICE_NR_BLOCKS_BITS))
#endif

/**
 * @brief I/O end request function
 *
 * @param request device request structure's pointer
 *
 * @note
 * You must specify the call routine of this function in your custom device
 * module
 */
typedef void (*device_end_req_fn)(struct device_request *);

/**
 * @brief generic device address format
 *
 * @note
 * `seqnum` is used for distinguish the each host pages in a device page
 */
struct device_address {
	union {
		struct {
			uint32_t bus : DEVICE_NR_BUS_BITS;
			uint32_t chip : DEVICE_NR_CHIPS_BITS;
			uint32_t page : DEVICE_NR_PAGES_BITS;
			uint32_t block : DEVICE_NR_BLOCKS_BITS;
		} format;
		struct {
			uint32_t bus : DEVICE_NR_BUS_BITS;
			uint32_t chip : DEVICE_NR_CHIPS_BITS;
			uint32_t page : DEVICE_NR_PAGES_BITS;
			uint32_t block : DEVICE_NR_BLOCKS_BITS;
		} raspberry_converter;
		struct {
			uint32_t page
				: (DEVICE_NR_PAGES_BITS + DEVICE_NR_CHIPS_BITS +
				   DEVICE_NR_BUS_BITS);
			uint32_t block : 32 -
				(DEVICE_NR_PAGES_BITS + DEVICE_NR_CHIPS_BITS +
				 DEVICE_NR_BUS_BITS);
		} raspberry;
		uint32_t lpn;
	};
};

/**
 * @brief request for device
 */
struct device_request {
	unsigned int flag; /**< flag describes the bio's direction */

	size_t data_len; /**< data length (bytes) */
	size_t sector; /**< sector cursor (bytes) */
	struct device_address paddr; /**< this contains the ppa */

	void *data; /**< pointer of the data */
	device_end_req_fn end_rq; /**< end request function */

	gint is_finish;

	pthread_mutex_t mutex;
	pthread_cond_t cond;

	void *rq_private; /**< contain the request's private data */
};

/**
 * @brief flash board's page information
 */
struct device_page {
	size_t size; /**< byte */
};

/**
 * @brief flash board's block information
 */
struct device_block {
	struct device_page page;
	size_t nr_pages;
};

/**
 * @brief flash board's package(nand chip) information
 */
struct device_package {
	struct device_block block;
	size_t nr_blocks;
};

/**
 * @brief flash board's architecture information
 */
struct device_info {
	struct device_package package;
	size_t nr_bus; /**< bus equal to channel */
	size_t nr_chips; /**< chip equal to way */
};

/**
 * @brief metadata of the device
 */
struct device {
	pthread_mutex_t mutex;
	const struct device_operations *d_op;
	struct device_info info;
	uint64_t *badseg_bitmap;
	void *d_private; /**< generally contain the sub-layer's data structure */
	int (*d_submodule_exit)(struct device *);
};

/**
 * @brief operations for device
 */
struct device_operations {
	int (*open)(struct device *, const char *name, int flags);
	ssize_t (*write)(struct device *, struct device_request *);
	ssize_t (*read)(struct device *, struct device_request *);
	int (*erase)(struct device *, struct device_request *);
	int (*close)(struct device *);
};

struct device_request *device_alloc_request(uint64_t flags);
void device_free_request(struct device_request *);

int device_module_init(const uint64_t modnum, struct device **, uint64_t flags);
int device_module_exit(struct device *);

/**
 * @brief get the number of segments in a flash board
 *
 * @param dev device structure pointer
 *
 * @return the number of segments in a flash board
 */
static inline size_t device_get_nr_segments(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	return package->nr_blocks;
}

static inline size_t device_get_blocks_per_segment(struct device *dev)
{
	struct device_info *info = &dev->info;
	return (info->nr_bus * info->nr_chips);
}

/**
 * @brief get the number of pages in a segment
 *
 * @param dev device structure pointer
 *
 * @return the number of pages in a segment
 */
static inline size_t device_get_pages_per_segment(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;

	return device_get_blocks_per_segment(dev) * block->nr_pages;
}

/**
 * @brief get flash board's NAND page size
 *
 * @param dev device structure pointer
 *
 * @return NAND page size (generally, 8192 or 4096)
 */
static inline size_t device_get_page_size(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;
	return page->size;
}

/**
 * @brief total size of a flash board
 *
 * @param dev device structure pointer
 *
 * @return flash board's total size (byte)
 */
static inline size_t device_get_total_size(struct device *dev)
{
	size_t nr_segments = device_get_nr_segments(dev);
	size_t nr_pages_per_segment = device_get_pages_per_segment(dev);
	size_t page_size = device_get_page_size(dev);

	return nr_segments * nr_pages_per_segment * page_size;
}

/**
 * @brief get total the number of pages in a flash board
 *
 * @param dev device structure pointer
 *
 * @return the number of pages in a flash board.
 */
static inline size_t device_get_total_pages(struct device *dev)
{
	return device_get_total_size(dev) / device_get_page_size(dev);
}

#endif
