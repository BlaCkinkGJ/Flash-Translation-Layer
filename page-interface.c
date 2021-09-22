/**
 * @file page-interface.c
 * @brief interface for page ftl
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include <errno.h>

#include "include/log.h"
#include "include/page.h"

/**
 * @brief open the page flash translation layer based device
 *
 * @param flash pointer of the flash device information
 *
 * @return zero to success, error number to fail
 */
static int page_ftl_open_interface(struct flash_device *flash)
{
	struct page_ftl *pgftl = NULL;
	if (flash == NULL) {
		pr_err("flash pointer doesn't exist\n");
		return -EINVAL;
	}
	pgftl = (struct page_ftl *)flash->f_private;
	if (pgftl == NULL) {
		pr_err("page FTL information doesn't exist\n");
		return -EINVAL;
	}
	return page_ftl_open(pgftl);
}

/**
 * @brief write the page flash translation layer based device
 *
 * @param flash pointer of the flash device information
 * @param buffer pointer of the data buffer
 * @param count size of the buffer (bytes)
 * @param offset size of the offset (bytes)
 *
 * @return positive or zero write size to success, negative number to fail
 */
static ssize_t page_ftl_write_interface(struct flash_device *flash,
					const void *buffer, size_t count,
					off_t offset)
{
	ssize_t size = -1;
	struct page_ftl *pgftl = NULL;
	struct page_ftl_request *request = NULL;

	/** check the pointer validity */
	if (flash == NULL) {
		pr_err("flash pointer doesn't exist\n");
		return -EINVAL;
	}

	pgftl = (struct page_ftl *)flash->f_private;
	if (pgftl == NULL) {
		pr_err("page FTL information doesn't exist\n");
		return -EINVAL;
	}

	/** allocate the request */
	request = (struct page_ftl_request *)malloc(
		sizeof(struct page_ftl_request));
	if (request == NULL) {
		pr_err("fail to allocate request structure\n");
		goto exception;
	}

	request->flag = FLASH_FTL_WRITE;
	request->data_len = count;
	request->sector = offset;
	request->data = buffer;

	/** submit the request */
	size = page_ftl_submit_request(pgftl, request);
	if (size < 0) {
		pr_err("page FTL submit request failed\n");
		goto exception;
	}
	free(request);
	return size;

exception:
	if (request) {
		free(request);
	}
	return size;
}

/**
 * @brief read the page flash translation layer based device
 *
 * @param flash pointer of the flash device information
 * @param buffer pointer of the data buffer
 * @param count size of the buffer (bytes)
 * @param offset size of the offset (bytes)
 *
 * @return positive or zero read size to success, negative number to fail
 */
static ssize_t page_ftl_read_interface(struct flash_device *flash,
				       const void *buffer, size_t count,
				       off_t offset)
{
	ssize_t size = -1;
	struct page_ftl *pgftl = NULL;
	struct page_ftl_request *request = NULL;

	/** check the pointer validity */
	if (flash == NULL) {
		pr_err("flash pointer doesn't exist\n");
		return -EINVAL;
	}
	pgftl = (struct page_ftl *)flash->f_private;
	if (pgftl == NULL) {
		pr_err("page FTL information doesn't exist\n");
		return -EINVAL;
	}

	/** allocate the request */
	request = (struct page_ftl_request *)malloc(
		sizeof(struct page_ftl_request));
	if (request == NULL) {
		pr_err("fail to allocate request structure\n");
		goto exception;
	}

	request->flag = FLASH_FTL_READ;
	request->data_len = count;
	request->sector = offset;
	request->data = buffer;

	/** submit the request */
	size = page_ftl_submit_request(pgftl, request);
	if (size < 0) {
		pr_err("page FTL submit request failed\n");
		goto exception;
	}
	free(request);
	return size;

exception:
	if (request) {
		free(request);
	}
	return size;
}

/**
 * @brief close the page flash translation layer based device
 *
 * @param flash pointer of the flash device information
 *
 * @return zero to success, error number to fail
 */
static int page_ftl_close_interface(struct flash_device *flash)
{
	struct page_ftl *pgftl = NULL;
	if (flash == NULL) {
		pr_err("flash pointer doesn't exist\n");
		return -EINVAL;
	}
	pgftl = (struct page_ftl *)flash->f_private;
	if (pgftl == NULL) {
		pr_err("page FTL information doesn't exist\n");
		return -EINVAL;
	}
	return page_ftl_close(pgftl);
}

/**
 * @brief implementation of the flash_operations
 */
const struct flash_operations __page_fops = {
	.open = page_ftl_open_interface,
	.write = page_ftl_write_interface,
	.read = page_ftl_read_interface,
	.close = page_ftl_close_interface,
};

/**
 * @brief initialize the page flash translation layer module
 *
 * @param flash pointer of the flash device information
 * @param flags flags for flash and submodule
 *
 * @return zero to success, error number to fail
 */
int page_ftl_module_init(struct flash_device *flash, uint64_t flags)
{
	int err = 0;
	struct page_ftl *pgftl = NULL;
	flash->f_op = &__page_fops;

	pgftl = (struct page_ftl *)malloc(sizeof(struct page_ftl));
	if (pgftl == NULL) {
		err = errno;
		pr_err("fail to allocate the page FTL information pointer\n");
		goto exception;
	}

	/** initialize the mapping table */
	for (uint64_t lpa = 0; lpa < FLASH_MAP_SIZE; lpa++) {
		pgftl->trans_map[lpa] = PADDR_EMPTY;
	}

	flash->f_private = (void *)pgftl;
	return 0;

exception:
	page_ftl_module_exit(flash);
	return err;
}

/**
 * @brief free resources in the page flash translation layer module
 *
 * @param flash pointer of the flash device information
 *
 * @return zero to success, error number to fail
 *
 * @note
 * You must not free resources related on the flash module which is parent module
 */
int page_ftl_module_exit(struct flash_device *flash)
{
	struct page_ftl *pgftl = NULL;
	if (flash == NULL) {
		pr_err("flash pointer is null detected\n");
		return 0;
	}

	pgftl = (struct page_ftl *)flash->f_private;
	if (pgftl) {
		free(pgftl);
	}
	return 0;
}
