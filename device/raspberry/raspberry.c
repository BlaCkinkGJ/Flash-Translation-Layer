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
#include "nand.h"

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
	int ret = 0, i;
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

	// TODO: this part will be removed after implmenting recovery routine
	for (i = 0; (size_t)i < package->nr_blocks; i++) {
		int status;
		pthread_spin_lock(&raspberry->lock);
		status = nand_erase(i);
		pthread_spin_unlock(&raspberry->lock);
		if (status) {
			set_bit(dev->badseg_bitmap, (uint64_t)i);
		}
	}

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
	struct device_address addr;
	size_t page_size = device_get_page_size(dev);
	ssize_t ret;
	struct raspberry *raspberry;

	addr.lpn = 0;
	addr.raspberry_converter.bus = request->paddr.format.bus;
	addr.raspberry_converter.chip = request->paddr.format.chip;
	addr.raspberry_converter.block = request->paddr.format.block;
	addr.raspberry_converter.page = request->paddr.format.page;

	ret = 0;

	raspberry = (struct raspberry *)dev->d_private;

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

	pthread_spin_lock(&raspberry->lock);
	ret = nand_write((char *)request->data, addr.raspberry.block,
			 addr.raspberry.page);
	pthread_spin_unlock(&raspberry->lock);
	if (ret) {
		pr_err("write error detected %zu\n", ret);
		goto exit;
	}
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
	struct device_address addr;
	size_t page_size;
	ssize_t ret;
	struct raspberry *raspberry;

	addr.lpn = 0;
	addr.raspberry_converter.bus = request->paddr.format.bus;
	addr.raspberry_converter.chip = request->paddr.format.chip;
	addr.raspberry_converter.block = request->paddr.format.block;
	addr.raspberry_converter.page = request->paddr.format.page;

	ret = 0;

	raspberry = (struct raspberry *)dev->d_private;

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

	pthread_spin_lock(&raspberry->lock);
	ret = nand_read((char *)request->data, addr.raspberry.block,
			addr.raspberry.page);
	pthread_spin_unlock(&raspberry->lock);
	if (ret) {
		pr_warn("read error detected %s\n",
			nand_get_read_error_msg((int)ret));
		goto exit;
	}
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
	uint16_t segnum;
	int ret;
	struct raspberry *raspberry;

	ret = 0;
	raspberry = (struct raspberry *)dev->d_private;

	if (request->flag != DEVICE_ERASE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_ERASE, request->flag);
		ret = -EINVAL;
		goto exit;
	}
	segnum = (uint16_t)request->paddr.format.block;

	pthread_spin_lock(&raspberry->lock);
	ret = nand_erase(segnum);
	pthread_spin_unlock(&raspberry->lock);
	if (ret) {
		pr_warn("erase fail detected %d\n", ret);
		set_bit(dev->badseg_bitmap, segnum);
		goto exit;
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
	raspberry->size = 0;
	dev->d_op = &__raspberry_dops;
	dev->d_private = (void *)raspberry;
	dev->d_submodule_exit = raspberry_device_exit;

	if (dev->d_private == NULL) {
		goto exception;
	}

	ret = nand_init();
	if (ret) {
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
	nand_free();
	return 0;
}
