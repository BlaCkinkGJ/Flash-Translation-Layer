/**
 * @file page-write.c
 * @brief write logic for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include "include/module.h"
#include "include/page.h"
#include "include/device.h"
#include "include/log.h"
#include "include/bits.h"
#include "include/atomic.h"

#include <pthread.h>
#include <assert.h>
#include <stdlib.h>
#include <errno.h>
#include <string.h>

#include <glib.h>

static void page_ftl_invalidate(struct page_ftl *pgftl, size_t lpn)
{
	struct page_ftl_segment *segment;
	struct device_address paddr;

	uint32_t segnum;
	size_t nr_valid_pages;

	/**< segment information update */
	paddr.lpn = pgftl->trans_map[lpn];
	segnum = paddr.format.block;
	segment = &pgftl->segments[segnum];

	segment->lpn_list =
		g_list_remove(segment->lpn_list, GSIZE_TO_POINTER(lpn));

	nr_valid_pages = atomic_load(&segment->nr_valid_pages);
	atomic_store(&segment->nr_valid_pages, nr_valid_pages - 1);

	/**< global information update */
	pgftl->trans_map[lpn] = PADDR_EMPTY;
	if (get_bit(pgftl->gc_seg_bits, segnum) != 1) {
		pgftl->gc_list = g_list_prepend(pgftl->gc_list, segment);
		set_bit(pgftl->gc_seg_bits, segnum);
	}
}

static void page_ftl_write_end_rq(struct device_request *request)
{
	struct page_ftl *pgftl;
	struct page_ftl_segment *segment;

	size_t lpn;

	pgftl = (struct page_ftl *)request->rq_private;

	lpn = page_ftl_get_lpn(pgftl, request->sector);
	if (pgftl->trans_map[lpn] != PADDR_EMPTY) {
		page_ftl_invalidate(pgftl, lpn);
		pr_debug("invalidate address: %lu => %u\n", lpn,
			 pgftl->trans_map[lpn]);
	}
	/**< segment information update */
	segment = &pgftl->segments[request->paddr.format.block];
	segment->lpn_list =
		g_list_prepend(segment->lpn_list, GSIZE_TO_POINTER(lpn));

	/**< global information update */
	page_ftl_update_map(pgftl, request->sector, request->paddr.lpn);

	pr_debug("new address: %lu => %u\n", lpn, pgftl->trans_map[lpn]);
	pr_debug("%lu/%lu(free/valid)\n", atomic_load(&segment->nr_free_pages),
		 atomic_load(&segment->nr_valid_pages));

	free(request->data);
	free(request);
}

static ssize_t page_ftl_read_for_overwrite(struct page_ftl *pgftl, size_t lpn,
					   void *buffer)
{
	struct device *dev;
	struct device_request *read_rq;

	size_t page_size;
	ssize_t ret;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);

	read_rq =
		(struct device_request *)malloc(sizeof(struct device_request));
	if (read_rq == NULL) {
		return -ENOMEM;
	}
	read_rq->flag = DEVICE_READ;
	read_rq->sector = lpn * page_size;
	read_rq->data_len = page_size;
	read_rq->data = buffer;
	ret = page_ftl_read(pgftl, read_rq);
	if (ret < 0) {
		pr_err("previous buffer read failed\n");
		return -EFAULT;
	}
	return ret;
}

ssize_t page_ftl_write(struct page_ftl *pgftl, struct device_request *request)
{
	struct device *dev;
	char *buffer;
	size_t page_size;

	size_t lpn, offset;
	size_t nr_entries;

	size_t write_size;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);
	write_size = request->data_len;

	lpn = page_ftl_get_lpn(pgftl, request->sector);
	offset = page_ftl_get_page_offset(pgftl, request->sector);

	nr_entries = page_ftl_get_map_size(pgftl) / sizeof(uint32_t);
	if (lpn > nr_entries) {
		pr_err("invalid lpn detected (lpn: %lu, max: %lu)\n", lpn,
		       nr_entries);
		return -EINVAL;
	}

	if (offset + request->data_len > page_size) {
		pr_err("overflow the write data (offset: %lu, length: %zu)\n",
		       offset, request->data_len);
		return -EINVAL;
	}

	buffer = (char *)malloc(page_size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		return -ENOMEM;
	}
	memset(buffer, 0, page_size);
	if (pgftl->trans_map[lpn] != PADDR_EMPTY) {
		ssize_t ret;
		ret = page_ftl_read_for_overwrite(pgftl, lpn, buffer);
		if (ret < 0) {
			pr_err("read failed (lpn:%lu)\n", lpn);
			return ret;
		}
	}
	memcpy(&buffer[offset], request->data, write_size);

	request->flag = DEVICE_WRITE;
	request->data = buffer;
	request->paddr =
		page_ftl_get_free_page(pgftl); /**< global data retrieve */
	request->rq_private = (void *)pgftl;
	request->data_len = page_size;
	request->end_rq = page_ftl_write_end_rq;

	dev->d_op->write(dev, request);

	return write_size;
}
