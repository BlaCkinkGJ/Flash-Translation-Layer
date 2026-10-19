/**
 * @file device.c
 * @brief implementation of the device
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-01
 */
#include "device.h"
#include "log.h"
#include "ramdisk.h"
#ifdef DEVICE_USE_BLUEDBM
#include "bluedbm.h"
#endif
#ifdef DEVICE_USE_ZONED
#include "zone.h"
#endif
#ifdef DEVICE_USE_RASPBERRY
#include "raspberry.h"
#endif

#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <errno.h>
#include <inttypes.h>
#include <pthread.h>

/*
 * Task 14 cutover: the Rust `SUBMODULE_INIT` table in `src/device/mod.rs` owns
 * the backend init routines, and this table points at the same `ffi_` entry
 * points so that deleting a backend's C source leaves no caller behind here.
 *
 * Every entry is now the ported one: the ramdisk backend and the zoned (#108),
 * raspberry (#115) and bluedbm (#109) ones. No backend has a C implementation
 * left, and `src/device/mod.rs` wires the same four entry points.
 *
 * Task 16a: the C request allocators (`device_alloc_request`,
 * `device_free_request`) went with the `ftl/page` sources, their last callers.
 * `ffi_device_alloc_request` / `ffi_device_free_request` in `src/device/mod.rs`
 * are the live pair.
 */
static int (*submodule_init[])(struct device *, uint64_t) = {
	/* [RAMDISK_MODULE] = */ ffi_ramdisk_device_init,
#ifdef DEVICE_USE_BLUEDBM
	/* [BULEDBM_MODULE] = */ ffi_bluedbm_device_init,
#else
	/* [BULEDBM_MODULE] = */ NULL,
#endif
#ifdef DEVICE_USE_ZONED
	/* [ZONE_MODULE] = */ ffi_zone_device_init,
#else
	/* [ZONE_MODULE] = */ NULL,
#endif
#ifdef DEVICE_USE_RASPBERRY
	/* [RASPBERRY_MODULE] = */ ffi_raspberry_device_init,
#else
	/* [RASPBERRY_MODULE] = */ NULL,
#endif
};

/**
 * @brief initialize the device module
 *
 * @param modnum module's number
 * @param __dev device structure pointer (will be allocated)
 * @param flags initializing flag
 *
 * @return 0 for success, negative value for fail
 */
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
	memset(dev, 0, sizeof(struct device));
	pthread_mutex_init(&dev->mutex, NULL);
	(void)flags;
	ret = submodule_init[modnum](dev, flags);
	if (ret) {
		pr_err("initialize the submodule failed(modnum:%" PRIu64 ")\n",
		       modnum);
		goto exception;
	}

	dev->badseg_bitmap = NULL;

	*__dev = dev;
	return ret;
exception:
	device_module_exit(dev);
	return ret;
}

/**
 * @brief deallocate the device module
 *
 * @param dev pointer of the device module's structure
 *
 * @return 0 for success, negative value for fail
 */
int device_module_exit(struct device *dev)
{
	int ret = 0;
	assert(NULL != dev);
	if (dev->d_submodule_exit) {
		dev->d_submodule_exit(dev);
		dev->d_submodule_exit = NULL;
	}
	pthread_mutex_destroy(&dev->mutex);
	free(dev);
	return ret;
}
