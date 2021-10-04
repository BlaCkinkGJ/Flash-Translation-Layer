/**
 * @file page-read.c
 * @brief read logic for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include "include/page.h"
#include "include/log.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>

static void page_ftl_read_end_rq(struct device_request *read_rq)
{
	struct device_request *request;
	struct page_ftl *pgftl;
	size_t offset;

	request = (struct device_request *)read_rq->rq_private;
	pgftl = (struct page_ftl *)request->rq_private;
	offset = page_ftl_get_page_offset(pgftl, request->sector);

	memcpy(request->data, &((char *)read_rq->data)[offset],
	       request->data_len);
	free(read_rq->data);
	free(read_rq);
	free(request);
}

ssize_t page_ftl_read(struct page_ftl *pgftl, struct device_request *request)
{
	struct device *dev;
	struct device_request *read_rq;

	char *buffer = NULL;

	size_t page_size;
	size_t lpn, offset;

	ssize_t ret = 0;
	ssize_t data_len;

	read_rq =
		(struct device_request *)malloc(sizeof(struct device_request));
	if (read_rq == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}

	dev = pgftl->dev;
	page_size = device_get_page_size(dev);
	lpn = page_ftl_get_lpn(pgftl, request->sector);

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
	memset(buffer, 0, page_size);

	read_rq->flag = DEVICE_READ;
	read_rq->data = buffer;
	read_rq->data_len = page_size;
	read_rq->paddr.lpn = pgftl->trans_map[lpn];
	read_rq->rq_private = request;
	read_rq->end_rq = page_ftl_read_end_rq;

	data_len = request->data_len;
	dev->d_op->read(dev, read_rq);
	ret = data_len;

	return ret;
exception:
	if (buffer) {
		free(buffer);
	}
	if (read_rq) {
		free(read_rq);
	}
	return ret;
}
