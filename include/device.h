/**
 * @file device.h
 * @brief contain the device information header
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-10-01
 */
#ifndef DEVICE_H
#define DEVICE_H

#include <stdlib.h>
#include <stdint.h>

struct device_request;
struct device_operations;

/**
 * @brief flash board I/O direction
 */
enum { DEVICE_WRITE = 0,
       DEVICE_READ,
       DEVICE_ERASE,
};

enum { RAMDISK_MODULE = 0,
       BLUEDBM_MODULE,
};

typedef void (*device_end_req_fn)(struct device_request *);

struct device_address {
	union {
		struct {
			uint32_t bus : 3;
			uint32_t chip : 3;
			uint32_t page : 7;
			uint32_t block : 19;
		} format;
		uint32_t lpn;
	};
};

/**
 * @brief request for device
 */
struct device_request {
	unsigned int flag; /**< flag describes the bio's direction */

	size_t data_len; /**< data length (bytes) */
	uint64_t sector; /**< sector cursor (divide by sector size(1 << PAGE_SHIFT bytes)) */
	struct device_address paddr; /**< this contains the ppa */

	void *data; /**< pointer of the data */
	device_end_req_fn end_rq; /**< end request function */

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
	const struct device_operations *d_op;
	struct device_info info;
	void *d_private; /**< generally contain the sub-layer's data structure */
	int (*d_submodule_exit)(struct device *);
};

/**
 * @brief operations for device
 */
struct device_operations {
	int (*open)(struct device *);
	ssize_t (*write)(struct device *, struct device_request *);
	ssize_t (*read)(struct device *, struct device_request *);
	ssize_t (*erase)(struct device *, struct device_request *);
	int (*close)(struct device *);
};

int device_module_init(const uint64_t modnum, struct device **, uint64_t flags);
int device_module_exit(struct device *);

static inline size_t device_get_nr_segments(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	return package->nr_blocks;
}

static inline size_t device_get_pages_per_segment(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;

	return (info->nr_bus * info->nr_chips) * block->nr_pages;
}

static inline size_t device_get_page_size(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;
	return page->size;
}

static inline size_t device_get_total_size(struct device *dev)
{
	size_t nr_segments = device_get_nr_segments(dev);
	size_t nr_pages_per_segment = device_get_pages_per_segment(dev);
	size_t page_size = device_get_page_size(dev);

	return nr_segments * nr_pages_per_segment * page_size;
}

static inline size_t device_get_total_pages(struct device *dev)
{
	return device_get_total_size(dev) / device_get_page_size(dev);
}

#endif
