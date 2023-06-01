#include "chip2chip_core.h"
#include "device.h"
#include "log.h"
#include "bits.h"

int fd_memory; //File descriptor of the memory device

struct _rgstr_vptr rgstr_vptr;

void vptr_mmap(u64** vptr, off_t addr) { //Mapping registers to the host's memory
	(*vptr = (u64 *)mmap(NULL, REG_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd_memory, addr));
}

void rgstr_offset_map(u64** vptr, u64 offset) {
	*vptr = rgstr_vptr.cmd + offset/sizeof(u64*);
}

int c2c_init(void) {	//opening memory device as a file descriptor to use them with mmap/msync
	
	if((fd_memory = open("/dev/mem", O_RDWR | O_SYNC)) != -1) { //"open" the memory device
		//assign vptr with mmap function
		
		vptr_mmap(&rgstr_vptr.cmd, C2C_CMD_ADDR);
		
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

int read_page(u64 bus, u64 chip, u64 block, u64 page, 
		u64* pReadBuf_upper, u64* pReadBuf_lower, size_t page_size){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = READ; // operation을 read로 설정
	u64 command;
	u64 readData_upper; // read queue에서 한번 dequeue된 값을 받아올 upper 변수
	u64 readData_lower;	// read queue에서 한번 dequeue된 값을 받아올 lower 변수
	int readyQ_number;
	int i = 0;
	/* command 생성 */
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT)				\
			|(page << PAGE_BIT);
	/* flash controller에서 command를 받을 준비가 될 때 까지 기다림 */
	if(wait_cmd_ready() < 0){
		pr_err("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* command 입력 */
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);

	/* read 동작이 flahs controller에 제대로 입력되어 read queue에 데이터가 쌓일 때 까지 기다림*/
	/* 동시에 명령에 대한 데이터가 몇 번째 queue에 있는지를 readQ_number 변수에 가져옴 */
	if(wait_flash_operation(op, tag, &readyQ_number, 0, 0) !=0){
		pr_err("Error: wait read data Q ready time out\r\n");
		return -1;
	}
	/* 8KB 용량 (==1page) 만큼 읽어 올 때 까지 반복*/
	for(size_t i; i < (page_size / (2 * sizeof(u64))); i++){
		/*read queue upper에 저장된 값을 변수에 저장*/
		pReadBuf_upper[i] = CTC_Readq_In_Upper(readyQ_number);
		//pReadBuf_upper[i] = Xil_In64(C2C_READ_DATA0_UPPER_ADDR + readyQ_number*READQ_ADDR_INTERVAL);
		
		/*read queue lower에 저장된 값을 변수에 저장*/
		pReadBuf_lower[i] = CTC_Readq_In_Lower(readyQ_number);
		//pReadBuf_lower[i] = Xil_In64(C2C_READ_DATA0_LOWER_ADDR + readyQ_number*READQ_ADDR_INTERVAL);
	}

	pr_info("read page operation was finished\r\n");
	return 0;
}

int write_page(u64 bus, u64 chip, u64 block, u64 page, 
		/*cosnt*/ u64* pWriteBuf_upper, /*cosnt*/ u64* pWriteBuf_lower, size_t page_size){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = WRITE;
	u64 command;
	u64 requested_tag;
	/* 미리 write & erase status register에 write data tag를 업데이트 하기 위한 패킷을 만들어 놓음*/
	/* write data의 tag로 명령의 tag를 사용, 동시에 tag valid bit을 1로 만듦 */
	u64 writeData_tag =  (VALID<<WR_DATA_TAG_VALID_BIT)|(tag<<WR_DATA_TAG_BIT);
	u64 status_reg_value;
	u64 ack;
	u64 ack_tag;
	/* command 생성*/
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT)				\
			|(page << PAGE_BIT);
	/* flash controller에서 command를 받을 준비가 될 때 까지 기다림 */
	if(wait_cmd_ready() < 0){
		pr_err("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* 명령 입력*/
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);
	
	/* write data request가 나올 때 까지 기다리고 나오면 tag를 가져옴*/
	if(wait_writeData_req(&requested_tag)!=0){
		pr_err("Error: write data request time out\r\n");
		return -1;
	}
	/* 가져온 tag가 명령의 tag와 일치하지 않으면 에러*/
	if(requested_tag != tag){
		pr_err("Error: the tag of the current command and the requested tag are different.\r\n");
		return -1;
	}
	pr_info("write data requested\r\n");
	/* flash controller에서 write data를 받을 준비가 될 때 까지 대기*/
	if(wait_wrData_ready() < 0){
		pr_err("Error: wait write data ready signal time out\r\n");
		return -1;
	}
	/* write & erase status 레지스터의 64-bit를 읽어옴*/
	status_reg_value = CTC_In(rgstr_vptr.wne_stat);
	//status_reg_value = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
	
	/* 읽어온 write & erase status 레지스터의 64-bit에서 write data tag만 앞서 미리 만들어놓은 패킷으로 업데이트 함 */
	CTC_Out(rgstr_vptr.wne_stat, status_reg_value | writeData_tag);
	//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, status_reg_value | writeData_tag);
	
	/* 8KB용량(==1page) 만큼 write data를 보낼 때 까지 반복*/
	for(size_t i; i < (page_size / (2 * sizeof(u64))); i++){
		/* write upper buffer에 있는 64-bit값을 보냄*/
		CTC_Out(rgstr_vptr.write_data_u, pWriteBuf_upper[i]);
		//CTC_Out(rgstr_vptr.write_data_u, *pWriteBuf_upper);
		//Xil_Out64(C2C_WRITE_DATA_UPPER_ADDR, *pWriteBuf_upper);
		
		/* write lower buffer에 있는 64-bit값을 보냄*/
		CTC_Out(rgstr_vptr.write_data_l, pWriteBuf_lower[i]);
		//CTC_Out(rgstr_vptr.write_data_l, *pWriteBuf_lower);
		//Xil_Out64(C2C_WRITE_DATA_LOWER_ADDR, *pWriteBuf_lower);
		
	}
	/* write 데이터를 다 보내고 나면*/
	/* write & erase status 레지스터의 write tag 값만 0으로 바꿈*/
	CTC_Out(rgstr_vptr.wne_stat, status_reg_value & (~writeData_tag));
	//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, status_reg_value & (~writeData_tag));
	
	/* ack 신호가 나올 때 까지 기다리고 나오면 ack 값을 받아옴*/
	if(wait_flash_operation(op, tag, 0, &ack, &ack_tag)!=0){
		pr_err("Error: wait ack time out\r\n");
		return -1;
	}
	/* ack 값을 확인했을 때 write가 정상 종료 되지 않았다면*/
	if(ack != ACK_WRITE_DONE){
		pr_err("Error: ack is not ACK_WRITE_DONE\r\n");
		return -1;
	}
	pr_info("write page operation was finished\r\n");

	return 0;
}

int erase_block(u64 bus, u64 chip, u64 block){
	u64 key = ACCESS_KEY;
	u64 tag = 0;
	u64 op = ERASE;
	u64 command;
	u64 ack;	//changed the type of ack, ack_tag from int to u64
	u64 ack_tag;
	/* command 생성*/
	command = (key << KEY_BIT)					\
			|(tag << TAG_BIT)					\
			|(op << OP_BIT)						\
			|(bus << BUS_BIT)					\
			|(chip << CHIP_BIT)					\
			|(block << BLOCK_BIT);
	/* flash controller에서 command를 받을 준비가 될 때 까지 기다림 */
	if(wait_cmd_ready() < 0){
		pr_err("Error: wait command ready signal time out\r\n");
		return -1;
	}
	/* command 입력*/
	CTC_Out(rgstr_vptr.cmd, command);
	//Xil_Out64(C2C_CMD_ADDR, command);
	
	/* ack 신호가 나올 때 까지 기다리고 나오면 ack 값을 받아옴*/
	if(wait_flash_operation(op, tag, 0, &ack, &ack_tag)!=0){
		pr_err("Error: wait ack time out\r\n");
		return -1;
	}
	/* ack 값을 확인했을 때 erase가 정상 종료 되지 않았다면*/
	if(ack != ACK_ERASE_DONE){
		pr_err("Error: ack is not ACK_ERASE_DONE\r\n");
		return -1;
	}
	//pr_info("erase block operation was finished\r\n");

	return 0;
}


int wait_cmd_ready(void){
	int time_cnt = MAX_CMD_RDY_WAIT_CNT;
	/* command 레지스터에서 64-bit를 읽어 온 뒤 command ready 값만 취했을 때 command ready 값이 0이면 반복*/
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
	/* command 레지스터에서 64-bit를 읽어 온 뒤 write data ready 값만 취했을 때 write data ready 값이 0이면 반복*/
	while((CTC_In(rgstr_vptr.cmd) & WR_DATA_READY_MASK) == 0){
	//while((Xil_In64(C2C_CMD_ADDR) & WR_DATA_READY_MASK) == 0){
		if(time_cnt == 0)
			return -1;
		time_cnt--;
	}
	return 0;
}

int wait_writeData_req(u64* requested_tag){
	volatile u64 wr_er_status;
	volatile u64 wr_dataReq_ready;
	volatile u64 wr_dataReq_tag;
	int time_cnt = MAX_WR_DATA_REQ_WAIT_CNT;

	while(time_cnt-- != 0){
		/* write & erase register에 현재 쓰여있는 값 64-bit 읽어옴*/
		wr_er_status = CTC_In(rgstr_vptr.wne_stat);
		//wr_er_status = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
		
		/* write & erase register에 현재 쓰여있는 값 64-bit 에서 write data request ready 신호만 가져옴*/
		wr_dataReq_ready = wr_er_status & WR_DATA_REQ_READY_MASK;
		//wr_dataReq_ready = wr_er_status & WR_DATA_REQ_READY_MASK;
		
		/* write & erase register에 현재 쓰여있는 값 64-bit 에서 write data request tag만 가져옴*/
		wr_dataReq_tag = (wr_er_status & WR_DATA_REQ_TAG_MASK)>>WR_DATA_REQ_TAG_BIT;
		
		/* if write data request ready가 0이 아니면 if문 들어가고 break*/
		if(wr_dataReq_ready > 0){
			/* write & erase register에 write data request valid bit만 1로 만듦*/
			CTC_Out(rgstr_vptr.wne_stat, wr_er_status|WR_DATA_REQ_VALID);
			//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status|WR_DATA_REQ_VALID);
			
			/* write & erase register에 write data request valid bit만 0으로 만듦*/
			CTC_Out(rgstr_vptr.wne_stat, wr_er_status&(~WR_DATA_REQ_VALID));
			//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status&(~WR_DATA_REQ_VALID));
			
			/* write data request로 나온 tag를 바깥 함수에서 사용하기 위해 변수에 저장*/
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
	volatile u64 readQ_ready;
	volatile u64 readQ_tag;
	volatile u64 wr_er_status;
	volatile u64 ack_ready;
	int time_cnt;		//limit wait time

	if(op == READ){
		/*타임 카운트 값을 미리 define 해놓은 read 동작의 최대 시간 값으로 업데이트*/
		time_cnt = MAX_READ_STATUS_WAIT_CNT;
		/*loop를 반복할 때 마다 타임 카운트 값을 감소*/
		/*타임 카운트 값==0 이면 반복문 break*/
		while(time_cnt-- != 0){
			/* read queue ready 신호를 가져옴 (ready7, ready6, ready5, ..., ready0) */
			readQ_ready = CTC_In(rgstr_vptr.read_stat) & READQ_READY_MASK;
			//readQ_ready = Xil_In64(C2C_READ_STATUS_ADDR) & READQ_READY_MASK;
			
			/* read queue에 저장된 data에 해당하는 tag를 가져옴 */
			readQ_tag = CTC_In(rgstr_vptr.read_stat) & READQ_TAG_MASK;
			//readQ_tag = Xil_In64(C2C_READ_STATUS_ADDR) & READQ_TAG_MASK;

			if((readQ_ready&READ_DATA0_READY_VALUE)>0) // if read queue0의 ready 신호가 1일 때
				if((readQ_tag & READ_DATA0_TAG_MASK) == (tag<<READ_DATA0_TAG_BIT)) { // and if queue0가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 0; // 명령에 대한 read data가 저장된 곳은 queue0
					break;
				}
			if((readQ_ready&READ_DATA1_READY_VALUE)>0)
				if((readQ_tag & READ_DATA1_TAG_MASK) == (tag<<READ_DATA1_TAG_BIT)) {// and if queue1가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 1;// 명령에 대한 read data가 저장된 곳은 queue1
					break;
				}
			if((readQ_ready&READ_DATA2_READY_VALUE)>0)
				if((readQ_tag & READ_DATA2_TAG_MASK) == (tag<<READ_DATA2_TAG_BIT)) {// and if queue2가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 2;// 명령에 대한 read data가 저장된 곳은 queue2
					break;
				}
			if((readQ_ready&READ_DATA3_READY_VALUE)>0)
				if((readQ_tag & READ_DATA3_TAG_MASK) == (tag<<READ_DATA3_TAG_BIT)) {// and if queue3가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 3;// 명령에 대한 read data가 저장된 곳은 queue3
					break;
				}
			if((readQ_ready&READ_DATA4_READY_VALUE)>0)
				if((readQ_tag & READ_DATA4_TAG_MASK) == (tag<<READ_DATA4_TAG_BIT)) {// and if queue4가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 4;// 명령에 대한 read data가 저장된 곳은 queue4
					break;
				}
			if((readQ_ready&READ_DATA5_READY_VALUE)>0)
				if((readQ_tag & READ_DATA5_TAG_MASK) == (tag<<READ_DATA5_TAG_BIT)) {// and if queue5가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 5;// 명령에 대한 read data가 저장된 곳은 queue5
					break;
				}
			if((readQ_ready&READ_DATA6_READY_VALUE)>0)
				if((readQ_tag & READ_DATA6_TAG_MASK) == (tag<<READ_DATA6_TAG_BIT)) {// and if queue6가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 6;// 명령에 대한 read data가 저장된 곳은 queue6
					break;
				}
			if((readQ_ready&READ_DATA7_READY_VALUE)>0)
				if((readQ_tag & READ_DATA7_TAG_MASK) == (tag<<READ_DATA7_TAG_BIT)) {// and if queue7가 가지고있는 데이터의 tag가 보냈던 명령의 tag와 일치하면
					*Qnumber = 7;// 명령에 대한 read data가 저장된 곳은 queue7
					break;
				}
		}// end while
	}// end if

	else if((op == WRITE)||(op == ERASE)){
		if(op == WRITE)
			/*타임 카운트 값을 미리 define 해놓은 write 동작의 최대 시간 값으로 업데이트*/
			time_cnt = MAX_WRITE_STATUS_WAIT_CNT;
		else if(op == ERASE)
			/*타임 카운트 값을 미리 define 해놓은 erase 동작의 최대 시간 값으로 업데이트*/
			time_cnt = MAX_ERASE_STATUS_WAIT_CNT;
		/*loop를 반복할 때 마다 타임 카운트 값을 감소*/
		/*타임 카운트 값==0 이면 반복문 break*/
		while(time_cnt-- != 0){
			/* write & erase status 레지스터의 64-bit 모두 읽어옴 */
			wr_er_status = CTC_In(rgstr_vptr.wne_stat);
			//wr_er_status = Xil_In64(C2C_WRITE_ERASE_STATUS_ADDR);
			/* write & erase status 레지스터값에서 ack ready 신호만 추출 */
			ack_ready = wr_er_status & WR_ER_ACK_READY_MASK;

			if(ack_ready > 0) {	//ack ready가 high 라면 if문 실행 -> while문 break
				CTC_Out(rgstr_vptr.wne_stat, wr_er_status|WR_ER_ACK_VALID);
				//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status|WR_ER_ACK_VALID); //write & erase status 레지스터의 ack valid 자리만 1로 만듦
				CTC_Out(rgstr_vptr.wne_stat, wr_er_status&(~WR_ER_ACK_VALID));
				//Xil_Out64(C2C_WRITE_ERASE_STATUS_ADDR, wr_er_status&(~WR_ER_ACK_VALID)); // write & erase status 레지스터의 ack valid 자리만 0으로 만듦
				*ack = ((wr_er_status & ACK_CODE_MASK)>>ACK_BIT);  // 동작 종료 결과 2-bit 가져와서 포인터에 저장
				*ack_tag = ((wr_er_status & ACK_TAG_MASK)>>ACK_TAG_BIT); //ack에 포함된 tag를 가져와서 포인터에 저장
				break;
			}
		}// end while
	}// end else if

	/* 타임 카운트 값이 0으로 반복문을 나왔다면 error 종료	 */
	if(time_cnt == 0)
		return -1;
	/* 타임 카운트 값이 0이 아닌 값으로 반복문을 나왔다면 정상 종료	 */
	else
		return 0;
}

