/**
 * @file page-core.c
 * @brief core logic for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <errno.h>
#ifdef __cplusplus
#include <atomic>
#else
#include <stdatomic.h>
#endif

#include <assert.h>

#include "include/page.h"
#include "include/log.h"

/**
 * @brief initialize each segment's metadata
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return 0 to success, negative value to fail
 */
static int page_ftl_init_segment(struct page_ftl *pgftl)
{
	int compensator;
	struct page_ftl_segment *segments = NULL;

	assert(0 < FLASH_HOST_PAGE_SIZE);
	assert(0 < FLASH_PAGE_SIZE);

	compensator = FLASH_PAGE_SIZE / FLASH_HOST_PAGE_SIZE;
	if (FLASH_PAGE_SIZE < FLASH_HOST_PAGE_SIZE) {
		pr_err("flash page size must larger than host page size (%d >= %d)\n",
		       FLASH_PAGE_SIZE, FLASH_HOST_PAGE_SIZE);
		return -EINVAL;
	}

	segments = pgftl->segment;
	for (uint64_t i = 0; i < FLASH_NR_SEGMENT; i++) {
		segments[i].valid_bits = NULL;
	}

	for (uint64_t i = 0; i < FLASH_NR_SEGMENT; i++) {
		struct vEB *valid_bits = NULL;
		valid_bits = vEB_init(FLASH_PAGES_PER_SEGMENT * compensator);
		if (valid_bits == NULL) {
			pr_err("allocated failed(seq:%lu)", i);
			return -ENOMEM;
		}
		segments[i].valid_bits = valid_bits;
		atomic_store(&segments[i].nr_invalid_blocks, 0);
	}
	return 0;
}

/**
 * @brief recovery l2p and metadata from the oob(out-of-bound) area
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return 0 to success, negative value to fail
 *
 * @todo you must implement this after create the read/write routine
 */
static int page_ftl_recovery_from_oob(struct page_ftl *pgftl)
{
	(void)pgftl;
	return 0;
}

/**
 * @brief allocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 *
 * @note
 * Each segment's van-emde-boas tree size is
 * FLASH_BLOCKS_PER_SEGMENT * FLASH_PAGES_PER_BLOCK * (FLASH_PAGE_SIZE / FLASH_HOST_IO_SIZE).
 *
 * This means we adjust the page io size to the host io size. For example,
 * the Linux's normal io size is 4096 bytes. However, our flash boards's page size
 * is 8192 bytes (double of the Linux's io size). Therefore, we must bitmap to adjust
 * that size. As a result, we use the `compensator`.
 *
 * Van-emde-boas tree's overhead is following below. (adopt the default option)
 * 1. bits per describe segment = (8192(pages per segment) * 2(compensator))/8(bits per byte) = 2048
 * 2. segments per flash = 4096
 * 3. segments per flash * bits per describe segment = 4096 * 2048 = 8388608 = 8MiB
 *
 * Mapping table overhead is following below. (adopt the default option)
 * 1. flash board size = 4096(# of segments) * 8192(pages per segment) * 8192(page size) = 256GiB
 * 2. entries per mapping table = 256GiB(flash board size) / 4KiB(host page size) = 67108864
 * 3. mapping table size = 4(entry size) * 67108864(entries per mapping table) = 256MiB
 * 
 */
int page_ftl_open(struct page_ftl *pgftl)
{
	int err;

	pthread_mutex_init(&pgftl->mutex, NULL);

	err = page_ftl_init_segment(pgftl);
	if (err) {
		goto exception;
	}

	return 0;

exception:
	page_ftl_close(pgftl);
	return err;
}

/**
 * @brief submit the request to the valid function
 *
 * @param pgftl pointer of the page ftl structure
 * @param request pointer of the request
 *
 * @return read and write return the size of the submit,
 * fail to return the nugative value
 */
ssize_t page_ftl_submit_request(struct page_ftl *pgftl,
				struct page_ftl_request *request)
{
	if (pgftl == NULL || request == NULL) {
		pr_err("null detected (pgftl:%p, request:%p)\n", pgftl,
		       request);
		return -EINVAL;
	}
	switch (request->flag) {
	case FLASH_FTL_WRITE:
		return page_ftl_write(pgftl, request);
	case FLASH_FTL_READ:
		return page_ftl_read(pgftl, request);
	default:
		pr_err("invalid flag detected: %u\n", request->flag);
		return -EINVAL;
	}
}

/**
 * @brief deallocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 */
int page_ftl_close(struct page_ftl *pgftl)
{
	struct page_ftl_segment *segments = NULL;

	if (pgftl == NULL) {
		pr_err("null page ftl structure submitted\n");
		return 0;
	}

	segments = pgftl->segment;
	if (segments != NULL) {
		for (int i = 0; i < FLASH_BLOCKS_PER_SEGMENT; i++) {
			struct vEB *valid_bits = NULL;
			valid_bits = segments[i].valid_bits;
			if (valid_bits == NULL) {
				continue;
			}
			vEB_free(valid_bits);
		}
	}
	return 0;
}
