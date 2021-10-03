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

#include "include/ramdisk.h"
#include "include/device.h"
#include "include/log.h"

static int ramdisk_open(struct device *dev)
{
	int ret = 0;
	char *buffer;
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

	buffer = (char *)malloc(ramdisk->size);
	if (buffer == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(buffer, 0, ramdisk->size);
	ramdisk->buffer = buffer;

	return ret;
exception:
	return ret;
}

static ssize_t ramdisk_write(struct device *dev, struct device_address addr,
			     void *buffer)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	size_t page_size = device_get_page_size(dev);
	memcpy(&ramdisk->buffer[addr.lpn], buffer, page_size);
	return 0;
}

static ssize_t ramdisk_read(struct device *dev, struct device_address addr,
			    void *buffer)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	size_t page_size = device_get_page_size(dev);
	memcpy(buffer, &ramdisk->buffer[addr.lpn], page_size);
	return 0;
}

static ssize_t ramdisk_erase(struct device *dev, struct device_address addr)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	size_t page_size;
	uint32_t nr_pages_per_segment;
	uint32_t lpn;

	page_size = device_get_page_size(dev);
	nr_pages_per_segment = (uint32_t)device_get_pages_per_segment(dev);
	for (lpn = addr.lpn; lpn < addr.lpn + nr_pages_per_segment; lpn++) {
		memset(&ramdisk->buffer[lpn], 0, page_size);
	}
	return 0;
}

static int ramdisk_close(struct device *dev)
{
	struct ramdisk *ramdisk = (struct ramdisk *)dev->d_private;
	if (ramdisk->buffer != NULL) {
		free(ramdisk->buffer);
		ramdisk->buffer = NULL;
	}
	ramdisk->size = 0;
	return 0;
}

const struct device_operations __ramdisk_dops = {
	.open = ramdisk_open,
	.write = ramdisk_write,
	.read = ramdisk_read,
	.erase = ramdisk_erase,
	.close = ramdisk_close,
};

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
