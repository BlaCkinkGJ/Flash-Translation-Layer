/**
 * @file page-read.c
 * @brief read logic for page ftl
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-09-22
 */
#include "include/page.h"
#include "include/log.h"

#include <errno.h>
#include <stringlib.h>
#include <stdlib.h>
#include <string.h>

/**
 * @brief read's end request function
 *
 * @param request the request which is submitted before
 */
static void page_ftl_read_end_rq(struct device_request *read_rq)
{
	struct device_request *request;
	struct device_address paddr;
	struct page_ftl *pgftl;
	size_t offset;

	request = (struct device_request *)read_rq->rq_private;
	pgftl = (struct page_ftl *)request->rq_private;
	offset = page_ftl_get_page_offset(pgftl, request->sector);

	paddr = read_rq->paddr;

	__memcpy_aarch64_simd(request->data, &((char *)read_rq->data)[offset],
			      request->data_len);
	free(read_rq->data);
	device_free_request(read_rq);

	pthread_mutex_lock(&request->mutex);
	if (g_atomic_int_get(&request->is_finish) == 0) {
		pthread_cond_signal(&request->cond);
	}
	g_atomic_int_set(&request->is_finish, 1);
	pthread_mutex_unlock(&request->mutex);
}

/**
 * @brief the core logic for reading the request to the device.
 *
 * @param pgftl pointer of the page FTL structure
 * @param request user's request pointer
 *
 * @return reading data size. a negative number means fail to read.
 * @note
 * if paddr.lpn doesn't exist, this function returns the buffer filled 0 value.
 */
ssize_t page_ftl_read(struct page_ftl *pgftl, struct device_request *request)
{
	struct device *dev;
	struct device_request *read_rq;
	struct device_address paddr;

	char *buffer;

	size_t page_size;
	size_t lpn, offset;

	ssize_t ret = 0;
	ssize_t data_len;

	buffer = NULL;
	read_rq = NULL;

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);
	lpn = page_ftl_get_lpn(pgftl, request->sector);

	pthread_mutex_lock(&pgftl->mutex);
	paddr.lpn = pgftl->trans_map[lpn];
	pthread_mutex_unlock(&pgftl->mutex);

	if (paddr.lpn == PADDR_EMPTY) { /**< YOU MUST TAKE CARE OF THIS LINE */
		pr_warn("cannot find the mapping information (lpn: %zu)\n",
			lpn);
		__memset_aarch64(request->data, 0, request->data_len);
		ret = request->data_len;
		device_free_request(request);
		goto exception;
	}

	request->rq_private = pgftl;

	offset = page_ftl_get_page_offset(pgftl, request->sector);

	if (offset + request->data_len > page_size) {
		pr_err("overflow the read data (offset: %lu, length: %zu)\n",
		       offset, request->data_len);
		ret = -EINVAL;
		goto exception;
	}

	buffer = (char *)malloc(page_size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	__memset_aarch64(buffer, 0, page_size);

	read_rq = device_alloc_request(DEVICE_DEFAULT_REQUEST);
	if (read_rq == NULL) {
		pr_err("request allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}

	read_rq->flag = DEVICE_READ;
	read_rq->data = buffer;
	read_rq->data_len = page_size;
	read_rq->paddr = paddr;
	read_rq->rq_private = request;
	read_rq->end_rq = page_ftl_read_end_rq;

	data_len = request->data_len;
	ret = dev->d_op->read(dev, read_rq);
	if (ret < 0) {
		pr_err("device read failed (ppn: %u)\n", request->paddr.lpn);
		read_rq = NULL;
		buffer = NULL;
		goto exception;
	}

	pthread_mutex_lock(&request->mutex);
	while (g_atomic_int_get(&request->is_finish) == 0) {
		pthread_cond_wait(&request->cond, &request->mutex);
	}
	pthread_mutex_unlock(&request->mutex);

	device_free_request(request);

	ret = data_len;
	return ret;
exception:
	if (buffer) {
		free(buffer);
	}
	if (read_rq) {
		device_free_request(read_rq);
	}
	return ret;
}
