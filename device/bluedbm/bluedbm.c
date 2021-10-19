#include <stdlib.h>
#include <errno.h>
#include <libmemio.h>
#include <pthread.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>
#include <string.h>

#include "include/bluedbm.h"
#include "include/device.h"
#include "include/log.h"
#include "include/bits.h"

static uint64_t *g_badseg_bitmap = NULL;
static gint *g_erase_counter = NULL;

static void bluedbm_erase_end_request(uint64_t segnum, uint8_t is_bad)
{
	if (g_badseg_bitmap == NULL || g_erase_counter == NULL) {
		pr_err("NULL pointer detected\n");
		pr_err("\tg_badseg_bitmap: %p\n", g_badseg_bitmap);
		pr_err("\tg_erase_finish_bitmap: %p\n", g_erase_counter);
		return;
	}
	g_atomic_int_inc(&g_erase_counter[segnum]);
	if (is_bad == 0) {
		return;
	}
	pr_info("bad segnum: %lu\n", segnum);
	set_bit(g_badseg_bitmap, segnum);
}

static int bluedbm_clear(struct device *dev)
{
	struct bluedbm *bdbm;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;
	struct device_address addr;

	size_t pages_per_segment;
	size_t erase_size;
	uint32_t segnum;

	bdbm = (struct bluedbm *)dev->d_private;
	if (bdbm->mio == NULL) {
		pr_err("mio must be specified.\n");
		return -EINVAL;
	}
	pages_per_segment = device_get_pages_per_segment(dev);
	erase_size = pages_per_segment * page->size;
	for (segnum = 0; segnum < package->nr_blocks; segnum++) {
		addr.lpn = 0;
		addr.format.block = segnum;
		memio_trim(bdbm->mio, addr.lpn, erase_size,
			   bluedbm_erase_end_request);
	}
	return 0;
}

static void bluedbm_wait_erase_finish(struct device *dev, size_t segnum,
				      size_t nr_segments)
{
	size_t blocks_per_segment;
	blocks_per_segment = device_get_blocks_per_segment(dev);
	while (segnum < segnum + nr_segments) {
		gint nr_erased_block;
		int status;

		status = get_bit(g_badseg_bitmap, segnum);
		if (status) {
			segnum++;
			continue;
		}

		nr_erased_block = g_atomic_int_get(&g_erase_counter[segnum]);

		if (nr_erased_block == (gint)blocks_per_segment) {
			g_atomic_int_set(&g_erase_counter[segnum], 0);
			segnum++;
			continue;
		}
		sleep(1);
	}
}

int bluedbm_open(struct device *dev, const char *name, int flags)
{
	struct bluedbm *bdbm;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	int ret;
	size_t nr_segments;

	memio_t *mio;

	(void)name;
	(void)flags;

	mio = memio_open();
	if (mio == NULL) {
		pr_err("memio open failed\n");
		ret = -EFAULT;
		goto exception;
	}

	nr_segments = device_get_nr_segments(dev);

	info->nr_bus = (1 << DEVICE_NR_BUS_BITS);
	info->nr_chips = (1 << DEVICE_NR_CHIPS_BITS);
	block->nr_pages = (1 << DEVICE_NR_PAGES_BITS);
	page->size = DEVICE_PAGE_SIZE;

	package->nr_blocks = BLUEDBM_NR_BLOCKS;

	bdbm = (struct bluedbm *)dev->d_private;
	bdbm->size = device_get_total_size(dev);
	bdbm->o_flags = flags;
	bdbm->mio = mio;

	dev->badseg_bitmap =
		(uint64_t *)malloc(BITS_TO_UINT64_ALIGN(nr_segments));
	if (dev->badseg_bitmap == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(dev->badseg_bitmap, 0, BITS_TO_UINT64_ALIGN(nr_segments));
	g_badseg_bitmap = dev->badseg_bitmap;

	g_erase_counter = (gint *)malloc(nr_segments * sizeof(gint));
	if (g_erase_counter == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(g_erase_counter, 0, nr_segments * sizeof(gint));

	if (bdbm->o_flags & O_CREAT) {
		bluedbm_clear(dev);
		sleep(1);
		bluedbm_wait_erase_finish(dev, 0, nr_segments);
	}

	return 0;
exception:
	bluedbm_close(dev);
	return ret;
}

static void bluedbm_end_rw_request(async_bdbm_req *rw_req)
{
	bluedbm_dma_t *dma;
	struct device_request *user_rq;

	if (rw_req == NULL) {
		pr_warn("NULL rw_req detected\n");
		return;
	}

	dma = (bluedbm_dma_t *)rw_req->private_data;
	if (dma == NULL) {
		pr_warn("NULL request detected (rw_req: %p)\n", rw_req);
		free(rw_req);
		return;
	}
	user_rq = (struct device_request *)dma->d_private;
	assert(NULL != user_rq);

	switch (rw_req->type) {
	case REQTYPE_IO_WRITE:
		memio_free_dma(DMA_WRITE_BUF, dma->tag);
		break;
	case REQTYPE_IO_READ:
		memcpy(user_rq->data, dma->data, user_rq->data_len);
		memio_free_dma(DMA_READ_BUF, dma->tag);
		break;
	default:
		pr_warn("unknown request type detected (flag: %u)",
			rw_req->type);
		break;
	}
	free(dma);
	free(rw_req);

	if (user_rq && user_rq->end_rq) {
		user_rq->end_rq(user_rq);
	}
}

ssize_t bluedbm_write(struct device *dev, struct device_request *request)
{
	bluedbm_dma_t *dma = NULL;
	async_bdbm_req *write_rq = NULL;
	memio_t *mio;

	struct bluedbm *bdbm;

	size_t page_size;
	ssize_t ret = 0;

	uint32_t lpn;

	page_size = device_get_page_size(dev);
	bdbm = (struct bluedbm *)dev->d_private;
	mio = bdbm->mio;

	if (mio == NULL) {
		pr_err("memio global structure doesn't exist\n");
		ret = -EFAULT;
		goto exception;
	}

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		ret = -ENODATA;
		goto exception;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exception;
	}

	if (request->flag != DEVICE_WRITE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_WRITE, request->flag);
		ret = -EINVAL;
		goto exception;
	}

	if (request->data_len != page_size) {
		pr_err("data write size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exception;
	}

	lpn = request->paddr.lpn;

	dma = (bluedbm_dma_t *)malloc(sizeof(bluedbm_dma_t));
	if (dma == NULL) {
		pr_err();
	}
	dma->tag = memio_alloc_dma(DMA_WRITE_BUF, &dma->data);
	dma->d_private = (void *)request;
	memcpy(dma->data, request->data, page_size);

	write_rq = (async_bdbm_req *)malloc(sizeof(async_bdbm_req));
	if (write_rq == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	write_rq->type = REQTYPE_IO_WRITE;
	write_rq->private_data = (void *)dma;
	write_rq->end_req = bluedbm_end_rw_request;

	ret = memio_write(mio, lpn, page_size, (uint8_t *)dma->data, false,
			  (void *)write_rq, dma->tag);
	return ret;
exception:
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	if (write_rq) {
		free(write_rq);
	}
	if (dma) {
		free(dma);
	}
	return ret;
}

ssize_t bluedbm_read(struct device *dev, struct device_request *request)
{
	bluedbm_dma_t *dma = NULL;
	async_bdbm_req *read_rq = NULL;
	memio_t *mio;

	struct bluedbm *bdbm;

	size_t page_size;
	ssize_t ret = 0;

	uint32_t lpn;

	page_size = device_get_page_size(dev);
	bdbm = (struct bluedbm *)dev->d_private;
	mio = bdbm->mio;

	if (mio == NULL) {
		pr_err("memio global structure doesn't exist\n");
		ret = -EFAULT;
		goto exception;
	}

	if (request->data == NULL) {
		pr_err("you do not pass the data pointer to NULL\n");
		ret = -ENODATA;
		goto exception;
	}

	if (request->flag != DEVICE_READ) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_READ, request->flag);
		ret = -EINVAL;
		goto exception;
	}
	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exception;
	}
	if (request->data_len != page_size) {
		pr_err("data read size is must be %zu (current: %zu)\n",
		       request->data_len, page_size);
		ret = -EINVAL;
		goto exception;
	}

	lpn = request->paddr.lpn;

	dma = (bluedbm_dma_t *)malloc(sizeof(bluedbm_dma_t));
	if (dma == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	dma->tag = memio_alloc_dma(DMA_READ_BUF, &dma->data);
	dma->d_private = (void *)request;

	read_rq = (async_bdbm_req *)malloc(sizeof(async_bdbm_req));
	if (read_rq == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	read_rq->type = REQTYPE_IO_READ;
	read_rq->private_data = (void *)dma;
	read_rq->end_req = bluedbm_end_rw_request;

	ret = memio_read(mio, lpn, page_size, (uint8_t *)dma->data, false,
			 (void *)read_rq, dma->tag);
	return ret;
exception:
	if (request && request->end_rq) {
		request->end_rq(request);
	}
	if (read_rq) {
		free(read_rq);
	}
	if (dma) {
		free(dma);
	}
	return ret;
}

int bluedbm_erase(struct device *dev, struct device_request *request)
{
	struct bluedbm *bdbm;
	memio_t *mio;
	struct device_address addr = request->paddr;
	size_t page_size;
	size_t segnum;
	uint32_t pages_per_segment;
	uint32_t lpn;
	size_t erase_size;
	int ret = 0;

	bdbm = (struct bluedbm *)dev->d_private;
	mio = bdbm->mio;

	if (mio == NULL) {
		pr_err("memio global structure doesn't exist\n");
		ret = -EFAULT;
		goto exception;
	}

	if (request->flag != DEVICE_ERASE) {
		pr_err("request type is not matched (expected: %u, current: %u)\n",
		       (unsigned int)DEVICE_ERASE, request->flag);
		ret = -EINVAL;
		goto exception;
	}

	if (request->paddr.lpn == PADDR_EMPTY) {
		pr_err("physical address is not specified...\n");
		ret = -EINVAL;
		goto exception;
	}

	page_size = device_get_page_size(dev);
	pages_per_segment = (uint32_t)device_get_pages_per_segment(dev);
	segnum = addr.format.block;
	addr.lpn = 0;
	addr.format.block = segnum;

	if (request->end_rq) {
		request->end_rq(request);
	}
	erase_size = pages_per_segment * page_size;
	memio_trim(mio, addr.lpn, erase_size, bluedbm_erase_end_request);
	bluedbm_wait_erase_finish(dev, segnum, 1);
	return ret;
exception:
	if (request->end_rq) {
		request->end_rq(request);
	}
	return ret;
}

int bluedbm_close(struct device *dev)
{
	struct bluedbm *bdbm;
	bdbm = (struct bluedbm *)dev->d_private;
	if (bdbm == NULL) {
		return 0;
	}

	if (bdbm->mio) {
		memio_close(bdbm->mio);
		bdbm->mio = NULL;
	}

	if (dev->badseg_bitmap) {
		free(dev->badseg_bitmap);
		g_badseg_bitmap = NULL;
	}

	if (g_erase_counter) {
		free(g_erase_counter);
		g_erase_counter = NULL;
	}

	return 0;
}

const struct device_operations __bluedbm_dops = {
	.open = bluedbm_open,
	.write = bluedbm_write,
	.read = bluedbm_read,
	.erase = bluedbm_erase,
	.close = bluedbm_close,
};

int bluedbm_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct bluedbm *bdbm;

	(void)flags;
	bdbm = (struct bluedbm *)malloc(sizeof(struct bluedbm));
	if (bdbm == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(bdbm, 0, sizeof(struct bluedbm));
	dev->d_op = &__bluedbm_dops;
	dev->d_private = (void *)bdbm;
	dev->d_submodule_exit = bluedbm_device_exit;
	return ret;
exception:
	bluedbm_device_exit(dev);
	return ret;
}

int bluedbm_device_exit(struct device *dev)
{
	struct bluedbm *bdbm;
	bdbm = (struct bluedbm *)dev->d_private;
	if (bdbm) {
		bluedbm_close(dev);
		free(bdbm);
		dev->d_private = NULL;
	}
	return 0;
}
