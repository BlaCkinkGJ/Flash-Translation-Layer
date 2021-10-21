/**
 * @file zone.c
 * @brief implementation of the lizbd which is inherited by the device
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-10-09
 */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <libzbd/zbd.h>
#include <fcntl.h>
#include <stdio.h>
#include <errno.h>
#include <glib.h>
#include <unistd.h>

#include "include/zone.h"
#include "include/device.h"
#include "include/log.h"

/**
 * @brief open the zoned block deivce file
 *
 * @param dev pointer of the device structure
 * @param name zoned block device's device filename
 * @param flags open flags for ramdisk
 *
 * @return 0 for success, negative number for fail
 */
int zone_open(struct device *dev, const char *name, int flags)
{
	struct zone_meta *meta;
	struct zbd_info zone_info;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	unsigned int reported_zones;
	unsigned int i;
	int ret = 0;

	info->nr_bus = (1 << DEVICE_NR_BUS_BITS);
	info->nr_chips = (1 << DEVICE_NR_CHIPS_BITS);

	block->nr_pages = (1 << DEVICE_NR_PAGES_BITS);
	page->size = DEVICE_PAGE_SIZE;

	meta = (struct zone_meta *)dev->d_private;

	ret = posix_memalign((void **)&meta->buffer, sysconf(_SC_PAGESIZE),
			     page->size);
	if (ret) {
		pr_err("buffer allocation failed\n");
		meta->buffer = NULL;
		goto exception;
	}
	meta->o_flags = flags;

	meta->read.fd = zbd_open(name, O_RDONLY, &zone_info);
	if (meta->read.fd < 0) {
		ret = -errno;
		pr_err("zone read file descriptor open failed\n");
		goto exception;
	}
	meta->write.fd = zbd_open(name, O_WRONLY | O_DIRECT, &zone_info);
	if (meta->write.fd < 0) {
		ret = -errno;
		pr_err("zone write file descriptor open failed\n");
		goto exception;
	}
	meta->total_size = zone_info.nr_zones * zone_info.zone_size;
	if (meta->o_flags & O_CREAT) {
		ret = zbd_reset_zones(meta->write.fd, 0, meta->total_size);
		if (ret) {
			pr_err("zone reset failed\n");
			goto exception;
		}
	}
	meta->zone_size = zone_info.zone_size;
	meta->nr_zones = zone_info.nr_zones;
	meta->block_size = zone_info.pblock_size;

	package->nr_blocks = meta->nr_zones;

	if (meta->zone_size % page->size) {
		pr_err("zone size is not aligned %lu bytes", page->size);
		ret = -EINVAL;
		goto exception;
	}

	if (zone_info.model != ZBD_DM_HOST_MANAGED) {
		pr_err("not host managed\n");
		ret = -EINVAL;
		goto exception;
	}

	ret = zbd_list_zones(meta->read.fd, 0, meta->total_size, ZBD_RO_ALL,
			     &meta->zones, &reported_zones);
	if (ret || reported_zones != meta->nr_zones) {
		pr_err("failed to list zones, err %d", ret);
		goto exception;
	}

	for (i = 0; i < reported_zones; i++) {
		struct zbd_zone *z = &meta->zones[i];
		if (zbd_zone_type(z) != ZBD_ZONE_TYPE_SWR) {
			pr_err("zone type is not sequential mode\n");
			goto exception;
		}
	}

	return ret;
exception:
	zone_close(dev);
	return ret;
}

/**
 * @brief execute the read and write function
 *
 * @param fd the number which contains the file descriptor
 * @param flag I/O direction
 * @param buffer pointer of the buffer
 * @param count the number of byte to read or write
 * @param offset position which wants to read or write
 *
 * @return the number of bytes after read or write, negative number means fail
 */
static ssize_t zone_do_rw(int fd, int flag, void *buffer, size_t count,
			  off_t offset)
{
	size_t remaining = count;
	off_t ofst = offset;
	ssize_t ret;

	while (remaining) {
		if (flag == DEVICE_READ) {
			ret = pread(fd, buffer, remaining, ofst);
		} else if (flag == DEVICE_WRITE) {
			ret = pwrite(fd, buffer, remaining, ofst);
		} else {
			pr_err("invalid flag detected (flag: %d)\n", flag);
			return -EINVAL;
		}
		if (ret < 0) {
			pr_err("%s failed %d (%s)\n",
			       flag == DEVICE_READ ? "read" : "write", errno,
			       strerror(errno));
			return -errno;
		}
		if (!ret)
			break;

		remaining -= ret;
		ofst += ret;
	}

	return count - remaining;
}

/**
 * @brief write to the zoned block device
 *
 * @param dev pointer of the device structre
 * @param request pointer of the user request
 *
 * @return the number of bytes to write, negative number for fail
 */
ssize_t zone_write(struct device *dev, struct device_request *request)
{
	struct zone_meta *meta;
	struct zbd_zone *zone;
	size_t page_size = device_get_page_size(dev);
	ssize_t ret = 0;
	uint64_t zone_num;

	meta = (struct zone_meta *)dev->d_private;

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		ret = -ENODATA;
		goto exception;
	}
	if (request->flag != DEVICE_WRITE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_WRITE, request->flag);
		ret = -EINVAL;
		goto exception;
	}
	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exception;
	}
	if (request->data_len != page_size) {
		pr_err("data write size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exception;
	}
	memcpy(meta->buffer, request->data, request->data_len);
	zone_num = zone_get_zone_number(dev, request->paddr);
	if (zone_num >= meta->nr_zones) {
		pr_err("invalid address value detected (lpn: %u)\n",
		       request->paddr.lpn);
		ret = -EINVAL;
		goto exception;
	}
	zone = &meta->zones[zone_num];
	if (zone->wp != (request->paddr.lpn * page_size)) {
		pr_err("write pointer doesn't match (expected: %lu, actual: %lu, zone: %lu)\n",
		       (uint64_t)(zone->wp),
		       (uint64_t)(request->paddr.lpn * page_size), zone_num);
		ret = -EINVAL;
		goto exception;
	}
	ret = zone_do_rw(meta->write.fd, request->flag, meta->buffer,
			 request->data_len,
			 (off_t)request->paddr.lpn * page_size);
	if (ret != (ssize_t)page_size) {
		pr_err("do io sequence failed(expected: %ld, actual: %ld)\n",
		       ret, (ssize_t)page_size);
		ret = -EFAULT;
		goto exception;
	}
	zone->wp += ret;
	if (zone->wp == zone->start + zone->len) {
		int status;
		status = zbd_finish_zones(meta->write.fd, zone->start,
					  zone->len);
		if (status) {
			pr_err("zone finish failed (start:%llu, len: %llu)\n",
			       zone->start, zone->len);
			goto exception;
		}
	}
	if (request->end_rq) {
		request->end_rq(request);
	}
	return ret;
exception:
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	return ret;
}

/**
 * @brief read to the zoned block device
 *
 * @param dev pointer of the device structre
 * @param request pointer of the user request
 *
 * @return the number of bytes to read, negative number for fail
 */
ssize_t zone_read(struct device *dev, struct device_request *request)
{
	struct zone_meta *meta;
	size_t page_size;
	ssize_t ret = 0;
	uint64_t zone_num;

	meta = (struct zone_meta *)dev->d_private;

	if (request->data == NULL) {
		pr_err("NULL data pointer detected\n");
		ret = -ENODATA;
		goto exception;
	}

	if (request->flag != DEVICE_READ) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_READ, request->flag);
	}

	page_size = device_get_page_size(dev);
	if (request->data_len != page_size) {
		pr_err("data read size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exception;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exception;
	}
	zone_num = zone_get_zone_number(dev, request->paddr);
	if (zone_num >= meta->nr_zones) {
		pr_err("invalid address value detected (lpn: %u)\n",
		       request->paddr.lpn);
		ret = -EINVAL;
		goto exception;
	}
	memset(meta->buffer, 0, page_size);
	ret = zone_do_rw(meta->read.fd, request->flag, meta->buffer,
			 request->data_len,
			 (off_t)request->paddr.lpn * page_size);
	memcpy(request->data, meta->buffer, page_size);
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	return ret;
exception:
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	return ret;
}

/**
 * @brief erase the segment to the zoned block device
 *
 * @param dev pointer of the device structre
 * @param request pointer of the user request
 *
 * @return 0 for success, negative number for fail
 */
int zone_erase(struct device *dev, struct device_request *request)
{
	struct zone_meta *meta;
	uint64_t zone_num;
	int ret = 0;
	off_t offset, length;

	meta = (struct zone_meta *)dev->d_private;
	zone_num = request->paddr.format.block;
	if (zone_num >= meta->nr_zones) {
		pr_err("invalid address detected\n");
		ret = -EINVAL;
		goto exception;
	}
	offset = meta->zone_size * zone_num;
	length = meta->zone_size;
	ret = zbd_reset_zones(meta->write.fd, offset, length);
	if (ret) {
		pr_err("zone reset failed (%lu ~ %lu)\n", offset,
		       offset + length);
		goto exception;
	}

	if (request->end_rq) {
		request->end_rq(request);
	}
	meta->zones[zone_num].wp = meta->zones[zone_num].start;
	return ret;
exception:
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	return ret;
}

/**
 * @brief close the zoned block device
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative number for fail
 */
int zone_close(struct device *dev)
{
	struct zone_meta *meta;
	meta = (struct zone_meta *)dev->d_private;
	if (meta == NULL) {
		return 0;
	}
	if (meta->buffer != NULL) {
		free(meta->buffer);
		meta->buffer = NULL;
	}
	if (meta->read.fd >= 0) {
		zbd_close(meta->read.fd);
		meta->read.fd = -1;
	}
	if (meta->write.fd >= 0) {
		zbd_close(meta->write.fd);
		meta->write.fd = -1;
	}
	if (meta->zones) {
		free(meta->zones);
		meta->zones = NULL;
	}
	return 0;
}

/**
 * @brief zoned block device operations
 */
struct device_operations __zone_dops = {
	.open = zone_open,
	.write = zone_write,
	.read = zone_read,
	.erase = zone_erase,
	.close = zone_close,
};

/**
 * @brief initialize the device module
 *
 * @param dev pointer of the device structure
 * @param flags flags for ramdisk and device
 *
 * @return 0 for sucess, negative value for fail
 */
int zone_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct zone_meta *meta;

	(void)flags;

	meta = (struct zone_meta *)malloc(sizeof(struct zone_meta));
	if (meta == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(meta, 0, sizeof(struct zone_meta));
	meta->read.fd = -1;
	meta->write.fd = -1;

	dev->d_private = (void *)meta;
	dev->d_submodule_exit = zone_device_exit;
	dev->d_op = &__zone_dops;

	return ret;
exception:
	zone_device_exit(dev);
	return ret;
}

/**
 * @brief deallocate the device module
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int zone_device_exit(struct device *dev)
{
	struct zone_meta *meta;
	meta = (struct zone_meta *)dev->d_private;
	if (meta != NULL) {
		zone_close(dev);
		free(meta);
		dev->d_private = NULL;
	}
	return 0;
}
