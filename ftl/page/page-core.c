/**
 * @file page-core.c
 * @brief core logic for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <errno.h>

#include <assert.h>
#include <string.h>
#include <glib.h>

#include "include/atomic.h"
#include "include/page.h"
#include "include/log.h"
#include "include/bits.h"

/**
 * @brief allocate the segment's bitmap
 *
 * @param pgftl pointer of the page-ftl structure
 * @param bitmap double pointer of the bitmap
 *
 * @return 0 for successfully allocated
 */
static int page_ftl_alloc_bitmap(struct page_ftl *pgftl, uint64_t **bitmap)
{
	size_t nr_pages_per_segment;
	uint64_t *bits;

	nr_pages_per_segment = device_get_pages_per_segment(pgftl->dev);
	bits = (uint64_t *)malloc(BITS_TO_BYTES(nr_pages_per_segment));
	if (bits == NULL) {
		pr_err("bitmap allocation failed\n");
		return -ENOMEM;
	}
	*bitmap = bits;
	return 0;
}

/**
 * @brief initialize the page ftl's segment data only
 *
 * @param pgftl pointer of the page-ftl structure
 * @param segment pointer of the target segment
 *
 * @return 0 for successfully initialized
 */
int page_ftl_segment_data_init(struct page_ftl *pgftl,
			       struct page_ftl_segment *segment)
{
	size_t nr_pages_per_segment;
	nr_pages_per_segment = device_get_pages_per_segment(pgftl->dev);
	atomic_store(&segment->nr_free_pages, nr_pages_per_segment);
	atomic_store(&segment->nr_valid_pages, 0);

	memset(segment->use_bits, 0, BITS_TO_BYTES(nr_pages_per_segment));
	if (segment->lpn_list) {
		g_list_free(segment->lpn_list);
	}
	segment->lpn_list = NULL;
	return 0;
}

/**
 * @brief initialize each segment's metadata
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return 0 to success, negative value to fail
 */
static int page_ftl_init_segment(struct page_ftl *pgftl)
{
	size_t nr_segments;
	size_t nr_pages_per_segment;

	struct page_ftl_segment *segments;

	nr_segments = device_get_nr_segments(pgftl->dev);
	nr_pages_per_segment = device_get_pages_per_segment(pgftl->dev);

	segments = (struct page_ftl_segment *)malloc(
		sizeof(struct page_ftl_segment) * nr_segments);
	if (segments == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	for (size_t i = 0; i < nr_segments; i++) {
		segments[i].use_bits = NULL;
	}
	for (size_t i = 0; i < nr_segments; i++) {
		int ret;
		ret = page_ftl_alloc_bitmap(pgftl, &segments[i].use_bits);
		if (ret) {
			pr_err("initialize the use bitmap failed (segnum: %zu)\n",
			       i);
			return ret;
		}
		segments[i].lpn_list = NULL;
		ret = page_ftl_segment_data_init(pgftl, &segments[i]);
		if (ret) {
			pr_err("initialize the segment data failed (segnum: %zu)\n",
			       i);
			return ret;
		}
		pr_debug("initialize the segment %zu (bits: %zu, size: %lu)\n",
			 i, nr_pages_per_segment,
			 (uint64_t)(nr_pages_per_segment) / 8);
	}

	pgftl->segments = segments;
	return 0;
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
	size_t nr_segments;

	struct device *dev;

	assert(NULL != pgftl->dev);

	err = pthread_mutex_init(&pgftl->mutex, NULL);
	if (err) {
		pr_err("mutex initialize failed\n");
		goto exception;
	}
	err = pthread_rwlock_init(&pgftl->rwlock, NULL);
	if (err) {
		pr_err("rwlock initialize failed\n");
		goto exception;
	}

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
		err = -ENOMEM;
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
	pgftl->gc_list = NULL;

	nr_segments = device_get_nr_segments(dev);
	pgftl->gc_seg_bits = (uint64_t *)malloc(BITS_TO_BYTES(nr_segments));
	if (pgftl->gc_seg_bits == NULL) {
		pr_err("memory allocation failed\n");
		goto exception;
	}
	memset(pgftl->gc_seg_bits, 0, BITS_TO_BYTES(nr_segments));

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
	int ret = 0;
	if (pgftl == NULL || request == NULL) {
		pr_err("null detected (pgftl:%p, request:%p)\n", pgftl,
		       request);
		return -EINVAL;
	}
	switch (request->flag) {
	case DEVICE_WRITE:
		pthread_rwlock_wrlock(&pgftl->rwlock);
		ret = page_ftl_write(pgftl, request);
		pthread_rwlock_unlock(&pgftl->rwlock);
		break;
	case DEVICE_READ:
		pthread_rwlock_rdlock(&pgftl->rwlock);
		ret = page_ftl_read(pgftl, request);
		pthread_rwlock_unlock(&pgftl->rwlock);
		break;
	case DEVICE_ERASE:
		pthread_rwlock_wrlock(&pgftl->rwlock);
		ret = page_ftl_do_gc(pgftl);
		pthread_rwlock_unlock(&pgftl->rwlock);
		break;
	default:
		pr_err("invalid flag detected: %u\n", request->flag);
		return -EINVAL;
	}
	return ret;
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
		uint64_t *use_bits;

		use_bits = segments[i].use_bits;

		if (use_bits != NULL) {
			free(use_bits);
		}

		segments[i].use_bits = NULL;

		if (segments[i].lpn_list) {
			g_list_free(segments[i].lpn_list);
			segments[i].lpn_list = NULL;
		}
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

	pthread_mutex_destroy(&pgftl->mutex);
	pthread_rwlock_destroy(&pgftl->rwlock);
	if (pgftl->segments) {
		page_ftl_free_segments(pgftl);
		free(pgftl->segments);
		pgftl->segments = NULL;
	}

	if (pgftl->trans_map) {
		free(pgftl->trans_map);
		pgftl->trans_map = NULL;
	}

	if (pgftl->gc_list) {
		g_list_free(pgftl->gc_list);
		pgftl->gc_list = NULL;
	}

	if (pgftl->gc_seg_bits) {
		free(pgftl->gc_seg_bits);
		pgftl->gc_seg_bits = NULL;
	}

	if (pgftl->dev && pgftl->dev->d_op) {
		struct device *dev = pgftl->dev;
		ret = dev->d_op->close(dev);
	}
	return ret;
}
