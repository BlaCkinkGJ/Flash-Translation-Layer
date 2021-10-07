/**
 * @file page-gc.c
 * @brief garbage collection logic for page ftl
 * @author Gijun Oh
 * @version 1.0
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

static void page_ftl_erase_end_rq(struct device_request *request)
{
	device_free_request(request);
}

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
		return -EFAULT;
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
		return -EFAULT;
	}
	free(buffer);
	return ret;
}

static int page_ftl_valid_page_copy(struct page_ftl *pgftl,
				    struct page_ftl_segment *segment)
{
	int ret = 0;
	GList *list;

	(void)pgftl;
	list = segment->lpn_list;

	while (list) {
		size_t lpn;
		char *buffer;

		GList *next = list->next;
		lpn = GPOINTER_TO_SIZE(list->data);
		ret = page_ftl_read_valid_page(pgftl, lpn, &buffer);
		if (ret < 0) {
			pr_err("read valid page failed\n");
			return ret;
		}
		ret = page_ftl_write_valid_page(pgftl, lpn, buffer);
		if (ret < 0) {
			pr_err("write valid page failed\n");
			return ret;
		}
		list = next;
	}
	return ret;
}

int page_ftl_do_gc(struct page_ftl *pgftl)
{
	struct device_address paddr;
	struct page_ftl_segment *segment;
	int ret;
	size_t segnum;

	segment = page_ftl_pick_gc_target(pgftl);
	if (segment == NULL) {
		pr_debug("gc target segment doesn't exist\n");
		return 0;
	}
	segnum = page_ftl_get_segment_number(pgftl, (uintptr_t)segment);
	pr_debug("current segnum: %zu\n", segnum);

	ret = page_ftl_valid_page_copy(pgftl, segment);
	if (ret < 0) {
		pr_err("valid page copy failed\n");
		return ret;
	}

	paddr.lpn = 0;
	paddr.format.block = segnum;
	ret = page_ftl_segment_erase(pgftl, paddr);
	if (ret) {
		pr_err("do erase failed\n");
		return ret;
	}

	ret = page_ftl_segment_data_init(pgftl, segment);
	if (ret) {
		pr_err("initialize the segment data failed\n");
		return ret;
	}
	reset_bit(pgftl->gc_seg_bits, segnum);
	return 0;
}