/**
 * @file ramdisk.c
 * @brief implementation of the ramdisk which is inherited by the device
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-10-03
 */
#include <stdlib.h>
#include <stdint.h>
#include <errno.h>
#include <string.h>

#include "include/flash.h"
#include "include/ramdisk.h"
#include "include/device.h"
#include "include/log.h"
#include "include/bits.h"

/**
 * @brief open the ramdisk (allocate the device resources)
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value to fail
 */
int ramdisk_open(struct device *dev)
{
	int ret = 0;
	char *buffer;
	uint64_t bitmap_size;
	uint64_t *is_used;
	struct ramdisk *ramdisk;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	info->nr_bus = 8;
	info->nr_chips = 8;

	package->nr_blocks = 64;
	block->nr_pages = 128;
	page->size = 8192;

	ramdisk = (struct ramdisk *)dev->d_private;
	ramdisk->size = device_get_total_size(dev);

	pr_info("ramdisk generated (size: %zu bytes)\n", ramdisk->size);
	buffer = (char *)malloc(ramdisk->size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(buffer, 0, ramdisk->size);
	ramdisk->buffer = buffer;

	bitmap_size = BITS_TO_BYTES(ramdisk->size / page->size);
	is_used = (uint64_t *)malloc(bitmap_size);
	if (is_used == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	pr_info("bitmap generated (size: %zu bytes)\n", bitmap_size);
	memset(is_used, 0, bitmap_size);
	ramdisk->is_used = is_used;

	return ret;
exception:
	ramdisk_close(dev);
	return ret;
}

/**
 * @brief write to the ramdisk
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return written size (byte)
 */
ssize_t ramdisk_write(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size = device_get_page_size(dev);
	ssize_t data_size = 0;
	int is_used;

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		return -ENODATA;
	}

	if (request->flag != DEVICE_WRITE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_WRITE, request->flag);
		return -EINVAL;
	}

	if (request->data_len != page_size) {
		pr_err("data write size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		return -EINVAL;
	}

	is_used = get_bit(ramdisk->is_used, addr.lpn);
	if (is_used == 1) {
		pr_err("you overwrite the already written page\n");
		return -EINVAL;
	}
	set_bit(ramdisk->is_used, addr.lpn);
	memcpy(&ramdisk->buffer[addr.lpn * page_size], request->data,
	       request->data_len);
	data_size = request->data_len;
	if (request->end_rq) {
		request->end_rq(request);
	}
	return data_size;
}

/**
 * @brief read to the ramdisk
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return read size (byte)
 */
ssize_t ramdisk_read(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size;
	ssize_t data_size;

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		return -ENODATA;
	}

	if (request->flag != DEVICE_READ) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_READ, request->flag);
		return -EINVAL;
	}

	page_size = device_get_page_size(dev);
	if (request->data_len != page_size) {
		pr_err("data read size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		return -EINVAL;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_debug("physical address is not specified...\n");
		goto exit;
	}

	memcpy(request->data, &ramdisk->buffer[addr.lpn * page_size],
	       request->data_len);
exit:
	data_size = request->data_len;
	if (request->end_rq) {
		request->end_rq(request);
	}
	return data_size;
}

/**
 * @brief erase a segment
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return 0 for success, negative value for fail
 */
int ramdisk_erase(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size;
	size_t segnum;
	uint32_t nr_pages_per_segment;
	uint32_t lpn;

	if (request->flag != DEVICE_ERASE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_ERASE, request->flag);
		return -EINVAL;
	}

	page_size = device_get_page_size(dev);
	nr_pages_per_segment = (uint32_t)device_get_pages_per_segment(dev);
	segnum = addr.format.block;
	addr.lpn = 0;
	addr.format.block = segnum;
	for (lpn = addr.lpn; lpn < addr.lpn + nr_pages_per_segment; lpn++) {
		memset(&ramdisk->buffer[lpn * page_size], 0, page_size);
		reset_bit(ramdisk->is_used, lpn);
	}

	if (request->end_rq) {
		request->end_rq(request);
	}
	return 0;
}

/**
 * @brief close the ramdisk
 *
 * @param dev pointer of the device
 *
 * @return 0 for success ,negative value for fail
 */
int ramdisk_close(struct device *dev)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	if (ramdisk->buffer != NULL) {
		free(ramdisk->buffer);
		ramdisk->buffer = NULL;
	}
	if (ramdisk->is_used != NULL) {
		free(ramdisk->is_used);
		ramdisk->is_used = NULL;
	}
	ramdisk->size = 0;
	return 0;
}

/**
 * @brief ramdisk operations
 */
const struct device_operations __ramdisk_dops = {
	.open = ramdisk_open,
	.write = ramdisk_write,
	.read = ramdisk_read,
	.erase = ramdisk_erase,
	.close = ramdisk_close,
};

/**
 * @brief initialize the device module
 *
 * @param dev pointer of the device structure
 * @param flags flags for ramdisk and device
 *
 * @return 0 for sucess, negative value for fail
 */
int ramdisk_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct ramdisk *ramdisk;

	(void)flags;
	ramdisk = (struct ramdisk *)malloc(sizeof(struct ramdisk));
	if (ramdisk == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	ramdisk->buffer = NULL;
	ramdisk->size = 0;
	dev->d_op = &__ramdisk_dops;
	dev->d_private = (void *)ramdisk;
	dev->d_submodule_exit = ramdisk_device_exit;
	return ret;
exception:
	ramdisk_device_exit(dev);
	return ret;
}

/**
 * @brief deallocate the device module
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int ramdisk_device_exit(struct device *dev)
{
	struct ramdisk *ramdisk;
	ramdisk = (struct ramdisk *)dev->d_private;
	if (ramdisk != NULL) {
		ramdisk_close(dev);
		free(ramdisk);
		dev->d_private = NULL;
	}
	return 0;
}
