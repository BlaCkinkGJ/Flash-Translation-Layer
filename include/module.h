/**
 * @file module.h
 * @brief creation and deletion of the module's interface
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#ifndef MODULE_H
#define MODULE_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdio.h>
#include <stdint.h>

#include "flash.h"

#define pr_info(format, ...)                                                   \
	fprintf(stdout,                                                        \
		"[" __FILE__ ":%s"                                             \
		"(%d)] " format,                                               \
		__func__, __LINE__, __VA_ARGS__)

enum { PAGE_FTL_MODULE = 0 /**< page FTL number*/,
};

int module_init(const int modnum, struct flash_device **, uint64_t flags);
int module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif
#endif
