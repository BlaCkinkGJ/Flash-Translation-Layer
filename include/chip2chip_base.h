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

#define C2C_CMD_ADDR 				C2C_BASE_ADDR + CMD_OFFSET					//reg0
#define C2C_READ_STATUS_ADDR 		C2C_BASE_ADDR + READ_STATUS_OFFSET			//reg2
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
#define C2C_WRITE_ERASE_STATUS_ADDR C2C_BASE_ADDR + WRITE_ERASE_STATUS_OFFSET	//reg20
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
#define ACK_BIT 				9
#define ACK_TAG_BIT 			11
#define ACK_CODE_MASK 			0x0000000000000600
#define ACK_TAG_MASK 			0x000000000003f800
#define WR_ER_ACK_READY_MASK 	0x0000000000040000
#define WR_ER_ACK_VALID	 		0x0000000000080000

#define VALID 					1
#define INVALID 				0

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

int fd_memory;		//File descriptor of the memory device

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
} rgstr_vptr;

void vptr_mmap(u64** vptr, off_t addr) { //Mapping registers to the host's memory
	return (*vptr = (u64 *)mmap(NULL, REG_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd_memory, addr));
}

void rgstr_offset_map(u64** vptr, u64 offset) {
	*vptr = rgstr_vptr.cmd + offset/sizeof(u64*);
}

int c2c_init(void) {	//opening memory device as a file descriptor to use them with mmap/msync
	
	if((fd_memory = open("/dev/mem", O_RDWR | O_SYNC)) != -1) { //"open" the memory device
		//assign vptr with mmap function
		
		vptr_mmap(&rgstr_vptr.cmd, C2C_CMD_ADDR);
		/*
		vptr_mmap(&rgstr_vptr.read_stat, C2C_READ_STATUS_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_0u, C2C_READ_DATA0_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_0l, C2C_READ_DATA0_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_1u, C2C_READ_DATA1_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_1l, C2C_READ_DATA1_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_2u, C2C_READ_DATA2_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_2l, C2C_READ_DATA2_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_3u, C2C_READ_DATA3_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_3l, C2C_READ_DATA3_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_4u, C2C_READ_DATA4_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_4l, C2C_READ_DATA4_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_5u, C2C_READ_DATA5_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_5l, C2C_READ_DATA5_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_6u, C2C_READ_DATA6_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_6l, C2C_READ_DATA6_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_7u, C2C_READ_DATA7_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.read_data_7l, C2C_READ_DATA7_LOWER_ADDR);
		vptr_mmap(&rgstr_vptr.wne_stat, C2C_WRITE_ERASE_STATUS_ADDR);
		vptr_mmap(&rgstr_vptr.write_data_u, C2C_WRITE_DATA_UPPER_ADDR);
		vptr_mmap(&rgstr_vptr.write_data_l, C2C_WRITE_DATA_LOWER_ADDR);
		*/
		
		rgstr_offset_map(&rgstr_vptr.read_stat, READ_STATUS_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_0u, READ_DATA0_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_0l, READ_DATA0_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_1u, READ_DATA1_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_1l, READ_DATA1_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_2u, READ_DATA2_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_2l, READ_DATA2_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_3u, READ_DATA3_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_3l, READ_DATA3_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_4u, READ_DATA4_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_4l, READ_DATA4_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_5u, READ_DATA5_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_5l, READ_DATA5_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_6u, READ_DATA6_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_6l, READ_DATA6_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_7u, READ_DATA7_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.read_data_7l, READ_DATA7_LOWER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.wne_stat, WRITE_ERASE_STATUS_OFFSET);
		rgstr_offset_map(&rgstr_vptr.write_data_u, WRITE_DATA_UPPER_OFFSET);
		rgstr_offset_map(&rgstr_vptr.write_data_l, WRITE_DATA_LOWER_OFFSET);

		/*
		 rgstr_vptr.read_stat = rgstr_vptr.cmd + READ_STATUS_OFFSET;
		 rgstr_vptr.read_data_0u = rgstr_vptr.cmd + READ_DATA0_UPPER_OFFSET;
		 rgstr_vptr.read_data_0l = rgstr_vptr.cmd + READ_DATA0_LOWER_OFFSET;
		 rgstr_vptr.read_data_1u = rgstr_vptr.cmd + READ_DATA1_UPPER_OFFSET;
		 rgstr_vptr.read_data_1l = rgstr_vptr.cmd + READ_DATA1_LOWER_OFFSET;
		 rgstr_vptr.read_data_2u = rgstr_vptr.cmd + READ_DATA2_UPPER_OFFSET;
		 rgstr_vptr.read_data_2l = rgstr_vptr.cmd + READ_DATA2_LOWER_OFFSET;
		 rgstr_vptr.read_data_3u = rgstr_vptr.cmd + READ_DATA3_UPPER_OFFSET;
		 rgstr_vptr.read_data_3l = rgstr_vptr.cmd + READ_DATA3_LOWER_OFFSET;
		 rgstr_vptr.read_data_4u = rgstr_vptr.cmd + READ_DATA4_UPPER_OFFSET;
		 rgstr_vptr.read_data_4l = rgstr_vptr.cmd + READ_DATA4_LOWER_OFFSET;
		 rgstr_vptr.read_data_5u = rgstr_vptr.cmd + READ_DATA5_UPPER_OFFSET;
		 rgstr_vptr.read_data_5l = rgstr_vptr.cmd + READ_DATA5_LOWER_OFFSET;
		 rgstr_vptr.read_data_6u = rgstr_vptr.cmd + READ_DATA6_UPPER_OFFSET;
		 rgstr_vptr.read_data_6l = rgstr_vptr.cmd + READ_DATA6_LOWER_OFFSET;
		 rgstr_vptr.read_data_7u = rgstr_vptr.cmd + READ_DATA7_UPPER_OFFSET;
		 rgstr_vptr.read_data_7l = rgstr_vptr.cmd + READ_DATA7_LOWER_OFFSET;
		 rgstr_vptr.wne_stat = rgstr_vptr.cmd + WRITE_ERASE_STATUS_OFFSET;
		 rgstr_vptr.write_data_u = rgstr_vptr.cmd + WRITE_DATA_UPPER_OFFSET;
		 rgstr_vptr.write_data_l = rgstr_vptr.cmd + WRITE_DATA_LOWER_OFFSET;
		*/

		/*for(int i = 0; i < sizeof(rgstr_vptr) - 2; i++) {
			if(vptr_mmap( ((&rgstr_vptr) + sizeof(u64*) * i), C2C_BASE_ADDR + (0x08 * i)) == -1)
				return -1;
		}
		if(vptr_mmap( ((&rgstr_vptr) + sizeof(u64*) * 19), C2C_WRITE_DATA_UPPER_ADDR) == -1)
			return -1;
		if(vptr_mmap( ((&rgstr_vptr) + sizeof(u64*) * 20), C2C_WRITE_DATA_LOWER_ADDR) == -1)
			return -1;
		*/
		return 0;
		
	} else {
		return -1;
	}
}

int c2c_terminate(void) { //Closing the file descriptor
	return close(fd_memory);
}

void CTC_Out(u64* vptr, u64 command) { //Replacement for "Xil_Out64()" function
	*vptr = command;
	msync(vptr, REG_SIZE, MS_SYNC);
}

u64 CTC_In(u64* vptr) { //Replacement for "Xil_In64()" function
	return *vptr;
}

u64 CTC_Readq_In_Upper(u64 Qnumber) {
	switch(Qnumber) {
		case 0:
			return CTC_In(rgstr_vptr.read_data_0u);
		case 1:
			return CTC_In(rgstr_vptr.read_data_1u);
		case 2:
			return CTC_In(rgstr_vptr.read_data_2u);
		case 3:
			return CTC_In(rgstr_vptr.read_data_3u);
		case 4:
			return CTC_In(rgstr_vptr.read_data_4u);
		case 5:
			return CTC_In(rgstr_vptr.read_data_5u);
		case 6:
			return CTC_In(rgstr_vptr.read_data_6u);
		case 7:
			return CTC_In(rgstr_vptr.read_data_7u);
	}
}

u64 CTC_Readq_In_Lower(u64 Qnumber) {
	switch(Qnumber) {
		case 0:
			return CTC_In(rgstr_vptr.read_data_0l);
		case 1:
			return CTC_In(rgstr_vptr.read_data_1l);
		case 2:
			return CTC_In(rgstr_vptr.read_data_2l);
		case 3:
			return CTC_In(rgstr_vptr.read_data_3l);
		case 4:
			return CTC_In(rgstr_vptr.read_data_4l);
		case 5:
			return CTC_In(rgstr_vptr.read_data_5l);
		case 6:
			return CTC_In(rgstr_vptr.read_data_6l);
		case 7:
			return CTC_In(rgstr_vptr.read_data_7l);
	}
}

int read_page(u64 bus, u64 chip, u64 block, u64 page, u64* pReadBuf_upper, u64* pReadBuf_lower){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = READ; // operation�� read�� ����
	u64 command;
	u64 readData_upper; // read queue���� �ѹ� dequeue�� ���� �޾ƿ� upper ����
	u64 readData_lower;	// read queue���� �ѹ� dequeue�� ���� �޾ƿ� lower ����
	int readyQ_number;
	int i = 0;
	/* command ���� */
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT)				\
			|(page << PAGE_BIT);
	/* flash controller���� command�� ���� �غ� �� �� ���� ��ٸ� */
	if(wait_cmd_ready() < 0){
		printf("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* command �Է� */
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);

	/* read ������ flahs controller�� ����� �ԷµǾ� read queue�� �����Ͱ� ���� �� ���� ��ٸ�*/
	/* ���ÿ� ��ɿ� ���� �����Ͱ� �� ��° queue�� �ִ����� readQ_number ������ ������ */
	if(wait_flash_operation(op, tag, &readyQ_number, 0, 0) !=0){
		printf("Error: wait read data Q ready time out\r\n");
		return -1;
	}
	/* 8KB �뷮 (==1page) ��ŭ �о� �� �� ���� �ݺ�*/
	for(int i = 0; i<514; i++){
		/*read queue upper�� ����� ���� ������ ����*/
		pReadBuf_upper[i] = CTC_Readq_In_Upper(readyQ_number);
		//pReadBuf_upper[i] = Xil_In64(C2C_READ_DATA0_UPPER_ADDR + readyQ_number*READQ_ADDR_INTERVAL);
		
		/*read queue lower�� ����� ���� ������ ����*/
		pReadBuf_lower[i] = CTC_Readq_In_Lower(readyQ_number);
		//pReadBuf_lower[i] = Xil_In64(C2C_READ_DATA0_LOWER_ADDR + readyQ_number*READQ_ADDR_INTERVAL);
	}

	printf("read page operation was finished\r\n");
	return 0;
}

int write_page(u64 bus, u64 chip, u64 block, u64 page, /*cosnt*/ u64* pWriteBuf_upper, /*cosnt*/ u64* pWriteBuf_lower){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = WRITE;
	u64 command;
	u64 requested_tag;
	/* �̸� write & erase status register�� write data tag�� ������Ʈ �ϱ� ���� ��Ŷ�� ����� ����*/
	/* write data�� tag�� ����� tag�� ���, ���ÿ� tag valid bit�� 1�� ���� */
	u64 writeData_tag =  (VALID<<WR_DATA_TAG_VALID_BIT)|(tag<<WR_DATA_TAG_BIT);
	u64 status_reg_value;
	int ack;
	int ack_tag;
	/* command ����*/
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT)				\
			|(page << PAGE_BIT);
	/* flash controller���� command�� ���� �غ� �� �� ���� ��ٸ� */
	if(wait_cmd_ready() < 0){
		printf("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* ��� �Է�*/
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);
	
	/* write data request�� ���� �� ���� ��ٸ��� ������ tag�� ������*/
	if(wait_writeData_req(&requested_tag)!=0){
		printf("Error: write data request time out\r\n");
		return -1;
	}
	/* ������ tag�� ����� tag�� ��ġ���� ������ ����*/
	if(requested_tag != tag){
		printf("Error: the tag of the current command and the requested tag are different.\r\n");
		return -1;
	}
	printf("write data requested\r\n");
	/* flash controller���� write data�� ���� �غ� �� �� ���� ���*/
	if(wait_wrData_ready() < 0){
		printf("Error: wait write data ready signal time out\r\n");
		return -1;
	}
	/* write & erase status ���������� 64-bit�� �о��*/
	status_reg_value = CTC_In(rgstr_vptr.wne_stat);
	//status_reg_value = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
	
	/* �о�� write & erase status ���������� 64-bit���� write data tag�� �ռ� �̸� �������� ��Ŷ���� ������Ʈ �� */
	CTC_Out(rgstr_vptr.wne_stat, status_reg_value | writeData_tag);
	//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, status_reg_value | writeData_tag);
	
	/* 8KB�뷮(==1page) ��ŭ write data�� ���� �� ���� �ݺ�*/
	for(int j=0; j<514; j++){
		/* write upper buffer�� �ִ� 64-bit���� ����*/
		CTC_Out(rgstr_vptr.write_data_u, pWriteBuf_upper[i]);
		//CTC_Out(rgstr_vptr.write_data_u, *pWriteBuf_upper);
		//Xil_Out64(C2C_WRITE_DATA_UPPER_ADDR, *pWriteBuf_upper);
		
		/* write lower buffer�� �ִ� 64-bit���� ����*/
		CTC_Out(rgstr_vptr.write_data_l, *pWriteBuf_lower[i]);
		//CTC_Out(rgstr_vptr.write_data_l, *pWriteBuf_lower);
		//Xil_Out64(C2C_WRITE_DATA_LOWER_ADDR, *pWriteBuf_lower);
		
		//(*pWriteBuf_upper) += 1;//making write data for test
		//(*pWriteBuf_lower) += 1;//making write data for test
	}
	/* write �����͸� �� ������ ����*/
	/* write & erase status ���������� write tag ���� 0���� �ٲ�*/
	CTC_Out(rgstr_vptr.wne_stat, status_reg_value & (~writeData_tag));
	//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, status_reg_value & (~writeData_tag));
	
	/* ack ��ȣ�� ���� �� ���� ��ٸ��� ������ ack ���� �޾ƿ�*/
	if(wait_flash_operation(op, tag, 0, &ack, &ack_tag)!=0){
		printf("Error: wait ack time out\r\n");
		return -1;
	}
	/* ack ���� Ȯ������ �� write�� ���� ���� ���� �ʾҴٸ�*/
	if(ack != ACK_WRITE_DONE){
		printf("Error: ack is not ACK_WRITE_DONE\r\n");
		return -1;
	}
	printf("write page operation was finished\r\n");

	return 0;
}

int erase_block(u64 bus, u64 chip, u64 block){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = ERASE;
	u64 command;
	u64 ack;	//changed the type of ack, ack_tag from int to u64
	u64 ack_tag;
	/* command ����*/
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT);
	/* flash controller���� command�� ���� �غ� �� �� ���� ��ٸ� */
	if(wait_cmd_ready() < 0){
		printf("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* command �Է�*/
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);
	
	/* ack ��ȣ�� ���� �� ���� ��ٸ��� ������ ack ���� �޾ƿ�*/
	if(wait_flash_operation(op, tag, 0, &ack, &ack_tag)!=0){
		printf("Error: wait ack time out\r\n");
		return -1;
	}
	/* ack ���� Ȯ������ �� erase�� ���� ���� ���� �ʾҴٸ�*/
	if(ack != ACK_ERASE_DONE){
		printf("Error: ack is not ACK_ERASE_DONE\r\n");
		return -1;
	}
	printf("erase block operation was finished\r\n");

	return 0;
}


int wait_cmd_ready(void){
	int time_cnt = MAX_CMD_RDY_WAIT_CNT;
	/* command �������Ϳ��� 64-bit�� �о� �� �� command ready ���� ������ �� command ready ���� 0�̸� �ݺ�*/
	while((CTC_In(rgstr_vptr.cmd) & CMD_READY_MASK) == 0){
	//while((Xil_In64(C2C_CMD_ADDR) & CMD_READY_MASK) == 0){
		if(time_cnt == 0)
			return -1;
		time_cnt--;
	}
	return 0;
}

int wait_wrData_ready(void){
	int time_cnt = MAX_WR_DATA_READY_CNT;
	/* command �������Ϳ��� 64-bit�� �о� �� �� write data ready ���� ������ �� write data ready ���� 0�̸� �ݺ�*/
	while((CTC_In(rgstr_vptr.cmd) & WR_DATA_READY_MASK) == 0){
	//while((Xil_In64(C2C_CMD_ADDR) & WR_DATA_READY_MASK) == 0){
		if(time_cnt == 0)
			return -1;
		time_cnt--;
	}
	return 0;
}

int wait_writeData_req(u64* requested_tag){
	u64 wr_er_status;
	u64 wr_dataReq_ready;
	u64 wr_dataReq_tag;
	int time_cnt = MAX_WR_DATA_REQ_WAIT_CNT;

	while(time_cnt-- != 0){
		/* write & erase register�� ���� �����ִ� �� 64-bit �о��*/
		wr_er_status = CTC_In(rgstr_vptr.wne_stat);
		//wr_er_status = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
		
		/* write & erase register�� ���� �����ִ� �� 64-bit ���� write data request ready ��ȣ�� ������*/
		wr_dataReq_ready = wr_er_status & WR_DATA_REQ_READY_MASK;
		//wr_dataReq_ready = wr_er_status & WR_DATA_REQ_READY_MASK;
		
		/* write & erase register�� ���� �����ִ� �� 64-bit ���� write data request tag�� ������*/
		wr_dataReq_tag = (wr_er_status & WR_DATA_REQ_TAG_MASK)>>WR_DATA_REQ_TAG_BIT;
		
		/* if write data request ready�� 0�� �ƴϸ� if�� ���� break*/
		if(wr_dataReq_ready > 0){
			/* write & erase register�� write data request valid bit�� 1�� ����*/
			CTC_Out(rgstr_vptr.wne_stat, wr_er_status|WR_DATA_REQ_VALID);
			//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status|WR_DATA_REQ_VALID);
			
			/* write & erase register�� write data request valid bit�� 0���� ����*/
			CTC_Out(rgstr_vptr.wne_stat, wr_er_status&(~WR_DATA_REQ_VALID));
			//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status&(~WR_DATA_REQ_VALID));
			
			/* write data request�� ���� tag�� �ٱ� �Լ����� ����ϱ� ���� ������ ����*/
			*requested_tag = wr_dataReq_tag;
			break;
		}
	}
	if(time_cnt == 0)
		return -1;
	else
		return 0;
}

int wait_flash_operation(u64 op, u64 tag, int* Qnumber, u64* ack,u64* ack_tag){
	u64 readQ_ready;
	u64 readQ_tag;
	u64 wr_er_status;
	u64 ack_ready;
	int time_cnt;		//limit wait time

	if(op == READ){
		/*Ÿ�� ī��Ʈ ���� �̸� define �س��� read ������ �ִ� �ð� ������ ������Ʈ*/
		time_cnt = MAX_READ_STATUS_WAIT_CNT;
		/*loop�� �ݺ��� �� ���� Ÿ�� ī��Ʈ ���� ����*/
		/*Ÿ�� ī��Ʈ ��==0 �̸� �ݺ��� break*/
		while(time_cnt-- != 0){
			/* read queue ready ��ȣ�� ������ (ready7, ready6, ready5, ..., ready0) */
			readQ_ready = CTC_In(rgstr_vptr.read_stat) & READQ_READY_MASK;
			//readQ_ready = Xil_In64(C2C_READ_STATUS_ADDR) & READQ_READY_MASK;
			
			/* read queue�� ����� data�� �ش��ϴ� tag�� ������ */
			readQ_tag = CTC_In(rgstr_vptr.read_stat) & READQ_TAG_MASK;
			//readQ_tag = Xil_In64(C2C_READ_STATUS_ADDR) & READQ_TAG_MASK;

			if((readQ_ready&READ_DATA0_READY_VALUE)>0) // if read queue0�� ready ��ȣ�� 1�� ��
				if((readQ_tag & READ_DATA0_TAG_MASK) == (tag<<READ_DATA0_TAG_BIT)) { // and if queue0�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 0; // ��ɿ� ���� read data�� ����� ���� queue0
					break;
				}
			if((readQ_ready&READ_DATA1_READY_VALUE)>0)
				if((readQ_tag & READ_DATA1_TAG_MASK) == (tag<<READ_DATA1_TAG_BIT)) {// and if queue1�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 1;// ��ɿ� ���� read data�� ����� ���� queue1
					break;
				}
			if((readQ_ready&READ_DATA2_READY_VALUE)>0)
				if((readQ_tag & READ_DATA2_TAG_MASK) == (tag<<READ_DATA2_TAG_BIT)) {// and if queue2�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 2;// ��ɿ� ���� read data�� ����� ���� queue2
					break;
				}
			if((readQ_ready&READ_DATA3_READY_VALUE)>0)
				if((readQ_tag & READ_DATA3_TAG_MASK) == (tag<<READ_DATA3_TAG_BIT)) {// and if queue3�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 3;// ��ɿ� ���� read data�� ����� ���� queue3
					break;
				}
			if((readQ_ready&READ_DATA4_READY_VALUE)>0)
				if((readQ_tag & READ_DATA4_TAG_MASK) == (tag<<READ_DATA4_TAG_BIT)) {// and if queue4�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 4;// ��ɿ� ���� read data�� ����� ���� queue4
					break;
				}
			if((readQ_ready&READ_DATA5_READY_VALUE)>0)
				if((readQ_tag & READ_DATA5_TAG_MASK) == (tag<<READ_DATA5_TAG_BIT)) {// and if queue5�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 5;// ��ɿ� ���� read data�� ����� ���� queue5
					break;
				}
			if((readQ_ready&READ_DATA6_READY_VALUE)>0)
				if((readQ_tag & READ_DATA6_TAG_MASK) == (tag<<READ_DATA6_TAG_BIT)) {// and if queue6�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 6;// ��ɿ� ���� read data�� ����� ���� queue6
					break;
				}
			if((readQ_ready&READ_DATA7_READY_VALUE)>0)
				if((readQ_tag & READ_DATA7_TAG_MASK) == (tag<<READ_DATA7_TAG_BIT)) {// and if queue7�� �������ִ� �������� tag�� ���´� ����� tag�� ��ġ�ϸ�
					*Qnumber = 7;// ��ɿ� ���� read data�� ����� ���� queue7
					break;
				}
		}// end while
	}// end if

	else if((op == WRITE)||(op == ERASE)){
		if(op == WRITE)
			/*Ÿ�� ī��Ʈ ���� �̸� define �س��� write ������ �ִ� �ð� ������ ������Ʈ*/
			time_cnt = MAX_WRITE_STATUS_WAIT_CNT;
		else if(op == ERASE)
			/*Ÿ�� ī��Ʈ ���� �̸� define �س��� erase ������ �ִ� �ð� ������ ������Ʈ*/
			time_cnt = MAX_ERASE_STATUS_WAIT_CNT;
		/*loop�� �ݺ��� �� ���� Ÿ�� ī��Ʈ ���� ����*/
		/*Ÿ�� ī��Ʈ ��==0 �̸� �ݺ��� break*/
		while(time_cnt-- != 0){
			/* write & erase status ���������� 64-bit ��� �о�� */
			wr_er_status = CTC_In(rgstr_vptr.wne_stat);
			//wr_er_status = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
			/* write & erase status �������Ͱ����� ack ready ��ȣ�� ���� */
			ack_ready = wr_er_status & WR_ER_ACK_READY_MASK;

			if(ack_ready > 0) {	//ack ready�� high ��� if�� ���� -> while�� break
				CTC_Out(rgstr_vptr.wne_stat, wr_er_status|WR_ER_ACK_VALID);
				//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status|WR_ER_ACK_VALID); //write & erase status ���������� ack valid �ڸ��� 1�� ����
				CTC_Out(rgstr_vptr.wne_stat, wr_er_status&(~WR_ER_ACK_VALID));
				//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status&(~WR_ER_ACK_VALID)); // write & erase status ���������� ack valid �ڸ��� 0���� ����
				*ack = ((wr_er_status & ACK_CODE_MASK)>>ACK_BIT);  // ���� ���� ��� 2-bit �����ͼ� �����Ϳ� ����
				*ack_tag = ((wr_er_status & ACK_TAG_MASK)>>ACK_TAG_BIT); //ack�� ���Ե� tag�� �����ͼ� �����Ϳ� ����
				break;
			}
		}// end while
	}// end else if

	/* Ÿ�� ī��Ʈ ���� 0���� �ݺ����� ���Դٸ� error ����	 */
	if(time_cnt == 0)
		return -1;
	/* Ÿ�� ī��Ʈ ���� 0�� �ƴ� ������ �ݺ����� ���Դٸ� ���� ����	 */
	else
		return 0;
}
