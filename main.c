/**
 * @file main.c
 * @brief main program for test the interface
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <assert.h>
#include <unistd.h>

#include "include/module.h"
#include "include/flash.h"
#include "include/log.h"
#include "include/device.h"

int main(void)
{
	struct flash_device *flash = NULL;
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE));
	pr_info("module initialize\n");
	flash->f_op->open(flash);
	flash->f_op->close(flash);
	assert(0 == module_exit(flash));
	pr_info("module deallcation\n");

	return 0;
}
