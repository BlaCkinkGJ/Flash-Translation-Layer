/**
 * @file zone.h
 * @brief zbd's header file
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-10-09
 */
#ifndef ZONE_H
#define ZONE_H

#include <libzbd/zbd.h>

#include "device.h"

struct zone_file_descriptor {
	int fd;
};

struct zone_meta {
	size_t total_size;
	uint64_t zone_size;
	uint64_t nr_zones;
	uint64_t block_size;

	char *buffer;

	struct zone_file_descriptor read;
	struct zone_file_descriptor write;
	struct zbd_info info;
	struct zbd_zone *zones;
};

int zone_open(struct device *, const char *name);
ssize_t zone_write(struct device *, struct device_request *);
ssize_t zone_read(struct device *, struct device_request *);
int zone_erase(struct device *, struct device_request *);
int zone_close(struct device *);

int zone_device_init(struct device *, uint64_t flags);
int zone_device_exit(struct device *);

static inline uint64_t zone_get_zone_number(struct device *dev,
					    struct device_address paddr)
{
	struct zone_meta *meta = (struct zone_meta *)dev->d_private;
	size_t page_size = device_get_page_size(dev);
	return paddr.lpn / (meta->zone_size / page_size);
}

#endif
