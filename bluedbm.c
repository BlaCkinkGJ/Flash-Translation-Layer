#include <stdlib.h>
#include <errno.h>

#include "include/bluedbm.h"
#include "include/log.h"

static int bluedbm_open(struct device *dev)
{
	struct bluedbm *bdbm;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	info->nr_bus = 8;
	info->nr_chips = 8;

	package->nr_blocks = 4096; /**< 8192 does not support yet */
	block->nr_pages = 128;
	page->size = 8192;

	bdbm = (struct bluedbm *)dev->d_private;
	bdbm->size = device_get_total_size(dev);

	return 0;
}

static ssize_t bluedbm_write(struct device *dev, struct device_address addr,
			     void *buffer)
{
	(void)dev;
	(void)addr;
	(void)buffer;
	return 0;
}

static ssize_t bluedbm_read(struct device *dev, struct device_address addr,
			    void *buffer)
{
	(void)dev;
	(void)addr;
	(void)buffer;
	return 0;
}

static ssize_t bluedbm_erase(struct device *dev, struct device_address addr)
{
	(void)dev;
	(void)addr;
	return 0;
}

static int bluedbm_close(struct device *dev)
{
	(void)dev;
	return 0;
}

const struct device_operations __bluedbm_dops = {
	.open = bluedbm_open,
	.write = bluedbm_write,
	.read = bluedbm_read,
	.erase = bluedbm_erase,
	.close = bluedbm_close,
};

int bluedbm_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct bluedbm *bdbm;

	(void)flags;
	bdbm = (struct bluedbm *)malloc(sizeof(struct bluedbm));
	if (bdbm == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	dev->d_op = &__bluedbm_dops;
	dev->d_private = (void *)bdbm;
	dev->d_submodule_exit = bluedbm_device_exit;
	return ret;
exception:
	bluedbm_device_exit(dev);
	return ret;
}

int bluedbm_device_exit(struct device *dev)
{
	struct bluedbm *bdbm;
	bdbm = (struct bluedbm *)dev->d_private;
	if (bdbm) {
		free(bdbm);
		dev->d_private = NULL;
	}
	return 0;
}
