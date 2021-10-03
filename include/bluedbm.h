#ifndef BLUEDBM_H
#define BLUEDBM_H

#include <stdint.h>

#include "device.h"

struct bluedbm {
	size_t size;
};

int bluedbm_device_init(struct device *, uint64_t flags);
int bluedbm_device_exit(struct device *);

#endif
