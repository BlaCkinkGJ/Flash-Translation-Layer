/**
 * @file device.c
 * @brief implementation of the device
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-10-01
 */
#include "include/device.h"
#include "include/log.h"
#include "include/ramdisk.h"
#include "include/bluedbm.h"

#include <stdlib.h>
#include <assert.h>
#include <errno.h>

static int (*submodule_init[])(struct device *, uint64_t) = {
	/* [RAMDISK_MODULE] = */ ramdisk_device_init,
	/* [BULEDBM_MODULE] = */ bluedbm_device_init,
};

int device_module_init(const uint64_t modnum, struct device **__dev,
		       uint64_t flags)
{
	int ret;
	struct device *dev;
	dev = (struct device *)malloc(sizeof(struct device));
	if (dev == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	dev->d_private = NULL;
	dev->d_op = NULL;
	dev->d_submodule_exit = NULL;
	(void)flags;
	ret = submodule_init[modnum](dev, flags);
	if (ret) {
		pr_err("initialize the submodule failed(modnum:%lu)\n", modnum);
		goto exception;
	}

	*__dev = dev;
	return ret;
exception:
	device_module_exit(dev);
	return ret;
}

int device_module_exit(struct device *dev)
{
	int ret = 0;
	assert(NULL != dev);
	if (dev->d_submodule_exit) {
		dev->d_submodule_exit(dev);
		dev->d_submodule_exit = NULL;
	}
	free(dev);
	return ret;
}
