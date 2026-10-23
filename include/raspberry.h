/**
 * @file raspberry.h
 * @brief raspberry's header file
 * @author Gijun Oh
 * @date 2023-05-29
 */
#ifndef RASPBERRY_H
#define RASPBERRY_H

#include <stdint.h>

#include "device.h"

/*
 * Task 14 cutover: `device/raspberry/raspberry.c` is gone, so its prototypes
 * went with it, and so did the `struct raspberry` they were written against.
 * `src/device/raspberry.rs` keeps its own state, which no C source ever read.
 * The init and exit routines below are what the device module installs.
 */
#ifdef __cplusplus
extern "C" {
#endif

int ffi_raspberry_device_init(struct device *, uint64_t flags);
int ffi_raspberry_device_exit(struct device *);

#ifdef __cplusplus
}
#endif

#endif
