/**
 * @file ramdisk.h
 * @brief ramdisk's header file
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-03
 */
#ifndef RAMDISK_H
#define RAMDISK_H

#include <stdint.h>

#include "device.h"

/*
 * Task 16a: `device/ramdisk/ramdisk.c` is gone, so its prototypes went with
 * it. So did the `struct ramdisk` they were written against, and the
 * `stdlib.h` / `sys/time.h` includes it needed on its own.
 * `src/device/ramdisk.rs` keeps its own state, and nothing outside the C
 * source ever read the struct. The init and exit routines below are what the
 * device module installs.
 */
#ifdef __cplusplus
extern "C" {
#endif

int ffi_ramdisk_device_init(struct device *, uint64_t flags);
int ffi_ramdisk_device_exit(struct device *);

#ifdef __cplusplus
}
#endif

#endif
