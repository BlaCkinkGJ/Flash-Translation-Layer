/**
 * @file raspberry.c
 * @brief implementation of the raspberry which is inherited by the device
 * @author Gijun Oh
 * @date 2023-05-29
 */
#include <cstdio>
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <stdint.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>

#include "flash.h"
#include "raspberry.h"
#include "device.h"
#include "log.h"
#include "bits.h"

/**
 * @brief open the raspberry (allocate the device resources)
 *
 * @param dev pointer of the device structure
 * @param name this does not use in this module
 * @param flags open flags for raspberry
 *
 * @return 0 for success, negative value to fail
 */
int raspberry_open(struct device *dev, const char *name, int flags)
{
	int ret = 0;
	char *buffer;
	size_t bitmap_size;
	uint64_t *is_used;
	struct raspberry *raspberry;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	size_t nr_segments;

	(void)name;
	(void)flags;

	info->nr_bus = (1 << DEVICE_NR_BUS_BITS);
	info->nr_chips = (1 << DEVICE_NR_CHIPS_BITS);
	block->nr_pages = (1 << DEVICE_NR_PAGES_BITS);

	package->nr_blocks = 1024;
	page->size = DEVICE_PAGE_SIZE;

	raspberry = (struct raspberry *)dev->d_private;
	raspberry->size = device_get_total_size(dev);
	raspberry->o_flags = flags;

	pr_info("raspberry generated (size: %zu bytes)\n", raspberry->size);
	buffer = (char *)malloc(raspberry->size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(buffer, 0, raspberry->size);
	raspberry->buffer = buffer;

	bitmap_size =
		(size_t)BITS_TO_UINT64_ALIGN(raspberry->size / page->size);
	is_used = (uint64_t *)malloc((size_t)bitmap_size);
	if (is_used == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	pr_info("bitmap generated (size: %zu bytes)\n", bitmap_size);
	memset(is_used, 0, bitmap_size);
	raspberry->is_used = is_used;

	nr_segments = device_get_nr_segments(dev);
	dev->badseg_bitmap =
		(uint64_t *)malloc((size_t)BITS_TO_UINT64_ALIGN(nr_segments));
	if (dev->badseg_bitmap == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(dev->badseg_bitmap, 0,
	       (size_t)BITS_TO_UINT64_ALIGN(nr_segments));
	return ret;
exception:
	raspberry_close(dev);
	return ret;
}

/**
 * @brief write to the raspberry
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return written size (bytes)
 */
ssize_t raspberry_write(struct device *dev, struct device_request *request)
{
	struct raspberry *raspberry = (struct raspberry *)dev->d_private;
	struct device_address addr;
	size_t page_size = device_get_page_size(dev);
	ssize_t ret = 0;
	int is_used;

	addr.lpn = 0;
	addr.raspberry_converter.bus = request->paddr.format.bus;
	addr.raspberry_converter.chip = request->paddr.format.chip;
	addr.raspberry_converter.block = request->paddr.format.block;
	addr.raspberry_converter.page = request->paddr.format.page;

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		ret = -ENODATA;
		goto exit;
	}

	if (request->flag != DEVICE_WRITE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_WRITE, request->flag);
		ret = -EINVAL;
		goto exit;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exit;
	}

	if (request->data_len != page_size) {
		pr_err("data write size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exit;
	}

	is_used = get_bit(raspberry->is_used, addr.lpn);
	if (is_used == 1) {
		pr_err("you overwrite the already written page\n");
		ret = -EINVAL;
		goto exit;
	}
	fflush(stdout);
	set_bit(raspberry->is_used, addr.lpn);
	memcpy(&raspberry->buffer[addr.lpn * page_size], request->data,
	       request->data_len);
	ret = (ssize_t)request->data_len;
	if (request->end_rq) {
		request->end_rq(request);
	}
exit:
	return ret;
}

/**
 * @brief read from the raspberry
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return read size (bytes)
 */
ssize_t raspberry_read(struct device *dev, struct device_request *request)
{
	struct raspberry *raspberry = (struct raspberry *)dev->d_private;
	struct device_address addr;
	size_t page_size;
	ssize_t ret;

	addr.lpn = 0;
	addr.raspberry_converter.bus = request->paddr.format.bus;
	addr.raspberry_converter.chip = request->paddr.format.chip;
	addr.raspberry_converter.block = request->paddr.format.block;
	addr.raspberry_converter.page = request->paddr.format.page;

	ret = 0;

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		ret = -ENODATA;
		goto exit;
	}

	if (request->flag != DEVICE_READ) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_READ, request->flag);
		ret = -EINVAL;
		goto exit;
	}

	page_size = device_get_page_size(dev);
	if (request->data_len != page_size) {
		pr_err("data read size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exit;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exit;
	}

	memcpy(request->data, &raspberry->buffer[addr.lpn * page_size],
	       request->data_len);
	ret = (ssize_t)request->data_len;
	pr_debug("request->end_rq %p %p\n", request->end_rq,
		 &((struct device_request *)request->rq_private)->mutex);
	if (request->end_rq) {
		request->end_rq(request);
	}
exit:
	return ret;
}

/**
 * @brief erase a segment
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return 0 for success, negative value for fail
 */
int raspberry_erase(struct device *dev, struct device_request *request)
{
	struct raspberry *raspberry = (struct raspberry *)dev->d_private;
	struct device_address addr;
	size_t page_size;
	uint32_t nr_pages_per_segment;
	uint32_t lpn;
	uint16_t segnum;
	int ret;

	addr.lpn = 0;
	ret = 0;

	if (request->flag != DEVICE_ERASE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_ERASE, request->flag);
		ret = -EINVAL;
		goto exit;
	}
	segnum = (uint16_t)request->paddr.format.block;
	page_size = device_get_page_size(dev);
	nr_pages_per_segment = (uint32_t)device_get_pages_per_segment(dev);
	addr.format.block = segnum;
	for (lpn = addr.lpn; lpn < addr.lpn + nr_pages_per_segment; lpn++) {
		memset(&raspberry->buffer[lpn * page_size], 0, page_size);
		reset_bit(raspberry->is_used, lpn);
	}

	if (request->end_rq) {
		request->end_rq(request);
	}
exit:
	return ret;
}

/**
 * @brief close the raspberry
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int raspberry_close(struct device *dev)
{
	struct raspberry *raspberry;
	if (dev->badseg_bitmap != NULL) {
		free(dev->badseg_bitmap);
		dev->badseg_bitmap = NULL;
	}
	raspberry = (struct raspberry *)dev->d_private;
	if (raspberry == NULL) {
		return 0;
	}
	if (raspberry->buffer != NULL) {
		free(raspberry->buffer);
		raspberry->buffer = NULL;
	}
	if (raspberry->is_used != NULL) {
		free(raspberry->is_used);
		raspberry->is_used = NULL;
	}
	raspberry->size = 0;
	return 0;
}

/**
 * @brief raspberry operations
 */
const struct device_operations __raspberry_dops = {
	.open = raspberry_open,
	.write = raspberry_write,
	.read = raspberry_read,
	.erase = raspberry_erase,
	.close = raspberry_close,
};

/**
 * @brief initialize the device and raspberry module
 *
 * @param dev pointer of the device structure
 * @param flags flags for raspberry and device
 *
 * @return 0 for sucess, negative value for fail
 */
int raspberry_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct raspberry *raspberry;

	(void)flags;
	raspberry = (struct raspberry *)malloc(sizeof(struct raspberry));
	if (raspberry == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	raspberry->buffer = NULL;
	raspberry->size = 0;
	dev->d_op = &__raspberry_dops;
	dev->d_private = (void *)raspberry;
	dev->d_submodule_exit = raspberry_device_exit;

	if (dev->d_private == NULL) {
		goto exception;
	}

	return ret;
exception:
	raspberry_device_exit(dev);
	return ret;
}

/**
 * @brief deallocate the device module
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int raspberry_device_exit(struct device *dev)
{
	struct raspberry *raspberry;
	raspberry = (struct raspberry *)dev->d_private;
	if (raspberry != NULL) {
		raspberry_close(dev);
		free(raspberry);
		dev->d_private = NULL;
	}
	return 0;
}
