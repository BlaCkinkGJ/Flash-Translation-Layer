/**
 * @file ramdisk.c
 * @brief implementation of the ramdisk which is inherited by the device
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-03
 */
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <stdint.h>
#include <errno.h>
#include <string.h>
#include <stringlib.h>
#include <unistd.h>

#include "include/flash.h"
#include "include/ramdisk.h"
#include "include/device.h"
#include "include/log.h"
#include "include/bits.h"

/**
 * @brief open the ramdisk (allocate the device resources)
 *
 * @param dev pointer of the device structure
 * @param name this does not use in this module
 * @param flags open flags for ramdisk
 *
 * @return 0 for success, negative value to fail
 */
int ramdisk_open(struct device *dev, const char *name, int flags)
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

	size_t nr_segments;

	(void)name;
	(void)flags;

	info->nr_bus = (1 << DEVICE_NR_BUS_BITS);
	info->nr_chips = (1 << DEVICE_NR_CHIPS_BITS);

	package->nr_blocks = 512; /**< This for make 4GiB disk */
	block->nr_pages = (1 << DEVICE_NR_PAGES_BITS);
	page->size = DEVICE_PAGE_SIZE;

	ramdisk = (struct ramdisk *)dev->d_private;
	ramdisk->size = device_get_total_size(dev);
	ramdisk->o_flags = flags;

	pr_info("ramdisk generated (size: %zu bytes)\n", ramdisk->size);
	buffer = (char *)malloc(ramdisk->size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	__memset_aarch64(buffer, 0, ramdisk->size);
	ramdisk->buffer = buffer;

	bitmap_size = BITS_TO_UINT64_ALIGN(ramdisk->size / page->size);
	is_used = (uint64_t *)malloc(bitmap_size);
	if (is_used == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	pr_info("bitmap generated (size: %zu bytes)\n", bitmap_size);
	__memset_aarch64(is_used, 0, bitmap_size);
	ramdisk->is_used = is_used;

	nr_segments = device_get_nr_segments(dev);
	dev->badseg_bitmap =
		(uint64_t *)malloc(BITS_TO_UINT64_ALIGN(nr_segments));
	if (dev->badseg_bitmap == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	__memset_aarch64(dev->badseg_bitmap, 0,
			 BITS_TO_UINT64_ALIGN(nr_segments));
	for (uint64_t i = 0; i < 10; i++) {
		set_bit(dev->badseg_bitmap, i);
	}
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
 * @return written size (bytes)
 */
ssize_t ramdisk_write(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size = device_get_page_size(dev);
	ssize_t ret = 0;
	int is_used;

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

	is_used = get_bit(ramdisk->is_used, addr.lpn);
	if (is_used == 1) {
		pr_err("you overwrite the already written page\n");
		ret = -EINVAL;
		goto exit;
	}
	set_bit(ramdisk->is_used, addr.lpn);
	__memcpy_aarch64_simd(&ramdisk->buffer[addr.lpn * page_size],
			      request->data, request->data_len);
	ret = request->data_len;
	if (request->end_rq) {
		request->end_rq(request);
	}
exit:
	return ret;
}

/**
 * @brief read from the ramdisk
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return read size (bytes)
 */
ssize_t ramdisk_read(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size;
	ssize_t ret;

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

	__memcpy_aarch64_simd(request->data,
			      &ramdisk->buffer[addr.lpn * page_size],
			      request->data_len);
	ret = request->data_len;
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
int ramdisk_erase(struct device *dev, struct device_request *request)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	struct device_address addr = request->paddr;
	size_t page_size;
	size_t segnum;
	uint32_t nr_pages_per_segment;
	uint32_t lpn;
	int ret;

	ret = 0;

	if (request->flag != DEVICE_ERASE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_ERASE, request->flag);
		ret = -EINVAL;
		goto exit;
	}

	page_size = device_get_page_size(dev);
	nr_pages_per_segment = (uint32_t)device_get_pages_per_segment(dev);
	segnum = addr.format.block;
	addr.lpn = 0;
	addr.format.block = segnum;
	for (lpn = addr.lpn; lpn < addr.lpn + nr_pages_per_segment; lpn++) {
		__memset_aarch64(&ramdisk->buffer[lpn * page_size], 0,
				 page_size);
		reset_bit(ramdisk->is_used, lpn);
	}

	if (request->end_rq) {
		request->end_rq(request);
	}
exit:
	return ret;
}

/**
 * @brief close the ramdisk
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int ramdisk_close(struct device *dev)
{
	struct ramdisk *ramdisk;
	if (dev->badseg_bitmap != NULL) {
		free(dev->badseg_bitmap);
		dev->badseg_bitmap = NULL;
	}
	ramdisk = (struct ramdisk *)dev->d_private;
	if (ramdisk == NULL) {
		return 0;
	}
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
 * @brief initialize the device and ramdisk module
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
