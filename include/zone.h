/**
 * @file zone.h
 * @brief zbd's header file
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-09
 */
#ifndef ZONE_H
#define ZONE_H

#include <stdint.h>

#include "device.h"

/*
 * Task 108: `device/zone/zone.c` is gone, so its prototypes, the
 * `struct zone_meta` they were written against and the libzbd include they
 * needed go with it. `src/device/zone.rs` keeps its own state and binds libzbd
 * by hand, and the init and exit routines below are what the device module
 * installs.
 */
int ffi_zone_device_init(struct device *, uint64_t flags);
int ffi_zone_device_exit(struct device *);

#endif
