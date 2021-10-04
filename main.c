/**
 * @file main.c
 * @brief main program for test the interface
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <assert.h>
#include <unistd.h>
#include <string.h>

#include "include/module.h"
#include "include/flash.h"
#include "include/page.h"
#include "include/log.h"
#include "include/device.h"

int main(void)
{
	struct flash_device *flash = NULL;
	char buffer[8192];
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE));
	pr_info("module initialize\n");
	flash->f_op->open(flash);
	for (int i = 0; i < 8192 * 10; i++) {
		int num;
		size_t sector;
		num = i * 2;
		memset(buffer, 0, 8192);
		*(int *)buffer = num;
		sector = rand() % (1 << 31);
		flash->f_op->write(flash, buffer, sizeof(int), sector);
		pr_info("write value: %d\n", *(int *)buffer);
		memset(buffer, 0, 8192);
		flash->f_op->read(flash, buffer, sizeof(int), sector);
		pr_info("read value: %d\n", *(int *)buffer);
		if (i % 8192 * 5 == 0) {
			flash->f_op->ioctl(flash, PAGE_FTL_IOCTL_TRIM);
		}
	}
	flash->f_op->close(flash);
	assert(0 == module_exit(flash));
	pr_info("module deallcation\n");

	return 0;
}
