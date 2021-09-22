/**
 * @file main.c
 * @brief main program for test the interface
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <assert.h>

#include "include/module.h"
#include "include/flash.h"

int main(void)
{
	struct flash_device *flash = NULL;
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, FLASH_DEFAULT_FLAG));
	assert(0 == module_exit(flash));
	return 0;
}
