/**
 * @file raspberry.h
 * @brief raspberry's header file
 * @author Gijun Oh
 * @date 2023-05-29
 */
#ifndef RASPBERRY_H
#define RASPBERRY_H

#include <stdint.h>
#include <stdlib.h>
#include <glib.h>
#include <sys/time.h>

#include "device.h"

/**
 * @brief structure for manage the raspberry
 */
struct raspberry {
	size_t size;
	char *buffer;
	uint64_t *is_used;
	int o_flags;
};

int raspberry_open(struct device *, const char *name, int flags);
ssize_t raspberry_write(struct device *, struct device_request *);
ssize_t raspberry_read(struct device *, struct device_request *);
int raspberry_erase(struct device *, struct device_request *);
int raspberry_close(struct device *);

int raspberry_device_init(struct device *, uint64_t flags);
int raspberry_device_exit(struct device *);

#endif
