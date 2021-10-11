/**
 * @file main.c
 * @brief main program for test the interface
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <assert.h>
#include <stdio.h>
#include <unistd.h>
#include <string.h>
#include <errno.h>

#include "include/module.h"
#include "include/flash.h"
#include "include/page.h"
#include "include/log.h"
#include "include/device.h"

#define WRITE_SIZE (8192 * 8192)
#define NR_ERASE (10)
#define COUNT_SIZE (8192)

void *read_thread(void *data)
{
	char buffer[8192];
	struct flash_device *flash;
	size_t sector;
	ssize_t ret;

	sector = 0;
	flash = (struct flash_device *)data;

	while (sector < WRITE_SIZE) {
		srand(time(NULL));
		memset(buffer, 0, 8192);
		ret = flash->f_op->read(flash, buffer, COUNT_SIZE, sector);
		if (ret < 0) {
			continue;
		}
		pr_info("read value: %d(sector: %lu)\n", *(int *)buffer,
			sector);
		sector += COUNT_SIZE;
		usleep(((rand() % 10) + 10) * 1000);
	}
	return NULL;
}

void *write_thread(void *data)
{
	char buffer[8192];
	struct flash_device *flash;
	size_t sector;
	ssize_t ret;

	sector = 0;
	flash = (struct flash_device *)data;

	while (sector < WRITE_SIZE) {
		srand(time(NULL));
		memset(buffer, 0, sizeof(buffer));
		*(int *)buffer = (int)sector;
		ret = flash->f_op->write(flash, buffer, COUNT_SIZE, sector);
		if (ret < 0) {
			pr_err("write failed (sector: %zu)\n", sector);
		}
		sector += COUNT_SIZE;
		usleep((rand() % 10) * 1000);
	}
	sector = 0;
	while (sector < WRITE_SIZE) {
		srand(time(NULL));
		memset(buffer, 0, sizeof(buffer));
		*(int *)buffer = (int)sector;
		ret = flash->f_op->write(flash, buffer, COUNT_SIZE, sector);
		if (ret < 0) {
			pr_err("write failed (sector: %zu)\n", sector);
		}
		sector += COUNT_SIZE;
		usleep((rand() % 10) * 1000);
	}

	return NULL;
}

void *erase_thread(void *data)
{
	struct flash_device *flash;
	int i;
	flash = (struct flash_device *)data;
	for (i = 0; i < NR_ERASE; i++) {
		flash->f_op->ioctl(flash, PAGE_FTL_IOCTL_TRIM);
		usleep(5000 * 1000); // 5s
	}

	return NULL;
}

int main(void)
{
	pthread_t threads[3]; // read, write, erase;
	int thread_id;
	size_t status;
	struct flash_device *flash = NULL;
#ifdef DEVICE_USE_ZONED
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, ZONE_MODULE));
#else
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE));
#endif
	flash->f_op->open(flash, "/dev/nvme0n2");
	thread_id =
		pthread_create(&threads[0], NULL, write_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[1], NULL, read_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[2], NULL, erase_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}

	pthread_join(threads[0], (void **)&status);
	pthread_join(threads[1], (void **)&status);
	pthread_join(threads[2], (void **)&status);

	flash->f_op->close(flash);
	assert(0 == module_exit(flash));

	return 0;
}
