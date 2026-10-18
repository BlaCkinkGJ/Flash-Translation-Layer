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
#include <stddef.h>
#include <pthread.h>

#include <assert.h>
#include <limits.h>
#include <unistd.h>

#include "list.h"
#include "flash.h"
#include "device.h"

// #define PAGE_FTL_USE_CACHE
#define PAGE_FTL_CACHE_SIZE ((1 << 10))
#define PAGE_FTL_GC_RATIO                                                      \
	((double)10 /                                                          \
	 100) /**< maximum the number of segments garbage collected */
#define PAGE_FTL_GC_ALL ((double)1) /**< collect all dirty segments */
#define PAGE_FTL_GC_THRESHOLD                                                  \
	((double)20 /                                                          \
	 100) /**< gc triggered when number of the free pages under threshold */

enum {
	PAGE_FTL_IOCTL_TRIM = 0,
};

/**
 * @brief segment information structure
 * @note
 * Segment number is same as block number
 */
struct page_ftl_segment {
	int nr_free_pages;
	int nr_valid_pages;
	int is_gc;

	uint64_t *use_bits; /**< contain the use page information */
	list_node_t *lpn_list; /**< lba_list which contains the valid data */
};

/**
 * @brief contain the page flash translation layer information
 */
struct page_ftl {
	uint32_t *trans_map; /**< page-level mapping table */
	uint64_t alloc_segnum; /**< last allocated segment number */
	struct page_ftl_segment *segments;
	struct device *dev;
	pthread_mutex_t mutex;
	pthread_mutex_t gc_mutex;
	pthread_rwlock_t *bus_rwlock;
#ifdef PAGE_FTL_USE_GLOBAL_RWLOCK
	pthread_rwlock_t rwlock;
#endif
	pthread_t gc_thread;
	int o_flags;

	// cppcheck-suppress unusedStructMember
	list_node_t *gc_list; /**< garbage collection target list */
	uint64_t *gc_seg_bits; /**< to find segnum is in gc list or not */
};

/**
 * @brief layout assertions
 *
 * `PageFtl` / `PageFtlSegment` in `src/ftl/mod.rs` are `#[repr(C)]` mirrors of
 * these structs and C reads them by offset, so the layout is the contract. The
 * Rust `layout_matches_c` test asserts the same portable offset formulas, so
 * keep the two in sync and never hardcode literal byte numbers here.
 */
#define PAGE_FTL_ALIGN_UP(value, align) (((value) + (align) - 1) / (align) * (align))

#ifdef __cplusplus
#define PAGE_FTL_ALIGNOF(type) alignof(type)
// cppcheck-suppress missingIncludeSystem
#define PAGE_FTL_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#else
#define PAGE_FTL_ALIGNOF(type) _Alignof(type)
#define PAGE_FTL_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl_segment, nr_free_pages) == 0,
		       "page_ftl_segment.nr_free_pages must be first");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl_segment, nr_valid_pages) ==
			       sizeof(int),
		       "page_ftl_segment.nr_valid_pages must follow "
		       "nr_free_pages");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl_segment, is_gc) ==
			       2 * sizeof(int),
		       "page_ftl_segment.is_gc must follow nr_valid_pages");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl_segment, use_bits) ==
			       PAGE_FTL_ALIGN_UP(3 * sizeof(int), sizeof(void *)),
		       "page_ftl_segment.use_bits must follow the counters");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl_segment, lpn_list) ==
			       offsetof(struct page_ftl_segment, use_bits) +
				       sizeof(void *),
		       "page_ftl_segment.lpn_list must follow use_bits");
PAGE_FTL_STATIC_ASSERT(sizeof(struct page_ftl_segment) ==
			       offsetof(struct page_ftl_segment, lpn_list) +
				       sizeof(void *),
		       "page_ftl_segment must end after lpn_list");

PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, trans_map) == 0,
		       "page_ftl.trans_map must be first");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, alloc_segnum) ==
			       sizeof(void *),
		       "page_ftl.alloc_segnum must follow trans_map");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, segments) ==
			       offsetof(struct page_ftl, alloc_segnum) +
				       sizeof(uint64_t),
		       "page_ftl.segments must follow alloc_segnum");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, dev) ==
			       offsetof(struct page_ftl, segments) +
				       sizeof(void *),
		       "page_ftl.dev must follow segments");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, mutex) ==
			       PAGE_FTL_ALIGN_UP(offsetof(struct page_ftl, dev) +
							 sizeof(void *),
						 PAGE_FTL_ALIGNOF(pthread_mutex_t)),
		       "page_ftl.mutex must follow dev");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, gc_mutex) ==
			       offsetof(struct page_ftl, mutex) +
				       sizeof(pthread_mutex_t),
		       "page_ftl.gc_mutex must follow mutex");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, bus_rwlock) ==
			       offsetof(struct page_ftl, gc_mutex) +
				       sizeof(pthread_mutex_t),
		       "page_ftl.bus_rwlock must follow gc_mutex");

#ifdef PAGE_FTL_USE_GLOBAL_RWLOCK
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, rwlock) ==
			       offsetof(struct page_ftl, bus_rwlock) +
				       sizeof(void *),
		       "page_ftl.rwlock must follow bus_rwlock");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, gc_thread) ==
			       offsetof(struct page_ftl, rwlock) +
				       sizeof(pthread_rwlock_t),
		       "page_ftl.gc_thread must follow rwlock");
#else
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, gc_thread) ==
			       offsetof(struct page_ftl, bus_rwlock) +
				       sizeof(void *),
		       "page_ftl.gc_thread must follow bus_rwlock");
#endif
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, o_flags) ==
			       offsetof(struct page_ftl, gc_thread) +
				       sizeof(pthread_t),
		       "page_ftl.o_flags must follow gc_thread");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, gc_list) ==
			       PAGE_FTL_ALIGN_UP(
				       offsetof(struct page_ftl, o_flags) +
					       sizeof(int),
				       sizeof(void *)),
		       "page_ftl.gc_list must follow o_flags");
PAGE_FTL_STATIC_ASSERT(offsetof(struct page_ftl, gc_seg_bits) ==
			       offsetof(struct page_ftl, gc_list) +
				       sizeof(void *),
		       "page_ftl.gc_seg_bits must follow gc_list");
PAGE_FTL_STATIC_ASSERT(sizeof(struct page_ftl) ==
			       PAGE_FTL_ALIGN_UP(
				       offsetof(struct page_ftl, gc_seg_bits) +
					       sizeof(void *),
				       PAGE_FTL_ALIGNOF(struct page_ftl)),
		       "page_ftl must end after gc_seg_bits");

#undef PAGE_FTL_STATIC_ASSERT
#undef PAGE_FTL_ALIGNOF
#undef PAGE_FTL_ALIGN_UP

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
ssize_t page_ftl_do_gc(struct page_ftl *);
ssize_t page_ftl_gc_from_list(struct page_ftl *, struct device_request *,
			      double gc_ratio);

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

static inline size_t page_ftl_get_free_pages(struct page_ftl *pgftl)
{
	size_t free_pages;
	size_t nr_segments, segnum;
	struct page_ftl_segment *segment;

	nr_segments = device_get_nr_segments(pgftl->dev);

	free_pages = 0;
	for (segnum = 0; segnum < nr_segments; segnum++) {
		segment = &pgftl->segments[segnum];
		assert(NULL != segment);
		free_pages += (size_t)__atomic_load_n(&segment->nr_free_pages, __ATOMIC_SEQ_CST);
	}
	return free_pages;
}
#endif
