/**
 * @file module.c
 * @brief this deal the initialization and removing of the module
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <assert.h>

#include "include/flash.h"
#include "include/module.h"
#include "include/page.h"
#include "include/log.h"

/**
 * @brief submodule list table
 *
 * @param flash pointer of the flash device information
 * @param int flags for flash module
 *
 * @note
 * You must follow the submodule index in the `include/module.h`
 */
static int (*submodule_init[])(struct flash_device *, uint64_t) = {
	/* [PAGE_FTL_MODULE] = */ page_ftl_module_init,
};

/**
 * @brief generic initializer for initialize the module
 *
 * @param modnum module number described in the `include/module.h`
 * @param __flash double pointer of the flash device information
 * @param flags flags for flash and submodule
 *
 * @return zero to success, error number to fail
 *
 * @note
 * This function allocates the memory to the __flash.
 * And you must not change this function!!
 */
int module_init(const int modnum, struct flash_device **__flash, uint64_t flags)
{
	int err;
	err = flash_module_init(__flash, flags);
	if (err) {
		pr_err("flash initialize failed\n");
		return err;
	}
	err = submodule_init[modnum](*__flash, flags);
	if (err) {
		pr_err("submodule initialize failed\n");
		return err;
	}
	return 0;
}

/**
 * @brief free resources in the flash module and submodule
 *
 * @param flash pointer of the flash device information
 *
 * @return zero to success, error number to fail
 */
int module_exit(struct flash_device *flash)
{
	int err;
	if (flash->f_submodule_exit) {
		err = flash->f_submodule_exit(flash);
		if (err) {
			pr_err("submodule resources free failed\n");
			return err;
		}
	}

	err = flash_module_exit(flash);
	if (err) {
		pr_err("flash resources free failed\n");
		return err;
	}
	return 0;
}
