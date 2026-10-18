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
#include <sys/time.h>
#include <pthread.h>

#include "device.h"

/**
 * @brief structure for manage the raspberry
 */
struct raspberry {
	size_t size;
	int o_flags;
	pthread_spinlock_t lock;
};

int raspberry_open(struct device *, const char *name, int flags);
ssize_t raspberry_write(struct device *, struct device_request *);
ssize_t raspberry_read(struct device *, struct device_request *);
int raspberry_erase(struct device *, struct device_request *);
int raspberry_close(struct device *);

int raspberry_device_init(struct device *, uint64_t flags);
int raspberry_device_exit(struct device *);

/*
 * Task 14 cutover: the Rust backend stub in `src/device/raspberry.rs` is what
 * `submodule_init[]` installs when `USE_RASPBERRY_DEVICE=1` enables the cargo
 * `raspberry` feature. The C prototypes above stay only as long as
 * `device/raspberry/raspberry.c` is still compiled.
 */
int ffi_raspberry_device_init(struct device *, uint64_t flags);
int ffi_raspberry_device_exit(struct device *);

#endif
