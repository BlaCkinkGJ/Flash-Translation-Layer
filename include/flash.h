/**
 * @file flash.h
 * @brief generic flash control interfaces' header
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-09-22
 */
#ifndef FLASH_H
#define FLASH_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdlib.h>
#include <stdint.h>
#include <pthread.h>

/**
 * @brief flags related on the flash and submodule
 */
enum { FLASH_DEFAULT_FLAG = 0 /**< flash default flags */,
};

struct flash_device;
struct flash_operations;

/**
 * @brief contain the flash device information
 */
struct flash_device {
	const struct flash_operations *f_op; /**< contain the flash operations */
	void *f_private; /**< device specific information contained */
	int (*f_submodule_exit)(struct flash_device *); /**< deallocate */
};

/**
 * @brief generic interface for communicate with the flash
 *
 * @note
 * `struct flash_device *` means flash control information
 * - count: length of the buffer (bytes)
 * - offset: offset of the write position (bytes, NOT sector(512 bytes))
 */
struct flash_operations {
	int (*open)(struct flash_device *,
		    const char *name); /**< open the flash device */
	ssize_t (*write)(struct flash_device *, void *buffer, size_t count,
			 off_t offset); /**< write to the flash */
	ssize_t (*read)(struct flash_device *, void *buffer, size_t count,
			off_t offset); /**< read from the flash */
	int (*ioctl)(struct flash_device *, unsigned int request,
		     ...); /**< for other instruction sets (barely use) */
	int (*close)(struct flash_device *); /** close the flash device */
};

int flash_module_init(struct flash_device **, uint64_t flags);
int flash_module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif

#endif
