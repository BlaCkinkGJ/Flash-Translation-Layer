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
// cppcheck-suppress missingIncludeSystem
#include <sys/types.h>
// cppcheck-suppress missingIncludeSystem
#include <unistd.h>
#include <pthread.h>
// cppcheck-suppress missingIncludeSystem
#include <stddef.h> /**< to use offsetof in the layout assertions */

/**
 * @brief flags related on the flash and submodule
 */
enum {
	FLASH_DEFAULT_FLAG = 0 /**< flash default flags */,
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
	int (*open)(struct flash_device *, const char *name,
		    int flags); /**< open the flash device */
	ssize_t (*write)(struct flash_device *, void *buffer, size_t count,
			 off_t offset); /**< write to the flash */
	ssize_t (*read)(struct flash_device *, void *buffer, size_t count,
			off_t offset); /**< read from the flash */
	int (*ioctl)(struct flash_device *, unsigned int request,
		     ...); /**< for other instruction sets (barely use) */
	int (*close)(struct flash_device *); /** close the flash device */
};

/**
 * @brief layout assertions
 *
 * These pin the offsets the Rust port in `src/flash.rs` mirrors. Its
 * `layout_matches_c` test asserts the same formulas, so a change to either
 * definition breaks a build instead of corrupting a flash device at runtime.
 */
#ifdef __cplusplus
// cppcheck-suppress missingIncludeSystem
#define FLASH_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#else
#define FLASH_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

FLASH_STATIC_ASSERT(offsetof(struct flash_device, f_op) == 0,
		    "flash_device must start with the operations pointer");
FLASH_STATIC_ASSERT(offsetof(struct flash_device, f_private) == sizeof(void *),
		    "flash_device private data must follow the operations pointer");
FLASH_STATIC_ASSERT(offsetof(struct flash_device, f_submodule_exit) ==
			    2 * sizeof(void *),
		    "flash_device submodule exit must follow the private data");
FLASH_STATIC_ASSERT(sizeof(struct flash_device) == 3 * sizeof(void *),
		    "flash_device must be three pointers, no padding");

FLASH_STATIC_ASSERT(offsetof(struct flash_operations, open) == 0,
		    "flash_operations must start with open");
FLASH_STATIC_ASSERT(offsetof(struct flash_operations, write) == sizeof(void *),
		    "flash_operations write must follow open");
FLASH_STATIC_ASSERT(offsetof(struct flash_operations, read) ==
			    2 * sizeof(void *),
		    "flash_operations read must follow write");
FLASH_STATIC_ASSERT(offsetof(struct flash_operations, ioctl) ==
			    3 * sizeof(void *),
		    "flash_operations ioctl must follow read");
FLASH_STATIC_ASSERT(offsetof(struct flash_operations, close) ==
			    4 * sizeof(void *),
		    "flash_operations close must follow ioctl");
FLASH_STATIC_ASSERT(sizeof(struct flash_operations) == 5 * sizeof(void *),
		    "flash_operations must be five pointers, no padding");

int flash_module_init(struct flash_device **, uint64_t flags);
int flash_module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif

#endif
