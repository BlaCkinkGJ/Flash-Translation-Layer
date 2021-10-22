/**
 * @file page-map.c
 * @brief manage the mapping information
 * @author Gijun Oh
 * @version 0.1
 * @date 2021-10-05
 */
#include "include/bits.h"
#include "include/page.h"
#include "include/device.h"
#include "include/log.h"

#include <errno.h>

/**
 * @brief get page from the segment
 *
 * @param pgftl pointer of the page-ftl structure
 *
 * @return free space's device address
 * @note
 * code block containing `pthread_mutex_trylock` is really dangerous.
 * Therefore, you must carefully use or check that lines.
 */
struct device_address page_ftl_get_free_page(struct page_ftl *pgftl)
{
	struct device_address paddr;
	struct device *dev;

	struct page_ftl_segment *segment;

	size_t nr_segments;
	size_t max_retry_size;
	size_t pages_per_segment;
	size_t segnum;
	size_t idx;

	uint64_t nr_free_pages;
	uint64_t nr_valid_pages;
	uint32_t offset;

	dev = pgftl->dev;
	nr_segments = device_get_nr_segments(dev);
	max_retry_size = nr_segments;
	pages_per_segment = device_get_pages_per_segment(dev);

	paddr.lpn = PADDR_EMPTY;
	idx = 0;

retry:
	if (idx == max_retry_size) {
		pr_err("cannot find the free page in the device\n");
		paddr.lpn = PADDR_EMPTY;
		goto exit;
	}
	segnum = (pgftl->alloc_segnum + idx) % nr_segments;
	idx += 1;

	if (dev->badseg_bitmap && get_bit(dev->badseg_bitmap, segnum)) {
		goto retry;
	}

	segment = &pgftl->segments[segnum];
	if (segment == NULL) {
		pr_err("fatal error detected: cannot find the segnum %zu\n",
		       segnum);
		paddr.lpn = PADDR_EMPTY;
		goto exit;
	}

	nr_free_pages = g_atomic_int_get(&segment->nr_free_pages);
	if (nr_free_pages == 0) {
		goto retry;
	}
	pgftl->alloc_segnum = segnum;

	offset = find_first_zero_bit(segment->use_bits, pages_per_segment, 0);
	if (offset == (uint32_t)BITS_NOT_FOUND) {
		pr_warn("nr_free_pages and use_bits bitmap are not synchronized(nr_free_pages: %lu, offset: %u)\n",
			nr_free_pages, offset);
		goto retry;
	}

	if (pthread_mutex_trylock(&segment->mutex)) {
#if defined(DEVICE_USE_ZONED)
		usleep(10);
		idx -= 1;
#else
		max_retry_size += segnum; /**< BE CAREFUL! */
#endif
		goto retry;
	}
	paddr.lpn = 0;
	paddr.format.block = segnum;
	paddr.lpn |= offset;

	set_bit(segment->use_bits, offset);
	g_atomic_int_set(&segment->nr_free_pages, nr_free_pages - 1);

	nr_valid_pages = g_atomic_int_get(&segment->nr_valid_pages);
	g_atomic_int_set(&segment->nr_valid_pages, nr_valid_pages + 1);

exit:
	return paddr;
}

/**
 * @brief update the mapping information
 *
 * @param pgftl pointer of the page FTL structure
 * @param sector logical address for mapping table
 * @param ppn physical address for mapping table
 *
 * @return 0 to success, negative number to fail
 */
int page_ftl_update_map(struct page_ftl *pgftl, uint64_t sector, uint32_t ppn)
{
	uint32_t *trans_map;
	uint64_t lpn;
	size_t map_size;
	int ret;

	ret = 0;
	lpn = page_ftl_get_lpn(pgftl, sector);

	map_size = page_ftl_get_map_size(pgftl) / sizeof(uint32_t);
	if (lpn >= (uint64_t)map_size) {
		pr_err("lpn value overflow detected (max: %zu, cur: %lu)\n",
		       map_size, lpn);
		ret = -EINVAL;
		goto exit;
	}

	trans_map = pgftl->trans_map;
	trans_map[lpn] = ppn;

exit:
	return ret;
}

/**
 * @brief get mapping information from the l2p table
 *
 * @param pgftl pointer of the page FTL structure
 * @param lpn logical page number
 *
 * @return physical page number
 */
uint32_t page_ftl_get_ppn(struct page_ftl *pgftl, size_t lpn)
{
	uint32_t ppn;
	if (lpn >= page_ftl_get_map_size(pgftl)) {
		pr_warn("invalid lpn detected: %zu\n", lpn);
		return PADDR_EMPTY;
	}
	ppn = pgftl->trans_map[lpn];
	return ppn;
}

/**
 * @brief invalidate current l2p mapping information
 *
 * @param pgftl pointer of the page FTL structure
 * @param lpn logical page number which wants to invalidate
 */
void page_ftl_invalidate_map(struct page_ftl *pgftl, size_t lpn)
{
	pgftl->trans_map[lpn] = PADDR_EMPTY;
}
