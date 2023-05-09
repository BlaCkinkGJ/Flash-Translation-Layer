#ifndef CHIP2CHIPBASE_H
#define CHIP2CHIPBASE_H

#include <stdio.h>
//#include "xil_printf.h"
//#include "xil_io.h"
//#include "xparameters.h"
#include <stdlib.h>
//#include "xuartps.h"
//#include <xil_types.h>
#include <stdint.h>
#include <stddef.h>

#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>

/* register address*/
#define C2C_BASE_ADDR 				0xa0000000
#define READQ_ADDR_INTERVAL			0x10
#define CMD_OFFSET 					0x00
#define READ_STATUS_OFFSET 			0x10
#define READ_DATA0_UPPER_OFFSET 	0x20
#define READ_DATA0_LOWER_OFFSET 	0x28
#define READ_DATA1_UPPER_OFFSET 	0x30
#define READ_DATA1_LOWER_OFFSET 	0x38
#define READ_DATA2_UPPER_OFFSET 	0x40
#define READ_DATA2_LOWER_OFFSET 	0x48
#define READ_DATA3_UPPER_OFFSET 	0x50
#define READ_DATA3_LOWER_OFFSET 	0x58
#define READ_DATA4_UPPER_OFFSET 	0x60
#define READ_DATA4_LOWER_OFFSET 	0x68
#define READ_DATA5_UPPER_OFFSET 	0x70
#define READ_DATA5_LOWER_OFFSET 	0x78
#define READ_DATA6_UPPER_OFFSET 	0x80
#define READ_DATA6_LOWER_OFFSET 	0x88
#define READ_DATA7_UPPER_OFFSET 	0x90
#define READ_DATA7_LOWER_OFFSET 	0x98
#define WRITE_ERASE_STATUS_OFFSET 	0xa0
#define WRITE_DATA_UPPER_OFFSET 	0xb0
#define WRITE_DATA_LOWER_OFFSET 	0xb8

#define C2C_CMD_ADDR 			C2C_BASE_ADDR + CMD_OFFSET			//reg0
#define C2C_READ_STATUS_ADDR 		C2C_BASE_ADDR + READ_STATUS_OFFSET		//reg2
#define C2C_READ_DATA0_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA0_UPPER_OFFSET		//reg4
#define C2C_READ_DATA0_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA0_LOWER_OFFSET		//reg5
#define C2C_READ_DATA1_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA1_UPPER_OFFSET		//reg6
#define C2C_READ_DATA1_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA1_LOWER_OFFSET		//reg7
#define C2C_READ_DATA2_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA2_UPPER_OFFSET		//reg8
#define C2C_READ_DATA2_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA2_LOWER_OFFSET		//reg9
#define C2C_READ_DATA3_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA3_UPPER_OFFSET		//reg10
#define C2C_READ_DATA3_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA3_LOWER_OFFSET		//reg11
#define C2C_READ_DATA4_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA4_UPPER_OFFSET		//reg12
#define C2C_READ_DATA4_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA4_LOWER_OFFSET		//reg13
#define C2C_READ_DATA5_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA5_UPPER_OFFSET		//reg14
#define C2C_READ_DATA5_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA5_LOWER_OFFSET		//reg15
#define C2C_READ_DATA6_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA6_UPPER_OFFSET		//reg16
#define C2C_READ_DATA6_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA6_LOWER_OFFSET		//reg17
#define C2C_READ_DATA7_UPPER_ADDR	C2C_BASE_ADDR + READ_DATA7_UPPER_OFFSET		//reg18
#define C2C_READ_DATA7_LOWER_ADDR 	C2C_BASE_ADDR + READ_DATA7_LOWER_OFFSET		//reg19
#define C2C_WRITE_ERASE_STATUS_ADDR 	C2C_BASE_ADDR + WRITE_ERASE_STATUS_OFFSET	//reg20
#define C2C_WRITE_DATA_UPPER_ADDR	C2C_BASE_ADDR + WRITE_DATA_UPPER_OFFSET		//reg22
#define C2C_WRITE_DATA_LOWER_ADDR 	C2C_BASE_ADDR + WRITE_DATA_LOWER_OFFSET		//reg23

/* command*/
#define ACCESS_KEY 				0x1E5A53
#define READ 					0x3
#define WRITE 					0x4
#define ERASE 					0x5
#define TAG_BIT 				33
#define KEY_BIT 				42
#define OP_BIT 					30
#define BUS_BIT 				27
#define CHIP_BIT 				24
#define BLOCK_BIT 				8
#define PAGE_BIT 				0
#define CMD_READY_BIT 			40
#define CMD_READY_MASK 			0x0000010000000000

/* acknowledge*/
#define ACK_ERASE_ERR 			0
#define ACK_ERASE_DONE 			1
#define ACK_WRITE_DONE 			2
#define ACK_BIT 			9
#define ACK_TAG_BIT 			11
#define ACK_CODE_MASK 			0x0000000000000600
#define ACK_TAG_MASK 			0x000000000003f800
#define WR_ER_ACK_READY_MASK 		0x0000000000040000
#define WR_ER_ACK_VALID	 		0x0000000000080000

#define VALID 				1
#define INVALID 			0

/* write*/
#define WR_DATA_TAG_BIT 		20
#define WR_DATA_TAG_VALID_BIT 	27
#define WAIT_WRDATA_READY_BIT 	41
#define WR_DATA_READY_MASK 		0x0000020000000000
#define WR_DATA_REQ_READY_MASK 	0x0000000000000080
#define WR_DATA_REQ_TAG_MASK 	0x000000000000007f
#define WR_DATA_REQ_VALID	 	0x0000000000000100
#define WR_DATA_REQ_TAG_BIT 	0
/* read */
#define READQ_READY_MASK 		0x8080808080808080
#define READQ_TAG_MASK 			0x7f7f7f7f7f7f7f7f
#define READ_DATA0_READY_VALUE 	0x0000000000000080
#define READ_DATA1_READY_VALUE 	0x0000000000008000
#define READ_DATA2_READY_VALUE 	0x0000000000800000
#define READ_DATA3_READY_VALUE 	0x0000000080000000
#define READ_DATA4_READY_VALUE 	0x0000008000000000
#define READ_DATA5_READY_VALUE 	0x0000800000000000
#define READ_DATA6_READY_VALUE 	0x0080000000000000
#define READ_DATA7_READY_VALUE 	0x8000000000000000
#define READ_DATA0_TAG_MASK		0x000000000000007f
#define READ_DATA1_TAG_MASK		0x0000000000007f00
#define READ_DATA2_TAG_MASK		0x00000000007f0000
#define READ_DATA3_TAG_MASK		0x000000007f000000
#define READ_DATA4_TAG_MASK		0x0000007f00000000
#define READ_DATA5_TAG_MASK		0x00007f0000000000
#define READ_DATA6_TAG_MASK		0x007f000000000000
#define READ_DATA7_TAG_MASK		0x7f00000000000000
#define READ_DATA0_TAG_BIT 		0
#define READ_DATA1_TAG_BIT 		8
#define READ_DATA2_TAG_BIT 		16
#define READ_DATA3_TAG_BIT 		24
#define READ_DATA4_TAG_BIT 		32
#define READ_DATA5_TAG_BIT 		40
#define READ_DATA6_TAG_BIT 		48
#define READ_DATA7_TAG_BIT 		56

/* max waiting time*/
#define MAX_WR_DATA_REQ_WAIT_CNT 	100
#define MAX_READ_STATUS_WAIT_CNT 	100
#define MAX_WRITE_STATUS_WAIT_CNT 	10000
#define MAX_ERASE_STATUS_WAIT_CNT 	100000
#define MAX_CMD_RDY_WAIT_CNT 		10000
#define MAX_WR_DATA_READY_CNT 		10000

#define REG_SIZE			0xA8


typedef unsigned long long u64;

struct _rgstr_vptr {
	u64 *cmd;		//Command register
	u64 *read_stat;		//Read status register
	u64 *read_data_0u;	//Read data upper register
	u64 *read_data_0l;	//Read data lower register
	u64 *read_data_1u;
	u64 *read_data_1l;
	u64 *read_data_2u;
	u64 *read_data_2l;
	u64 *read_data_3u;
	u64 *read_data_3l;
	u64 *read_data_4u;
	u64 *read_data_4l;
	u64 *read_data_5u;
	u64 *read_data_5l;
	u64 *read_data_6u;
	u64 *read_data_6l;
	u64 *read_data_7u;
	u64 *read_data_7l;
	u64 *wne_stat;		//Write and erase status register

	u64 *write_data_u;	//Write data upper register
	u64 *write_data_l;	//Write data lower register
};


void vptr_mmap(u64** vptr, off_t addr);

void rgstr_offset_map(u64** vptr, u64 offset);

int c2c_init(void);	//opening memory device as a file descriptor to use them with mmap/msync

int c2c_terminate(void); //Closing the file descriptor

void CTC_Out(u64* vptr, u64 command); //Replacement for "Xil_Out64()" function

u64 CTC_In(u64* vptr); //Replacement for "Xil_In64()" function

u64 CTC_Readq_In_Upper(u64 Qnumber);

u64 CTC_Readq_In_Lower(u64 Qnumber);

int read_page(u64 bus, u64 chip, u64 block, u64 page, 
		u64* pReadBuf_upper, u64* pReadBuf_lower, size_t page_size);
		
int write_page(u64 bus, u64 chip, u64 block, u64 page,
		/*cosnt*/ u64* pWriteBuf_upper, /*cosnt*/ u64* pWriteBuf_lower, size_t page_size);

int erase_block(u64 bus, u64 chip, u64 block);

int wait_cmd_ready(void);

int wait_wrData_ready(void);

int wait_writeData_req(u64* requested_tag);

int wait_flash_operation(u64 op, u64 tag, int* Qnumber, u64* ack,u64* ack_tag);

#endif
