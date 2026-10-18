/**
 * @file module.h
 * @brief creation and deletion of the module's interface
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-09-22
 */
#ifndef MODULE_H
#define MODULE_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>

#include "flash.h"

enum {
	PAGE_FTL_MODULE = 0 /**< page FTL number*/,
};

/*
 * Task 16: `src/module.rs` is the only implementation, and the submodule table
 * `interface/module.c` held lives there. The pair below is the whole C surface
 * of the module layer.
 */
int ffi_module_init(const int modnum, struct flash_device **, uint64_t flags);
int ffi_module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif
#endif
