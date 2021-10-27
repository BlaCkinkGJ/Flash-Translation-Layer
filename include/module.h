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

enum { PAGE_FTL_MODULE = 0 /**< page FTL number*/,
};

int module_init(const int modnum, struct flash_device **, uint64_t flags);
int module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif
#endif
