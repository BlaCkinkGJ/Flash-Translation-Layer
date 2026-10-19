/**
 * @file bluedbm.h
 * @brief bluedbm module's header file
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-20
 */
#ifndef BLUEDBM_H
#define BLUEDBM_H

#include <stdint.h>

#include "device.h"

/*
 * The BlueDBM backend is Rust: `src/device/bluedbm.rs` owns it and
 * `device/bluedbm/bluedbm.c` is gone. The two `ffi_` entry points below are
 * what `src/device/mod.rs` installs, and the backend reaches libmemio through
 * the hand-written binding in that module.
 */
int ffi_bluedbm_device_init(struct device *, uint64_t flags);
int ffi_bluedbm_device_exit(struct device *);

#endif
