/**
 * @file page-gc.c
 * @brief garbage collection logic for page ftl
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-10-06
 */
#include <glib.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

#include "include/page.h"
#include "include/log.h"
#include "include/bits.h"

/**
 * @brief page ftl gc list compare function
 *
 * @param a compare target 1
 * @param b compare target 2
 *
 * @return to make precede a segment that contains the less valid pages
 */
gint page_ftl_gc_list_cmp(gconstpointer a, gconstpointer b)
{
	struct page_ftl_segment *segment[2];
	uint64_t nr_valid_pages[2];
	segment[0] = (struct page_ftl_segment *)a;
	segment[1] = (struct page_ftl_segment *)b;
	nr_valid_pages[0] = g_atomic_int_get(&segment[0]->nr_valid_pages);
	nr_valid_pages[1] = g_atomic_int_get(&segment[1]->nr_valid_pages);
	return nr_valid_pages[0] - nr_valid_pages[1];
}

/**
 * @brief erase's end request function
 *
 * @param request the request which is submitted before
 */
static void page_ftl_erase_end_rq(struct device_request *request)
{
	device_free_request(request);
}

/**
 * @brief the function which chooses the appropriate garbage collection target.
 *
 * @param pgftl pointer of the page FTL structure
 *
 * @return garbage collection target segment's pointer
 */
static struct page_ftl_segment *page_ftl_pick_gc_target(struct page_ftl *pgftl)
{
	struct page_ftl_segment *segment;
	if (pgftl->gc_list == NULL) {
		return NULL;
	}
	pgftl->gc_list = g_list_sort(pgftl->gc_list, page_ftl_gc_list_cmp);
	segment = (struct page_ftl_segment *)pgftl->gc_list->data;
	pr_debug("gc target: %zd (valid: %u) => %p\n",
		 page_ftl_get_segment_number(pgftl, (uintptr_t)segment),
		 g_atomic_int_get(&segment->nr_valid_pages), segment);
	pgftl->gc_list = g_list_remove(pgftl->gc_list, segment);
	g_atomic_int_set(&segment->nr_free_pages, 0);
	return segment;
}

/**
 * @brief erase the garbage collection target segment
 *
 * @param pgftl pointer of the page FTL structure
 * @param paddr the address containing the segment number which wants to erase
 *
 * @return 0 for success, negative number for fail
 */
static int page_ftl_segment_erase(struct page_ftl *pgftl,
				  struct device_address paddr)
{
	struct device *dev;
	struct device_request *request;
	int ret;

	dev = pgftl->dev;

	request = device_alloc_request(DEVICE_DEFAULT_REQUEST);
	if (request == NULL) {
		pr_err("request allocation failed\n");
		return -ENOMEM;
	}

	request->flag = DEVICE_ERASE;
	request->paddr = paddr;
	request->end_rq = page_ftl_erase_end_rq;
	ret = dev->d_op->erase(dev, request);
	if (ret) {
		pr_err("erase error detected(errno: %d)\n", ret);
		return ret;
	}
	return 0;
}

/**
 * @brief read the valid pages from the garbage collection target segment
 *
 * @param pgftl pointer of the page FTL structure
 * @param lpn read position which contains the logical page number
 * @param __buffer buffer pointer's address which dynamically allocated by this function
 *
 * @return reading data size. a negative number means fail to read
 */
static ssize_t page_ftl_read_valid_page(struct page_ftl *pgftl, size_t lpn,
					char **__buffer)
{
	struct device *dev;
	struct device_request *request;
	char *buffer;
	ssize_t page_size;
	ssize_t ret;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);
	request = NULL;

	buffer = (char *)malloc(page_size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(buffer, 0, page_size);

	request = device_alloc_request(DEVICE_DEFAULT_REQUEST);
	if (request == NULL) {
		pr_err("request allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}

	request->flag = DEVICE_READ;
	request->data_len = page_size;
	request->sector = lpn * page_size;
	request->data = buffer;

	ret = page_ftl_read(pgftl, request);
	if (ret != page_size) {
		pr_err("invalid read size detected (expected: %zd, acutal: %zd)\n",
		       page_size, ret);
		ret = -EFAULT;
		goto exception;
	}

	*__buffer = buffer;
	return ret;
exception:
	if (buffer) {
		free(buffer);
	}
	if (request) {
		device_free_request(request);
	}
	return ret;
}

/**
 * @brief write valid page to the other segment.
 *
 * @param pgftl pointer of the page FTL structure
 * @param lpn write position which contains the logical page number
 * @param buffer buffer pointer containing the valid page
 *
 * @return writing data size. a negative number means fail to write
 */
static ssize_t page_ftl_write_valid_page(struct page_ftl *pgftl, size_t lpn,
					 char *buffer)
{
	struct device *dev;
	struct device_request *request;
	ssize_t page_size;
	ssize_t ret;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);

	request = device_alloc_request(DEVICE_DEFAULT_REQUEST);
	if (request == NULL) {
		pr_err("request allocation failed\n");
		return -ENOMEM;
	}

	request->flag = DEVICE_WRITE;
	request->data_len = page_size;
	request->sector = lpn * page_size;
	request->data = buffer;

	ret = page_ftl_write(pgftl, request);
	if (ret != page_size) {
		pr_err("invalid write size detected (expected: %zd, acutal: %zd)\n",
		       page_size, ret);
		device_free_request(request);
		free(buffer);
		return -EFAULT;
	}
	free(buffer);
	return ret;
}

/**
 * @brief core logic of the valid page copy
 *
 * @param pgftl pointer of the page FTL structure
 * @param segment segment which wants to copy the valid pages
 *
 * @return 0 for success, negative number for fail
 */
static int page_ftl_valid_page_copy(struct page_ftl *pgftl,
				    struct page_ftl_segment *segment)
{
	int ret = 0;
	GList *list;

	list = segment->lpn_list;

	while (list) {
		size_t lpn;
		char *buffer;

		pthread_mutex_lock(&pgftl->mutex);
		GList *next = list->next;
		lpn = GPOINTER_TO_SIZE(list->data);
		pthread_mutex_unlock(&pgftl->mutex);
		ret = page_ftl_read_valid_page(pgftl, lpn, &buffer);
		if (ret < 0) {
			pr_warn("read valid page failed\n");
			list = next;
			continue;
		}
		ret = page_ftl_write_valid_page(pgftl, lpn, buffer);
		if (ret < 0) {
			pr_warn("write valid page failed\n");
			list = next;
			continue;
		}
		list = next;
	}
	return ret;
}

/**
 * @brief core logic of the garbage collection
 *
 * @param pgftl pointer of the page FTL structure
 *
 * @return 0 for success, negative number for fail
 */
int page_ftl_do_gc(struct page_ftl *pgftl)
{
	struct device_address paddr;
	struct page_ftl_segment *segment;
	int ret;
	size_t segnum;

	pthread_mutex_lock(&pgftl->mutex);
	segment = page_ftl_pick_gc_target(pgftl);
	if (segment == NULL) {
		pr_debug("gc target segment doesn't exist\n");
		ret = 0;
		pthread_mutex_unlock(&pgftl->mutex);
		goto exit;
	}
	while (1) {
		ret = pthread_mutex_trylock(&segment->mutex);
		pthread_mutex_unlock(&pgftl->mutex);
		if (ret == 0) {
			break;
		}
		usleep(100);
		pthread_mutex_lock(&pgftl->mutex);
	}
	segnum = page_ftl_get_segment_number(pgftl, (uintptr_t)segment);
	pr_debug("current segnum: %zu\n", segnum);

	ret = page_ftl_valid_page_copy(pgftl, segment);
	if (ret < 0) {
		pr_err("valid page copy failed\n");
		goto exit;
	}

	paddr.lpn = 0;
	paddr.format.block = segnum;
	ret = page_ftl_segment_erase(pgftl, paddr);
	if (ret) {
		pr_err("do erase failed\n");
		goto exit;
	}

	ret = page_ftl_segment_data_init(pgftl, segment);
	if (ret) {
		pr_err("initialize the segment data failed\n");
		goto exit;
	}
	reset_bit(pgftl->gc_seg_bits, segnum);
exit:
	if (segment) {
		pthread_mutex_unlock(&segment->mutex);
	}
	return ret;
}
