/**
 * @file ramdisk.h
 * @brief ramdisk's header file
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-10-03
 */
#ifndef RAMDISK_H
#define RAMDISK_H

#include <stdint.h>
#include <stdlib.h>

#include "device.h"

struct ramdisk {
	size_t size;
	char *buffer;
};

int ramdisk_device_init(struct device *, uint64_t flags);
int ramdisk_device_exit(struct device *);

#endif
