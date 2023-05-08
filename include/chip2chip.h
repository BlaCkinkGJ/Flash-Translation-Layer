/**
 * @file chip2chip.h
 * @brief chip2chip module's header file
 * @author Sungjune Yune
 * @version 0.2
 * @date 2023-04-27
 */
#ifndef CHIP2CHIP_H
#define CHIP2CHIP_H

#include <stdint.h>
//#include <libmemio.h>

#include "device.h"
#include "chip2chip_base.h"

#define CHIP2CHIP_NR_BLOCKS                                                      \
	(4096) /**< number of blocks(segments) in the flash board 
			using same value of BLUEDBM_NR_BLOCKS
		 	may need to be changed afterwards */

/**
 * @brief structure for manage the dma
 */
/*
typedef struct {
	uint32_t tag;
	char *data;
	void *d_private;
} chip2chip_dma_t;
*/

/**
 * @brief a structure for managing chip2chip device
 */
struct chip2chip {
	size_t size;	//total flash size, initialized in chip2chip_open 
	//memio_t *mio;
	int o_flags;
	u64 *readData_upper_arr; //buffers for chip2chip RW interface
	u64 *readData_lower_arr;
	u64 *writeData_upper_arr;
	u64 *writeData_lower_arr;
};

int chip2chip_open(struct device *, const char *name, int flags);
ssize_t chip2chip_write(struct device *, struct device_request *);
ssize_t chip2chip_read(struct device *, struct device_request *);
int chip2chip_erase(struct device *, struct device_request *);
int chip2chip_close(struct device *);

int chip2chip_device_init(struct device *, uint64_t flags);
int chip2chip_device_exit(struct device *);

#endif
