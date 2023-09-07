/**
 * @file chip2chip.c
 * @brief implementation of the chip2chip abstraction layer which is inherited by the device
 * @author Sungjune Yune
 * @version 0.2
 * @date 2023-04-27
 */
#include <stdlib.h>
#include <errno.h>
#include <pthread.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>
#include <string.h>

#include <malloc.h>

#include "chip2chip.h"
#include "device.h"
#include "log.h"
#include "bits.h"

gint *g_badseg_counter = NULL; /**< counter for bad segemnt detection */
gint *g_erase_counter = NULL; /**< counter for # of erase in the segment*/

/**
 * @brief end request for the erase
 *
 * @param segnum erased segment number
 * @param is_bad erased segment is bad segment or not
 *
 * @note
 * Do not use the non atomic operation or complex operation in this routine.
 * It may occur serious problem.
 */
static void chip2chip_erase_end_request(uint64_t segnum, uint8_t is_bad)
{
	if (g_badseg_counter == NULL || g_erase_counter == NULL) {
		pr_err("NULL pointer detected\n");
		pr_err("\tg_badseg_counter: %p\n", g_badseg_counter);
		pr_err("\tg_erase_counter: %p\n", g_erase_counter);
		return;
	}
	g_atomic_int_inc(&g_erase_counter[segnum]);
	if (is_bad) {
		g_atomic_int_inc(&g_badseg_counter[segnum]);
	}
}


/**
 * @brief copy data from a c2c buffer to a buffer given by FTL
 *
 * @param read_upper c2c upper buffer for reading
 * @param read_lower c2c lower buffer for reading
 * @param buffer a buffer given by FTL(page size)
 * @param page_size FTL page size
 *
 */ 
static void read_buffer_cpy(u64* read_upper, u64* read_lower, u64* buffer, size_t page_size)
{
	for(size_t i = 0; i < (page_size / (2 * sizeof(u64))); i++) {
		buffer[2*i] = read_upper[i];
		buffer[2*i+1] = read_lower[i];
	}
}

/**
 * @brief copy data from a buffer given by FTL to a c2c buffer
 *
 * @param write_upper c2c upper buffer for writing
 * @param write_lower c2c lower buffer for writing
 * @param buffer a buffer given by FTL(page size)
 * @param page_size FTL page size
 *
 */ 
static void write_buffer_cpy(u64* write_upper, u64* write_lower, u64* buffer, size_t page_size)
{
	for(size_t i = 0; i < (page_size / (2 * sizeof(u64))); i++) {
		write_upper[i] = buffer[2*i];
		write_lower[i] = buffer[2*i+1];
	}
}


/**
 * @brief clear all segments in the flash board
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
static int chip2chip_clear(struct device *dev)
{
	struct chip2chip *c2c;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;
	struct device_address addr;


	size_t busnum = info->nr_bus;
	size_t chipnum = info->nr_chips;
	size_t blocknum = package->nr_blocks;

	int result = 0;	

	c2c = (struct chip2chip *)dev->d_private;
	
	//clear all segments : (bus * chip) * block
	for(size_t block = 0; block < blocknum; block++) {
		for(size_t bus = 0; bus < busnum; bus++) {
			for(size_t chip = 0; chip < chipnum; chip++) {
				result = erase_block((u64)bus, (u64)chip, (u64)block);
				if(result == -1) {
          set_bit(dev->badseg_bitmap, block); 
        }
			}
		}
	}

  for(size_t block = 0; block < blocknum; block++) {
    if(get_bit(dev->badseg_bitmap, block))
      pr_info("\n\tbadseg : %d\n\tbadseg_bitmap : %d\n", block, get_bit(dev->badseg_bitmap, block));
  }

	return 0;
}

/**
 * @brief wait the erase is finished
 *
 * @param dev pointer of the device structure
 * @param segnum initial position to erase target segment
 * @param nr_segments number of segments to erase
 *
 * @note
 * If you fail to execute the erase the target segment,
 * this function may block the thread forever.
 */


static void chip2chip_wait_erase_finish(struct device *dev, size_t segnum,
				      size_t nr_segments)
{
	size_t blocks_per_segment;
	size_t end_segment = segnum + nr_segments;
	blocks_per_segment = device_get_blocks_per_segment(dev);
	while (segnum < end_segment) {
		gint nr_erased_block;
		gint status;

		status = g_atomic_int_get(&g_badseg_counter[segnum]);
		if (status) {
			set_bit(dev->badseg_bitmap, segnum);
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



/**
 * @brief open the chip2chip based device
 *
 * @param dev pointer of the device structure
 * @param name this does not use in this module
 * @param flags open flags for this module
 *
 * @return 0 for success, negative value to fail
 */
int chip2chip_open(struct device *dev, const char *name, int flags)
{
	struct chip2chip *c2c;

	struct device_info *info = &dev->info;
	struct device_package *package = &info->package;
	struct device_block *block = &package->block;
	struct device_page *page = &block->page;

	int ret;
	size_t nr_segments;

	(void)name;
	info->nr_bus = (1 << DEVICE_NR_BUS_BITS);
	info->nr_chips = (1 << DEVICE_NR_CHIPS_BITS);
	block->nr_pages = (1 << DEVICE_NR_PAGES_BITS);
	page->size = DEVICE_PAGE_SIZE;

	package->nr_blocks = CHIP2CHIP_NR_BLOCKS;

	nr_segments = device_get_nr_segments(dev);
	
	c2c = (struct chip2chip *)dev->d_private;
	c2c->size = device_get_total_size(dev);
	c2c->o_flags = flags;

	dev->badseg_bitmap =
		(uint64_t *)malloc(BITS_TO_UINT64_ALIGN(nr_segments));
	if (dev->badseg_bitmap == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(dev->badseg_bitmap, 0, BITS_TO_UINT64_ALIGN(nr_segments));

	g_erase_counter = (gint *)malloc(nr_segments * sizeof(gint));
	if (g_erase_counter == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	memset(g_erase_counter, 0, nr_segments * sizeof(gint));

	g_badseg_counter = (gint *)malloc(nr_segments * sizeof(gint));
	if (g_badseg_counter == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}

	if (c2c->o_flags & O_CREAT) {
		//chip2chip_clear(dev);
		//sleep(1);
		//chip2chip_wait_erase_finish(dev, 0, nr_segments);
	}

	return 0;
exception:
	chip2chip_close(dev);
	return ret;
}

/**
 * @brief end request for the read/write
 *
 * @param request user request request pointer
 */
static void chip2chip_end_rw_request(struct device_request *request)
{
	struct device_request *user_rq;

	if (request == NULL) {
		pr_warn("NULL request detected\n");
		return;
	}

	user_rq = (struct device_request *)request;
	assert(NULL != user_rq);

	if (user_rq && user_rq->end_rq) {
		user_rq->end_rq(user_rq);
	}
}

/**
 * @brief write to the flash board
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return written size (bytes)
 */
ssize_t chip2chip_write(struct device *dev, struct device_request *request)
{
	struct chip2chip *c2c;

	size_t page_size;
	int result = -1; // result of the core write function(write_page)
	ssize_t ret = 0;

	uint32_t lpn;

	page_size = device_get_page_size(dev);
	c2c = (struct chip2chip *)dev->d_private;

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

	write_buffer_cpy(c2c->writeData_upper_arr, c2c->writeData_lower_arr,
			(u64*)request->data, page_size);

	result = write_page((u64)request->paddr.format.bus,
			(u64)request->paddr.format.chip,
			(u64)request->paddr.format.block,
			(u64)request->paddr.format.page,
			c2c->writeData_upper_arr,
			c2c->writeData_lower_arr,
			page_size
			);

	if(result == -1)
		goto exception;
	chip2chip_end_rw_request(request);	
	ret = page_size;
	
	return ret;
exception:
	/*
	if (write_rq) {
		free(write_rq);
	}
	*/
	return ret;
}

/**
 * @brief read from the flash board
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return read size (bytes)
 */
ssize_t chip2chip_read(struct device *dev, struct device_request *request)
{
	struct chip2chip *c2c;

	size_t page_size;
	int result = -1;//result of the core read function(read_page)
	ssize_t ret = 0;

	uint32_t lpn;

	page_size = device_get_page_size(dev);
	c2c = (struct chip2chip *)dev->d_private;

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
	
	result = read_page((u64)request->paddr.format.bus,
			(u64)request->paddr.format.chip,
			(u64)request->paddr.format.block,
			(u64)request->paddr.format.page,
			c2c->readData_upper_arr,
			c2c->readData_lower_arr,
			page_size
			);

	read_buffer_cpy(c2c->readData_upper_arr, c2c->readData_lower_arr,
			(u64*)request->data, page_size);
	
	chip2chip_end_rw_request(request);	
	
	if(result == -1)
		goto exception;
	ret = page_size;

	return ret;
exception:
	/*
	if (read_rq) {
		free(read_rq);
	}
	*/
	return ret;
}

/**
 * @brief erase a segment
 *
 * @param dev pointer of the device structure
 * @param request pointer of the device request structure
 *
 * @return 0 for success, negative value for fail
 */
int chip2chip_erase(struct device *dev, struct device_request *request)
{
	struct chip2chip *c2c;
	
	struct device_address addr = request->paddr;
	//size_t page_size;
	size_t segnum;
	/*
	uint32_t pages_per_segment;
	uint32_t lpn;
	size_t erase_size;
	*/
	//int result = -1;
	size_t busnum = dev->info.nr_bus;
	size_t chipnum = dev->info.nr_chips;
	size_t page_size = device_get_page_size(dev); 
	int ret = 0;

	pthread_mutex_t *mutex;

	c2c = (struct chip2chip *)dev->d_private;

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
	segnum = addr.format.block;
	
	if (request->end_rq) {
		request->end_rq(request);
	}
	
	mutex = &c2c->iomutex;

	for(size_t bus = 0; bus < busnum; bus++) {
		for(size_t chip = 0; chip < chipnum; chip++) {
			pthread_mutex_lock(mutex);
			ret = erase_block((u64)bus, (u64)chip,
				(u64)request->paddr.format.block
				);
			pthread_mutex_unlock(mutex);
			if(ret == -1)
				goto exception;
		}
	}
	chip2chip_wait_erase_finish(dev, segnum, 1); //may need to be removed if FTL doesn't function
	ret = busnum * chipnum * page_size;		
	return ret;
exception:
	return ret;
}

/**
 * @brief close the bluedbm
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int chip2chip_close(struct device *dev)
{
	struct chip2chip *c2c;
	c2c = (struct chip2chip *)dev->d_private;
	if (c2c == NULL) {
		return 0;
	}

	if (c2c->readData_upper_arr) {
		free(c2c->readData_upper_arr);
		c2c->readData_upper_arr = NULL;
	}
	if (c2c->readData_lower_arr) {
		free(c2c->readData_lower_arr);
		c2c->readData_lower_arr = NULL;
	}
	if (c2c->writeData_upper_arr) {
		free(c2c->writeData_upper_arr);
		c2c->writeData_upper_arr = NULL;
	}
	if (c2c->writeData_lower_arr) {
		free(c2c->writeData_lower_arr);
		c2c->writeData_lower_arr = NULL;
	}

	if (dev->badseg_bitmap) {
		free(dev->badseg_bitmap);
		dev->badseg_bitmap = NULL;
	}

	if (g_erase_counter) {
		free(g_erase_counter);
		g_erase_counter = NULL;
	}

	if (g_badseg_counter) {
		free(g_badseg_counter);
		g_badseg_counter = NULL;
	}

	return 0;
}

/**
 * @brief chip2chip module operations
 */
const struct device_operations __chip2chip_dops = {
	.open = chip2chip_open,
	.write = chip2chip_write,
	.read = chip2chip_read,
	.erase = chip2chip_erase,
	.close = chip2chip_close,
};

/**
 * @brief initialize the device and chip2chip module
 *
 * @param dev pointer of the device structure
 * @param flags flags for bluedbm and device
 *
 * @return 0 for success, negative value for fail
 */
int chip2chip_device_init(struct device *dev, uint64_t flags)
{
	int ret = 0;
	struct chip2chip *c2c;
	size_t page_size;
	int core_init;
	pr_info("chip2chip_device_init\n");
	(void)flags;
	c2c = (struct chip2chip *)malloc(sizeof(struct chip2chip));
	if (c2c == NULL) {
		pr_err("memory allocation failed\n");
		ret = -ENOMEM;
		goto exception;
	}
	page_size = device_get_page_size(dev);
	memset(c2c, 0, sizeof(struct chip2chip));

	//initialization from chip2chip_core.h
	core_init = c2c_init();
	if(core_init == -1)
		goto exception;

	//set R/W upper/lower buffers for chip2chip RW interface.
	//size of the buffer shall be changed with "page_size/2".
	//buffer allocation may need to be moved to "chip2chip_open()" function.
	c2c->readData_upper_arr = (u64*)malloc(8224/2);
	c2c->readData_lower_arr = (u64*)malloc(8224/2);
	c2c->writeData_upper_arr = (u64*)malloc(8224/2);
	c2c->writeData_lower_arr = (u64*)malloc(8224/2);

	pthread_mutex_init(&(c2c->iomutex), NULL);

	dev->d_op = &__chip2chip_dops;
	dev->d_private = (void *)c2c;
	dev->d_submodule_exit = chip2chip_device_exit;
	return ret;
exception:
	chip2chip_device_exit(dev);
	return ret;
}

/**
 * @brief deallocate the device module
 *
 * @param dev pointer of the device structure
 *
 * @return 0 for success, negative value for fail
 */
int chip2chip_device_exit(struct device *dev)
{
	struct chip2chip *c2c;
	int core_terminate;	
	//termination from chip2chip_core.h
	//has return value but not in use currently.
	core_terminate = c2c_terminate();

	c2c = (struct chip2chip *)dev->d_private;
	if (c2c) {
		chip2chip_close(dev);
		free(c2c);
		dev->d_private = NULL;
	}
	return 0;
}
