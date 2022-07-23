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

#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <errno.h>
#include <inttypes.h>
#include <pthread.h>

/**
 * @brief initialize the submodule
 *
 * @param device pointer of the device structure
 * @param uint64_t contain the flag information
 */
static int (*submodule_init[])(struct device *, uint64_t) = {
	/* [RAMDISK_MODULE] = */ ramdisk_device_init,
#ifdef DEVICE_USE_BLUEDBM
	/* [BULEDBM_MODULE] = */ bluedbm_device_init,
#else
	/* [BULEDBM_MODULE] = */ NULL,
#endif
#ifdef DEVICE_USE_ZONED
	/* [ZONE_MODULE] = */ zone_device_init,
#else
	/* [ZONE_MODULE] = */ NULL,
#endif
};

/**
 * @brief dynamic allocate the device request
 *
 * @param flags flags for allocate the device request
 *
 * @return device_request pointer when it is allocated or NULL when it is not allocated
 */
struct device_request *device_alloc_request(uint64_t flags)
{
	struct device_request *request;
	int ret = 0;
	(void)flags;

	request =
		(struct device_request *)malloc(sizeof(struct device_request));
	if (request == NULL) {
		pr_err("request allocation failed\n");
		return NULL;
	}
	memset(request, 0, sizeof(struct device_request));
	ret = pthread_mutex_init(&request->mutex, NULL);
	if (ret) {
		pr_err("pthread mutex initialize failed\n");
		errno = ret;
		return NULL;
	}

	ret = pthread_cond_init(&request->cond, NULL);
	if (ret) {
		pr_err("pthread conditional variable initialize failed\n");
		errno = ret;
		return NULL;
	}

	g_atomic_int_set(&request->is_finish, 0);

	return request;
}

/**
 * @brief free pre-allocated device_request resource
 *
 * @param request pointer of the device request
 */
void device_free_request(struct device_request *request)
{
	pthread_cond_destroy(&request->cond);
	pthread_mutex_destroy(&request->mutex);
	free(request);
}

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
