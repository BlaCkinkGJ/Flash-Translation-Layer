/**
 * @file device.h
 * @brief contain the device information header
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-01
 */
#ifndef DEVICE_H
#define DEVICE_H

#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>

// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#include "log.h" /**< to use the TOSTRING */

#define PADDR_EMPTY ((uint32_t)UINT32_MAX)

struct device_request;
struct device_operations;

#ifndef DEVICE_PAGE_SIZE
#define DEVICE_PAGE_SIZE (8192)
#endif

/**
 * @brief request allocation flags
 */
enum {
	DEVICE_DEFAULT_REQUEST = 0,
};

/**
 * @brief flash board I/O direction
 */
enum {
	DEVICE_WRITE = 0 /**< write flag */,
	DEVICE_READ /**< read flag */,
	DEVICE_ERASE /**< erase flag */,
};

/**
 * @brief support module list
 */
enum {
	RAMDISK_MODULE = 0 /**< select the ramdisk module */,
	BLUEDBM_MODULE /**< select the bluedbm module */,
	ZONE_MODULE /**< select the zone module */,
	RASPBERRY_MODULE /**< select the raspberry module */,
};

/**
 * @brief device address information
 *
 * @note
 * If you want to use the ZONE_MODULE, you must change the
 * DEVICE_NR_PAGES_BITS and DEVICE_NR_BLOCKS_BITS based on the zone's
 * size.
 */
#ifndef DEVICE_NR_BUS_BITS
#define DEVICE_NR_BUS_BITS (3)
#endif

#ifndef DEVICE_NR_CHIPS_BITS
#define DEVICE_NR_CHIPS_BITS (3)
#endif

#ifndef DEVICE_NR_PAGES_BITS
#define DEVICE_NR_PAGES_BITS (7)
#endif

#ifndef DEVICE_NR_BLOCKS_BITS
#define DEVICE_NR_BLOCKS_BITS (19)
#endif

#if 0
#pragma message("DEVICE_NR_BUS_BITS = " TOSTRING(DEVICE_NR_BUS_BITS))
#pragma message("DEVICE_NR_CHIPS_BITS = " TOSTRING(DEVICE_NR_CHIPS_BITS))
#pragma message("DEVICE_NR_PAGES_BITS = " TOSTRING(DEVICE_NR_PAGES_BITS))
#pragma message("DEVICE_NR_BLOCKS_BITS = " TOSTRING(DEVICE_NR_BLOCKS_BITS))
#endif

/**
 * @brief I/O end request function
 *
 * @param request device request structure's pointer
 *
 * @note
 * You must specify the call routine of this function in your custom device
 * module
 */
typedef void (*device_end_req_fn)(struct device_request *);

/**
 * @brief generic device address format
 *
 * @note
 * `seqnum` is used for distinguish the each host pages in a device page
 */
struct device_address {
	union {
		struct {
			uint32_t bus : DEVICE_NR_BUS_BITS;
			uint32_t chip : DEVICE_NR_CHIPS_BITS;
			uint32_t page : DEVICE_NR_PAGES_BITS;
			uint32_t block : DEVICE_NR_BLOCKS_BITS;
		} format;
		struct {
			uint32_t bus : DEVICE_NR_BUS_BITS;
			uint32_t chip : DEVICE_NR_CHIPS_BITS;
			uint32_t page : DEVICE_NR_PAGES_BITS;
			uint32_t block : DEVICE_NR_BLOCKS_BITS;
		} raspberry_converter;
		struct {
			uint32_t page
				: (DEVICE_NR_PAGES_BITS + DEVICE_NR_CHIPS_BITS +
				   DEVICE_NR_BUS_BITS);
			uint32_t block : 32 -
				(DEVICE_NR_PAGES_BITS + DEVICE_NR_CHIPS_BITS +
				 DEVICE_NR_BUS_BITS);
		} raspberry;
		uint32_t lpn;
	};
};

/**
 * @brief request for device
 */
struct device_request {
	unsigned int flag; /**< flag describes the bio's direction */

	size_t data_len; /**< data length (bytes) */
	size_t sector; /**< sector cursor (bytes) */
	struct device_address paddr; /**< this contains the ppa */

	void *data; /**< pointer of the data */
	device_end_req_fn end_rq; /**< end request function */

	// cppcheck-suppress unusedStructMember
	int is_finish;

	pthread_mutex_t mutex;
	pthread_cond_t cond;

	void *rq_private; /**< contain the request's private data */
};

/**
 * @brief flash board's page information
 */
struct device_page {
	size_t size; /**< byte */
};

/**
 * @brief flash board's block information
 */
struct device_block {
	struct device_page page;
	size_t nr_pages;
};

/**
 * @brief flash board's package(nand chip) information
 */
struct device_package {
	struct device_block block;
	size_t nr_blocks;
};

/**
 * @brief flash board's architecture information
 */
struct device_info {
	struct device_package package;
	size_t nr_bus; /**< bus equal to channel */
	size_t nr_chips; /**< chip equal to way */
};

/**
 * @brief metadata of the device
 */
struct device {
	pthread_mutex_t mutex;
	const struct device_operations *d_op;
	struct device_info info;
	uint64_t *badseg_bitmap;
	void *d_private; /**< generally contain the sub-layer's data structure */
	int (*d_submodule_exit)(struct device *);
};

/**
 * @brief operations for device
 */
struct device_operations {
	int (*open)(struct device *, const char *name, int flags);
	ssize_t (*write)(struct device *, struct device_request *);
	ssize_t (*read)(struct device *, struct device_request *);
	int (*erase)(struct device *, struct device_request *);
	int (*close)(struct device *);
};

/**
 * @brief layout assertions
 *
 * These pin the offsets the Rust port in `src/device/mod.rs` mirrors; its
 * `layout_matches_c` test asserts the same formulas, so a change to either
 * definition breaks a build instead of corrupting a device at runtime.
 */
#define DEVICE_ALIGN_UP(value, align) (((value) + (align) - 1) / (align) * (align))

#ifdef __cplusplus
#define DEVICE_ALIGNOF(type) alignof(type)
// cppcheck-suppress missingIncludeSystem
#define DEVICE_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#else
#define DEVICE_ALIGNOF(type) _Alignof(type)
#define DEVICE_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

DEVICE_STATIC_ASSERT(sizeof(struct device_address) == sizeof(uint32_t),
		     "device_address must be one packed uint32_t");

DEVICE_STATIC_ASSERT(sizeof(struct device_page) == sizeof(size_t),
		     "device_page must be one size_t");
DEVICE_STATIC_ASSERT(sizeof(struct device_block) == 2 * sizeof(size_t),
		     "device_block must be page plus nr_pages");
DEVICE_STATIC_ASSERT(sizeof(struct device_package) == 3 * sizeof(size_t),
		     "device_package must be block plus nr_blocks");
DEVICE_STATIC_ASSERT(sizeof(struct device_info) == 5 * sizeof(size_t),
		     "device_info must be package plus nr_bus and nr_chips");
DEVICE_STATIC_ASSERT(offsetof(struct device_info, nr_bus) ==
			     3 * sizeof(size_t),
		     "device_info.nr_bus must follow the package");

DEVICE_STATIC_ASSERT(offsetof(struct device, mutex) == 0,
		     "device.mutex must be first");
DEVICE_STATIC_ASSERT(offsetof(struct device, d_op) ==
			     DEVICE_ALIGN_UP(sizeof(pthread_mutex_t),
					     sizeof(void *)),
		     "device.d_op must follow mutex");
DEVICE_STATIC_ASSERT(offsetof(struct device, info) ==
			     offsetof(struct device, d_op) + sizeof(void *),
		     "device.info must follow d_op");
DEVICE_STATIC_ASSERT(offsetof(struct device, badseg_bitmap) ==
			     offsetof(struct device, info) +
				     5 * sizeof(size_t),
		     "device.badseg_bitmap must follow info");
DEVICE_STATIC_ASSERT(offsetof(struct device, d_private) ==
			     offsetof(struct device, badseg_bitmap) +
				     sizeof(void *),
		     "device.d_private must follow badseg_bitmap");
DEVICE_STATIC_ASSERT(offsetof(struct device, d_submodule_exit) ==
			     offsetof(struct device, d_private) +
				     sizeof(void *),
		     "device.d_submodule_exit must follow d_private");
DEVICE_STATIC_ASSERT(sizeof(struct device) ==
			     offsetof(struct device, d_submodule_exit) +
				     sizeof(void *),
		     "device must end after d_submodule_exit");

DEVICE_STATIC_ASSERT(offsetof(struct device_request, flag) == 0,
		     "device_request.flag must be first");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, data_len) ==
			     DEVICE_ALIGN_UP(sizeof(unsigned int),
					     sizeof(size_t)),
		     "device_request.data_len must follow flag");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, sector) ==
			     offsetof(struct device_request, data_len) +
				     sizeof(size_t),
		     "device_request.sector must follow data_len");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, paddr) ==
			     offsetof(struct device_request, sector) +
				     sizeof(size_t),
		     "device_request.paddr must follow sector");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, data) ==
			     DEVICE_ALIGN_UP(
				     offsetof(struct device_request, paddr) +
					     sizeof(struct device_address),
				     sizeof(void *)),
		     "device_request.data must follow paddr");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, end_rq) ==
			     offsetof(struct device_request, data) +
				     sizeof(void *),
		     "device_request.end_rq must follow data");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, is_finish) ==
			     offsetof(struct device_request, end_rq) +
				     sizeof(void *),
		     "device_request.is_finish must follow end_rq");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, mutex) ==
			     DEVICE_ALIGN_UP(
				     offsetof(struct device_request, is_finish) +
					     sizeof(int),
				     DEVICE_ALIGNOF(pthread_mutex_t)),
		     "device_request.mutex must follow is_finish");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, cond) ==
			     offsetof(struct device_request, mutex) +
				     sizeof(pthread_mutex_t),
		     "device_request.cond must follow mutex");
DEVICE_STATIC_ASSERT(offsetof(struct device_request, rq_private) ==
			     DEVICE_ALIGN_UP(
				     offsetof(struct device_request, cond) +
					     sizeof(pthread_cond_t),
				     sizeof(void *)),
		     "device_request.rq_private must follow cond");
DEVICE_STATIC_ASSERT(sizeof(struct device_request) ==
			     offsetof(struct device_request, rq_private) +
				     sizeof(void *),
		     "device_request must end after rq_private");

DEVICE_STATIC_ASSERT(sizeof(struct device_operations) == 5 * sizeof(void *),
		     "device_operations must be five function pointers");

#undef DEVICE_STATIC_ASSERT
#undef DEVICE_ALIGNOF
#undef DEVICE_ALIGN_UP

struct device_request *device_alloc_request(uint64_t flags);
void device_free_request(struct device_request *);

int device_module_init(const uint64_t modnum, struct device **, uint64_t flags);
int device_module_exit(struct device *);

/**
 * @brief get the number of segments in a flash board
 *
 * @param dev device structure pointer
 *
 * @return the number of segments in a flash board
 */
static inline size_t device_get_nr_segments(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	return package->nr_blocks;
}

static inline size_t device_get_blocks_per_segment(struct device *dev)
{
	struct device_info *info = &dev->info;
	return (info->nr_bus * info->nr_chips);
}

/**
 * @brief get the number of pages in a segment
 *
 * @param dev device structure pointer
 *
 * @return the number of pages in a segment
 */
static inline size_t device_get_pages_per_segment(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;

	return device_get_blocks_per_segment(dev) * block->nr_pages;
}

/**
 * @brief get flash board's NAND page size
 *
 * @param dev device structure pointer
 *
 * @return NAND page size (generally, 8192 or 4096)
 */
static inline size_t device_get_page_size(struct device *dev)
{
	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;
	return page->size;
}

/**
 * @brief total size of a flash board
 *
 * @param dev device structure pointer
 *
 * @return flash board's total size (byte)
 */
static inline size_t device_get_total_size(struct device *dev)
{
	size_t nr_segments = device_get_nr_segments(dev);
	size_t nr_pages_per_segment = device_get_pages_per_segment(dev);
	size_t page_size = device_get_page_size(dev);

	return nr_segments * nr_pages_per_segment * page_size;
}

/**
 * @brief get total the number of pages in a flash board
 *
 * @param dev device structure pointer
 *
 * @return the number of pages in a flash board.
 */
static inline size_t device_get_total_pages(struct device *dev)
{
	return device_get_total_size(dev) / device_get_page_size(dev);
}

#endif
