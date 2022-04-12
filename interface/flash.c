/**
 * @file flash.c
 * @brief generic flash control interfaces
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-09-22
 */
#include <stdlib.h>
#include <errno.h>

#include "module.h"
#include "flash.h"
#include "log.h"

/**
 * @brief initialize the flash module
 *
 * @param __flash double pointer of the flash device information
 * @param flags flags for flash module and submodule
 *
 * @return zero to success, error number to fail
 *
 * @note
 * This function allocates the memory to the __flash
 */
int flash_module_init(struct flash_device **__flash, uint64_t flags)
{
	int err;
	struct flash_device *flash;

	(void)flags;

	flash = (struct flash_device *)malloc(sizeof(struct flash_device));
	if (flash == NULL) {
		err = -errno;
		pr_err("fail to allocate the flash information pointer\n");
		goto exception;
	}
	flash->f_op = NULL;
	flash->f_private = NULL;
	flash->f_submodule_exit = NULL;

	*__flash = flash;

	return 0;

exception:
	flash_module_exit(flash);
	return err;
}

/**
 * @brief free resources in the flash module
 *
 * @param flash pointer of the flash device information
 *
 * @return zero to success, error number to fail
 */
int flash_module_exit(struct flash_device *flash)
{
	if (flash == NULL) {
		pr_err("flash pointer is null detected\n");
		return 0;
	}

	free(flash);
	return 0;
}
