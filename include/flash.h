/**
 * @file flash.h
 * @brief generic flash control interfaces' header
 * @author Gijun Oh
 * @version 1.0
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

/** follow the linux kernel's value */
#define PAGE_SHIFT (12)

/** define empty mapping information */
#define PADDR_EMPTY (UINT32_MAX)

/**
 * @brief flash board information
 *
 * @note
 * You MUST NOT think this information based on the
 * chip datasheet.
 */
enum { FLASH_NR_SEGMENT = 4096,
       FLASH_PAGES_PER_BLOCK = 128,
       FLASH_PAGE_SIZE = 8192,
       FLASH_TOTAL_CHIPS = 8,
       FLASH_TOTAL_BUS = 8,
};

/**
 * @brief flash board I/O direction
 */
enum { FLASH_FTL_WRITE = 0,
       FLASH_FTL_READ,
       FLASH_FTL_ERASE,
};

/** segment information */
#define FLASH_BLOCKS_PER_SEGMENT (FLASH_TOTAL_CHIPS * FLASH_TOTAL_BUS)
#define FLASH_PAGES_PER_SEGMENT                                                \
	(FLASH_BLOCKS_PER_SEGMENT * FLASH_PAGES_PER_BLOCK)
/** total flash board size (bytes) */
#define FLASH_DISK_SIZE                                                        \
	((uint64_t)FLASH_NR_SEGMENT * FLASH_PAGES_PER_SEGMENT * FLASH_PAGE_SIZE)
/** mapping table size (bytes) */
#define FLASH_MAP_SIZE (FLASH_DISK_SIZE >> PAGE_SHIFT)
/** host I/O size (bytes) */
#define FLASH_HOST_PAGE_SIZE (4096)
/** number of the cache block */
#define FLASH_NR_CACHE_BLOCK (1024)

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
	int (*open)(struct flash_device *); /**< open the flash device */
	ssize_t (*write)(struct flash_device *, const void *buffer,
			 size_t count, off_t offset); /**< write to the flash */
	ssize_t (*read)(struct flash_device *, const void *buffer, size_t count,
			off_t offset); /**< read from the flash */
	int (*close)(struct flash_device *); /** close the flash device */
};

int flash_module_init(struct flash_device **, uint64_t flags);
int flash_module_exit(struct flash_device *);

#ifdef __cplusplus
}
#endif

#endif
