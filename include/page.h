/**
 * @file page.h
 * @brief declaration of data structures and macros for page ftl
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-09-22
 */
#ifndef PAGE_H
#define PAGE_H

#include <stdint.h>
#include <pthread.h>

#include <limits.h>
#include <unistd.h>

#include <glib.h>

#include "flash.h"
#include "device.h"

#define PAGE_FTL_CACHE_SIZE (2)
#define PAGE_FTL_GC_RATIO                                                      \
	((double)30 /                                                          \
	 100) /**< maximum the number of segments garbage collected */
#define PAGE_FTL_GC_THRESHOLD                                                  \
	((double)60 /                                                          \
	 100) /**< gc triggered when number of the free pages under threshold */

enum { PAGE_FTL_IOCTL_TRIM = 0,
};

/**
 * @brief segment information structure
 * @note
 * Segment number is same as block number
 */
struct page_ftl_segment {
	gint nr_free_pages;
	gint nr_valid_pages;
	gint is_gc;

	uint64_t *use_bits; /**< contain the use page information */
	GList *lpn_list; /**< lba_list which contains the valid data */
};

/**
 * @brief contain the page flash translation layer information
 */
struct page_ftl {
	uint32_t *trans_map; /**< page-level mapping table */
	uint64_t alloc_segnum; /**< last allocated segment number */
	struct page_ftl_segment *segments;
	struct device *dev;
	pthread_spinlock_t mutex;
	pthread_rwlock_t gc_rwlock;
	pthread_rwlock_t *bus_rwlock;
#ifdef PAGE_FTL_USE_GLOBAL_RWLOCK
	pthread_rwlock_t rwlock;
#endif
	pthread_t gc_thread;
	int o_flags;

	GList *gc_list; /**< garbage collection target list */
	uint64_t *gc_seg_bits; /**< to find segnum is in gc list or not */
};

/* page-interface.c */
int page_ftl_open(struct page_ftl *, const char *name, int flags);
int page_ftl_close(struct page_ftl *);

ssize_t page_ftl_submit_request(struct page_ftl *, struct device_request *);
ssize_t page_ftl_write(struct page_ftl *, struct device_request *);
ssize_t page_ftl_read(struct page_ftl *, struct device_request *);

int page_ftl_module_init(struct flash_device *, uint64_t flags);
int page_ftl_module_exit(struct flash_device *);

/* page-map.c */
struct device_address page_ftl_get_free_page(struct page_ftl *);
int page_ftl_update_map(struct page_ftl *, size_t sector, uint32_t ppn);

/* page-core.c */
int page_ftl_segment_data_init(struct page_ftl *, struct page_ftl_segment *);

/* page-gc.c */
int page_ftl_do_gc(struct page_ftl *);

static inline size_t page_ftl_get_map_size(struct page_ftl *pgftl)
{
	struct device *dev = pgftl->dev;
	return ((device_get_total_size(dev) / device_get_page_size(dev)) + 1) *
	       sizeof(uint32_t);
}
static inline size_t page_ftl_get_lpn(struct page_ftl *pgftl, size_t sector)
{
	return sector / device_get_page_size(pgftl->dev);
}

static inline size_t page_ftl_get_page_offset(struct page_ftl *pgftl,
					      size_t sector)
{
	return sector % device_get_page_size(pgftl->dev);
}

static inline size_t page_ftl_get_segment_number(struct page_ftl *pgftl,
						 uintptr_t segment)
{
	return (segment - (uintptr_t)pgftl->segments) /
	       sizeof(struct page_ftl_segment);
}
#endif
