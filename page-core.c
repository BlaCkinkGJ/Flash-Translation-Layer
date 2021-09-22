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

#include "include/module.h"
#include "include/page.h"

/**
 * @brief allocate the page ftl structure's members
 *
 * @param pgftl pointer of the page ftl structure
 *
 * @return zero to success, negative number to fail
 */
int page_ftl_open(struct page_ftl *pgftl)
{
	int i = 0;
	struct page_ftl_segment *segments = NULL;

	pthread_mutex_init(&pgftl->mutex, NULL);

	segments = pgftl->segment;
	for (i = 0; i < FLASH_NR_SEGMENT; i++) {
		segments[i].valid_bits = NULL;
	}

	for (i = 0; i < FLASH_NR_SEGMENT; i++) {
		struct vEB *valid_bits = NULL;
		valid_bits = vEB_init(FLASH_BLOCKS_PER_SEGMENT);
		if (valid_bits == NULL) {
			pr_info("allocated failed(seq:%d)", i);
			goto exception;
		}
		segments[i].valid_bits = valid_bits;
		atomic_store(&segments[i].nr_invalid_blocks, 0);
	}
	return 0;

exception:
	page_ftl_close(pgftl);
	return -ENOMEM;
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
		pr_info("null detected (pgftl:%p, request:%p)\n", pgftl,
			request);
		return -EINVAL;
	}
	switch (request->flag) {
	case FLASH_FTL_WRITE:
		return page_ftl_write(pgftl, request);
	case FLASH_FTL_READ:
		return page_ftl_read(pgftl, request);
	default:
		pr_info("invalid flag detected: %u\n", request->flag);
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
		pr_info("%s\n", "null page ftl structure submitted");
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
