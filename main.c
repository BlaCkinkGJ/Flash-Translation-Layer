/**
 * @file main.c
 * @brief main program for test the interface
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-09-22
 */
#include <assert.h>
#include <climits>
#include <stdio.h>
#include <unistd.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <glib.h>

#include "include/module.h"
#include "include/flash.h"
#include "include/page.h"
#include "include/log.h"
#include "include/device.h"

// #define USE_FORCE_ERASE
#define USE_RANDOM_WAIT

// #define SEQ_WORKLOAD
#define RAND_WORKLOAD

#define DEVICE_PATH "/dev/nvme0n2"
#define WRITE_SIZE ((size_t)8192 * 8192)
#define NR_ERASE (10)
#if defined(RAND_WORKLOAD)
#define BLOCK_SIZE ((size_t)4096) // 4 KB
#elif defined(SEQ_WORKLOAD)
#define BLOCK_SIZE ((size_t)1024 * 1024) // 4 KB
#endif

int is_check[WRITE_SIZE / BLOCK_SIZE];

void *read_thread(void *data)
{
	char buffer[BLOCK_SIZE];
	struct flash_device *flash;
	size_t sector;
	ssize_t ret;

	sector = 0;
	flash = (struct flash_device *)data;

	while (sector < WRITE_SIZE) {
		srand((time(NULL) * sector) % UINT_MAX);
		memset(buffer, 0, sizeof(buffer));
		ret = flash->f_op->read(flash, buffer, BLOCK_SIZE, sector);
		if (ret < 0 || (sector > 0 && *(int *)buffer == 0)) {
			continue;
		}
		assert(ret == BLOCK_SIZE);
		printf("%-12s: %-16d(sector: %lu)\n", "read", *(int *)buffer,
		       sector);
		is_check[*(int *)buffer / BLOCK_SIZE] = 1;
		sector += BLOCK_SIZE;
#ifdef USE_RANDOM_WAIT
		usleep((rand() % 500) + 100);
#endif
	}
	return NULL;
}

void *write_thread(void *data)
{
	char buffer[BLOCK_SIZE];
	struct flash_device *flash;
	size_t sector;
	ssize_t ret;

	sector = 0;
	flash = (struct flash_device *)data;

	while (sector < WRITE_SIZE) {
		srand((time(NULL) * sector + 1) % UINT_MAX);
		memset(buffer, 0, sizeof(buffer));
		*(int *)buffer = (int)sector;
		ret = flash->f_op->write(flash, buffer, BLOCK_SIZE, sector);
		if (ret < 0) {
			pr_err("write failed (sector: %zu)\n", sector);
		}
		printf("%-12s: %-16d(sector: %lu)\n", "write", *(int *)buffer,
		       sector);
		assert(ret == BLOCK_SIZE);
		sector += BLOCK_SIZE;
#ifdef USE_RANDOM_WAIT
		usleep((rand() % 500) + 100);
#endif
	}
	return NULL;
}

gint is_overwrite = 0;

void *overwrite_thread(void *data)
{
	char buffer[BLOCK_SIZE];
	struct flash_device *flash;
	size_t sector;
	ssize_t ret;

	sector = 0;
	flash = (struct flash_device *)data;

	g_atomic_int_set(&is_overwrite, 1);

	sleep(2);
	while (sector < WRITE_SIZE) {
		srand((time(NULL) * sector + 2) % UINT_MAX);
		memset(buffer, 0, sizeof(buffer));
		*(int *)buffer = (int)sector;
		ret = flash->f_op->write(flash, buffer, BLOCK_SIZE, sector);
		if (ret < 0) {
			pr_err("overwrite failed (sector: %zu)\n", sector);
		}
		printf("%-12s: %-16d(sector: %lu)\n", "overwrite",
		       *(int *)buffer, sector);
		sector += BLOCK_SIZE;
#ifdef USE_RANDOM_WAIT
		usleep((rand() % 500) + 100);
#endif
	}
	return NULL;
}

void *erase_thread(void *data)
{
#ifdef USE_FORCE_ERASE
	struct flash_device *flash;
	int i;
	flash = (struct flash_device *)data;
	while (!g_atomic_int_get(&is_overwrite)) {
		usleep(100);
	}
	for (i = 0; i < NR_ERASE; i++) {
		usleep(1000 * 1000);
		flash->f_op->ioctl(flash, PAGE_FTL_IOCTL_TRIM);
		printf("\tforced garbage collection!\n");
	}
#endif
	(void)data;

	return NULL;
}

int main(void)
{
	pthread_t threads[6]; // write, write, read, read, overwrite, erase;
	int thread_id;
	int is_all_valid;
	size_t status;
	size_t i;
	struct flash_device *flash = NULL;

	memset(is_check, 0, sizeof(is_check));
#if defined(DEVICE_USE_ZONED)
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, ZONE_MODULE));
#elif defined(DEVICE_USE_BLUEDBM)
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, BLUEDBM_MODULE));
#else
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE));
#endif
	assert(0 == flash->f_op->open(flash, DEVICE_PATH, O_CREAT | O_RDWR));
	thread_id =
		pthread_create(&threads[0], NULL, write_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[1], NULL, write_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[2], NULL, read_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[3], NULL, read_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id = pthread_create(&threads[4], NULL, overwrite_thread,
				   (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}
	thread_id =
		pthread_create(&threads[5], NULL, erase_thread, (void *)flash);
	if (thread_id < 0) {
		perror("thread create failed");
		exit(errno);
	}

	pthread_join(threads[0], (void **)&status);
	pthread_join(threads[1], (void **)&status);
	pthread_join(threads[2], (void **)&status);
	pthread_join(threads[3], (void **)&status);
	pthread_join(threads[4], (void **)&status);
	pthread_join(threads[5], (void **)&status);

	flash->f_op->close(flash);
	assert(0 == module_exit(flash));

	is_all_valid = 1;
	for (i = 0; i < sizeof(is_check) / sizeof(int); i++) {
		if (!is_check[i]) {
			printf("read failed sector: %-20zu\n",
			       (i * BLOCK_SIZE));
			is_all_valid = 0;
		}
	}
	if (is_all_valid) {
		printf("all data exist => FINISH\n");
	}

	return 0;
}
