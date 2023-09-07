/**
 * @file page-map.c
 * @brief manage the mapping information
 * @author Gijun Oh
 * @version 0.2
 * @date 2021-10-05
 */
#include "bits.h"
#include "page.h"
#include "device.h"
#include "log.h"

#include <errno.h>
#include <inttypes.h>

/**
 * @brief get page from the segment
 *
 * @param pgftl pointer of the page-ftl structure
 *
 * @return free space's device address
 */
struct device_address page_ftl_get_free_page(struct page_ftl *pgftl)
{
	struct device_address paddr;
	struct device *dev;

	struct page_ftl_segment *segment;

	size_t nr_segments;
	size_t pages_per_segment;
	size_t segnum;
	size_t idx;

	uint64_t nr_free_pages;
	uint64_t nr_valid_pages;
	uint32_t page;

	dev = pgftl->dev;
	nr_segments = device_get_nr_segments(dev);
	pages_per_segment = device_get_pages_per_segment(dev);

	paddr.lpn = PADDR_EMPTY;
	idx = 0;

retry:
	if (idx == nr_segments) {
		pr_err("cannot find the free page in the device\n");
		paddr.lpn = PADDR_EMPTY;
		return paddr;
	}
	segnum = ((size_t)pgftl->alloc_segnum + idx) % nr_segments;
	idx += 1;

	if (dev->badseg_bitmap && get_bit(dev->badseg_bitmap, segnum)) {
    //pr_info("\n\tbadseg : %d\n\tbadseg_bitmap : %d\n", segnum, dev->badseg_bitmap[segnum]);
		goto retry;
	}

	segment = &pgftl->segments[segnum];
	if (segment == NULL) {
		pr_err("fatal error detected: cannot find the segnum %zu\n",
		       segnum);
		paddr.lpn = PADDR_EMPTY;
		return paddr;
	}
	nr_free_pages = (uint64_t)g_atomic_int_get(&segment->nr_free_pages);
	if (nr_free_pages == 0) {
		goto retry;
	}
	pgftl->alloc_segnum = segnum;

	page = (uint32_t)find_first_zero_bit(segment->use_bits,
					     pages_per_segment, 0);
	if (page == (uint32_t)BITS_NOT_FOUND) {
		pr_warn("nr_free_pages and use_bits bitmap are not synchronized(nr_free_pages: %" PRIu64
			", page: %u)\n",
			nr_free_pages, page);
		goto retry;
	}
	paddr.lpn = 0;
	paddr.format.block = (uint16_t)segnum;
	paddr.lpn |= page;

	set_bit(segment->use_bits, page);
	g_atomic_int_set(&segment->nr_free_pages, (gint)nr_free_pages - 1);

	nr_valid_pages = (uint64_t)g_atomic_int_get(&segment->nr_valid_pages);
	g_atomic_int_set(&segment->nr_valid_pages, (gint)nr_valid_pages + 1);

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
int page_ftl_update_map(struct page_ftl *pgftl, size_t sector, uint32_t ppn)
{
	uint32_t *trans_map;
	uint64_t lpn;
	size_t map_size;

	lpn = page_ftl_get_lpn(pgftl, sector);

	map_size = page_ftl_get_map_size(pgftl) / sizeof(uint32_t);
	if (lpn >= (uint64_t)map_size) {
		pr_err("lpn value overflow detected (max: %zu, cur: %" PRIu64
		       ")\n",
		       map_size, lpn);
		return -EINVAL;
	}

	trans_map = pgftl->trans_map;
	trans_map[lpn] = ppn;

	return 0;
}
