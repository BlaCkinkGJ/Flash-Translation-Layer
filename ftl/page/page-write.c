/**
 * @file page-write.c
 * @brief write logic for page ftl
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-09-22
 */
#include "include/module.h"
#include "include/page.h"
#include "include/device.h"
#include "include/log.h"
#include "include/bits.h"

#include <pthread.h>
#include <assert.h>
#include <stdlib.h>
#include <errno.h>
#include <string.h>

#include <glib.h>

/**
 * @brief invalidate a segment that including to the given LPN
 *
 * @param pgftl pointer of the page FTL structure
 * @param lpn logical page address to invalidate
 */
static void page_ftl_invalidate(struct page_ftl *pgftl, size_t lpn)
{
	struct page_ftl_segment *segment;
	struct device_address paddr;

	uint32_t segnum;
	size_t nr_valid_pages;

	pthread_mutex_lock(&pgftl->mutex);
	/**< segment information update */
	paddr.lpn = page_ftl_get_ppn(pgftl, lpn);
	segnum = paddr.format.block;
	segment = &pgftl->segments[segnum];

	segment->lpn_list =
		g_list_remove(segment->lpn_list, GSIZE_TO_POINTER(lpn));

	nr_valid_pages = g_atomic_int_get(&segment->nr_valid_pages);
	g_atomic_int_set(&segment->nr_valid_pages, nr_valid_pages - 1);

	/**< global information update */
	page_ftl_invalidate_map(pgftl, lpn);
	if (get_bit(pgftl->gc_seg_bits, segnum) != 1) {
		pgftl->gc_list = g_list_prepend(pgftl->gc_list, segment);
		set_bit(pgftl->gc_seg_bits, segnum);
	}
	pthread_mutex_unlock(&pgftl->mutex);
}

/**
 * @brief write's end request function
 *
 * @param request the request which is submitted before
 */
static void page_ftl_write_end_rq(struct device_request *request)
{
	free(request->data);

	pthread_mutex_lock(&request->mutex);
	if (g_atomic_int_get(&request->is_finish) == 0) {
		pthread_cond_signal(&request->cond);
	}
	g_atomic_int_set(&request->is_finish, 1);
	pthread_mutex_unlock(&request->mutex);
}

/**
 * @brief read sequence for overwrite
 *
 * @param pgftl pointer of the page FTL
 * @param lpn logical page address which wants to overwrite
 * @param buffer a pointer to a buffer containing the result of the read
 *
 * @return reading data size. a negative number means fail to read
 *
 * @note
 * NAND-based storage doesn't allow to do overwrite.
 * Therefore, you must use the out-of-place update. So this logic is necessary.
 */
static ssize_t page_ftl_read_for_overwrite(struct page_ftl *pgftl, size_t lpn,
					   void *buffer)
{
	struct device *dev;
	struct device_request *read_rq;

	size_t page_size;
	ssize_t ret;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);

	read_rq = device_alloc_request(DEVICE_DEFAULT_REQUEST);
	if (read_rq == NULL) {
		pr_err("crete read request failed\n");
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

/**
 * @brief the core logic for writing the request to the device.
 *
 * @param pgftl pointer of the page FTL structure
 * @param request user's request pointer
 *
 * @return writing data size. a negative number means fail to write.
 */
ssize_t page_ftl_write(struct page_ftl *pgftl, struct device_request *request)
{
	struct device *dev;
	struct page_ftl_segment *segment;
	struct page_ftl_segment *read_segment;
	struct device_address paddr, read_paddr;
	char *buffer;
	ssize_t ret;
	size_t page_size;

	size_t lpn, offset;
	size_t nr_entries;

	size_t write_size;

	pthread_mutex_lock(&pgftl->mutex);
	dev = pgftl->dev;
	page_size = device_get_page_size(dev);
	write_size = request->data_len;
	segment = NULL;

	lpn = page_ftl_get_lpn(pgftl, request->sector);
	read_paddr.lpn = page_ftl_get_ppn(pgftl, lpn);
	offset = page_ftl_get_page_offset(pgftl, request->sector);

	nr_entries = page_ftl_get_map_size(pgftl) / sizeof(uint32_t);
	if (lpn > nr_entries) {
		pr_err("invalid lpn detected (lpn: %lu, max: %lu)\n", lpn,
		       nr_entries);
		ret = -EINVAL;
		pthread_mutex_unlock(&pgftl->mutex);
		goto exception;
	}

	if (offset + request->data_len > page_size) {
		pr_err("overflow the write data (offset: %lu, length: %zu)\n",
		       offset, request->data_len);
		ret = -EINVAL;
		pthread_mutex_unlock(&pgftl->mutex);
		goto exception;
	}

	paddr = page_ftl_get_free_page(pgftl); /**< segment lock acquired */
	if (paddr.lpn == PADDR_EMPTY) {
		pr_err("cannot allocate the valid page from device\n");
		ret = -EFAULT;
		pthread_mutex_unlock(&pgftl->mutex);
		goto exception;
	}

	segment = &pgftl->segments[paddr.format.block];
	if (segment == NULL) {
		pr_err("segment does not exist (segnum: %u)\n",
		       paddr.format.block);
		ret = -EFAULT;
		pthread_mutex_unlock(&pgftl->mutex);
		goto exception;
	}

	if (read_paddr.lpn != PADDR_EMPTY) {
		read_segment = &pgftl->segments[read_paddr.format.block];
		while (1) {
			ret = pthread_mutex_trylock(&read_segment->mutex);
			pthread_mutex_unlock(&pgftl->mutex);
			if (ret == 0) {
				break;
			}
			usleep(100);
			pthread_mutex_lock(&pgftl->mutex);
		}
	} else {
		pthread_mutex_unlock(&pgftl->mutex);
	}

	buffer = (char *)malloc(page_size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(buffer, 0, page_size);
	if (read_paddr.lpn != PADDR_EMPTY) {
		ret = page_ftl_read_for_overwrite(pgftl, read_paddr.lpn,
						  buffer);
		pthread_mutex_unlock(&read_segment->mutex);
		if (ret < 0) {
			pr_err("read failed (lpn:%u)\n", read_paddr.lpn);
			goto exception;
		}
	}
	memcpy(&buffer[offset], request->data, write_size);

	request->flag = DEVICE_WRITE;
	request->data = buffer;
	request->paddr = paddr;
	request->rq_private = (void *)pgftl;
	request->data_len = page_size;
	request->end_rq = page_ftl_write_end_rq;

	ret = dev->d_op->write(dev, request);
	if (ret < 0) {
		pr_err("device write failed (ppn: %u)\n", request->paddr.lpn);
		goto exception;
	}

	pthread_mutex_lock(&request->mutex);
	while (g_atomic_int_get(&request->is_finish) == 0) {
		pthread_cond_wait(&request->cond, &request->mutex);
	}
	pthread_mutex_unlock(&request->mutex);

	pthread_mutex_lock(&pgftl->mutex);
	lpn = page_ftl_get_lpn(pgftl, request->sector);
	if (page_ftl_get_ppn(pgftl, lpn) != PADDR_EMPTY) {
		page_ftl_invalidate(pgftl, lpn);
		pr_debug("invalidate address: %lu => %u\n", lpn,
			 page_ftl_get_ppn(pgftl, lpn));
	}
	/**< segment information update */
	segment = &pgftl->segments[request->paddr.format.block];
	segment->lpn_list =
		g_list_prepend(segment->lpn_list, GSIZE_TO_POINTER(lpn));

	/**< global information update */
	page_ftl_update_map(pgftl, request->sector, request->paddr.lpn);

	pr_debug("new address: %lu => %u (seg: %u)\n", lpn,
		 page_ftl_get_ppn(pgftl, lpn),
		 page_ftl_get_ppn(pgftl, lpn) >> 13);
	pr_debug("%u/%u(free/valid)\n",
		 g_atomic_int_get(&segment->nr_free_pages),
		 g_atomic_int_get(&segment->nr_valid_pages));
	pthread_mutex_unlock(&pgftl->mutex);

	pthread_mutex_unlock(&segment->mutex);
	device_free_request(request);

	return write_size;
exception:
	if (segment) {
		pthread_mutex_unlock(&segment->mutex);
	}
	if (ret >= 0 && request) {
		device_free_request(request);
	}
	return ret;
}
