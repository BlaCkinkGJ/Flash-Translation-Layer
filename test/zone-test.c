#include "include/device.h"
#include "include/zone.h"
#include "unity.h"

#define ZBD_FILE_NAME "/dev/nvme0n2"

// #define WRITE_PAGE_SIZE(x) (device_get_total_pages(x))
#define WRITE_PAGE_SIZE(x) (device_get_pages_per_segment(x) * 5)

struct device *dev;

void setUp(void)
{
	int ret;
	ret = device_module_init(ZONE_MODULE, &dev, 0);
	TEST_ASSERT_EQUAL_INT(0, ret);
}

void tearDown(void)
{
	TEST_ASSERT_NOT_NULL(dev);
	device_module_exit(dev);
}

void test_open(void)
{
	int ret;
	ret = dev->d_op->open(dev, ZBD_FILE_NAME);
	TEST_ASSERT_EQUAL_INT(0, ret);
	ret = dev->d_op->close(dev);
	TEST_ASSERT_EQUAL_INT(0, ret);
}

void test_full_write(void)
{
	struct device_request request;
	struct device_address addr;
	char *buffer;
	size_t page_size;
	size_t total_pages;

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->open(dev, ZBD_FILE_NAME));
	page_size = device_get_page_size(dev);
	total_pages = WRITE_PAGE_SIZE(dev);

	buffer = (char *)malloc(page_size);
	TEST_ASSERT_NOT_NULL(buffer);
	memset(buffer, 0, page_size);

	/**< note that all I/O functions run synchronously */
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->write(dev, &request));
	}

	memset(buffer, 0, page_size);
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_READ;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->read(dev, &request));
		TEST_ASSERT_EQUAL_UINT32(addr.lpn, *(uint32_t *)request.data);
	}

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->close(dev));
	free(buffer);
}

void test_overwrite(void)
{
	struct device_request request;
	struct device_address addr;
	char *buffer;
	size_t page_size;
	size_t total_pages;

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->open(dev, ZBD_FILE_NAME));
	page_size = device_get_page_size(dev);
	total_pages = WRITE_PAGE_SIZE(dev);
	buffer = (char *)malloc(page_size);
	TEST_ASSERT_NOT_NULL(buffer);
	memset(buffer, 0, page_size);

	/**< note that all I/O functions run synchronously */
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->write(dev, &request));
	}

	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(-EINVAL, dev->d_op->write(dev, &request));
	}

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->close(dev));
	free(buffer);
}

void test_erase(void)
{
	struct device_request request;
	struct device_address addr;
	char *buffer;
	size_t page_size;
	size_t total_pages;
	size_t nr_segments;
	size_t nr_pages_per_segment;
	size_t segnum;

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->open(dev, ZBD_FILE_NAME));
	page_size = device_get_page_size(dev);
	total_pages = WRITE_PAGE_SIZE(dev);
	buffer = (char *)malloc(page_size);
	TEST_ASSERT_NOT_NULL(buffer);
	memset(buffer, 0, page_size);

	/**< note that all I/O functions run synchronously */
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->write(dev, &request));
	}

	nr_segments = WRITE_PAGE_SIZE(dev) / device_get_pages_per_segment(dev);
	for (segnum = 0; segnum < nr_segments - 1; segnum++) {
		addr.lpn = 0;
		addr.format.block = segnum;
		request.paddr = addr;
		request.flag = DEVICE_ERASE;
		request.end_rq = NULL;
		TEST_ASSERT_EQUAL_INT(0, dev->d_op->erase(dev, &request));
	};

	nr_pages_per_segment = device_get_pages_per_segment(dev);
	for (addr.lpn = 0; addr.lpn < total_pages - nr_pages_per_segment;
	     addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->write(dev, &request));
	}

	for (; addr.lpn < total_pages; addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = NULL;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(-EINVAL, dev->d_op->write(dev, &request));
	}

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->close(dev));
	free(buffer);
}

static void end_rq(struct device_request *request)
{
	struct device_address paddr = request->paddr;
	uint8_t *is_check = (uint8_t *)request->rq_private;
	is_check[paddr.lpn] = 1;
}

void test_end_rq_works(void)
{
	struct device_request request;
	struct device_address addr;
	char *buffer;
	uint8_t *is_check;
	size_t page_size;
	size_t total_pages;
	size_t segnum;
	size_t nr_segments;

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->open(dev, ZBD_FILE_NAME));
	page_size = device_get_page_size(dev);
	total_pages = WRITE_PAGE_SIZE(dev);
	nr_segments = WRITE_PAGE_SIZE(dev) / device_get_pages_per_segment(dev);

	buffer = (char *)malloc(page_size);
	TEST_ASSERT_NOT_NULL(buffer);
	memset(buffer, 0, page_size);

	is_check = (uint8_t *)malloc(total_pages);
	TEST_ASSERT_NOT_NULL(is_check);
	memset(is_check, 0, total_pages);

	/**< note that all I/O functions run synchronously */
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		memcpy(buffer, &addr.lpn, sizeof(uint32_t));
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = end_rq;
		request.flag = DEVICE_WRITE;
		request.sector = 0;
		request.data = buffer;
		request.rq_private = (void *)is_check;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->write(dev, &request));
	}

	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		TEST_ASSERT_EQUAL_INT(1, is_check[addr.lpn]);
	}

	memset(is_check, 0, total_pages);
	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		request.paddr = addr;
		request.data_len = page_size;
		request.end_rq = end_rq;
		request.flag = DEVICE_READ;
		request.sector = 0;
		request.data = buffer;
		TEST_ASSERT_EQUAL_INT(request.data_len,
				      dev->d_op->read(dev, &request));
		TEST_ASSERT_EQUAL_UINT32(addr.lpn, *(uint32_t *)request.data);
	}

	for (addr.lpn = 0; addr.lpn < total_pages; addr.lpn++) {
		TEST_ASSERT_EQUAL_INT(1, is_check[addr.lpn]);
	}

	memset(is_check, 0, total_pages);
	for (segnum = 0; segnum < nr_segments; segnum++) {
		addr.lpn = 0;
		addr.format.block = segnum;
		request.paddr = addr;
		request.flag = DEVICE_ERASE;
		request.end_rq = end_rq;
		TEST_ASSERT_EQUAL_INT(0, dev->d_op->erase(dev, &request));
	};

	for (segnum = 0; segnum < nr_segments; segnum++) {
		addr.lpn = 0;
		addr.format.block = segnum;
		TEST_ASSERT_EQUAL_INT(1, is_check[addr.lpn]);
	}

	TEST_ASSERT_EQUAL_INT(0, dev->d_op->close(dev));
	free(buffer);
	free(is_check);
}

int main(void)
{
	UNITY_BEGIN();
	RUN_TEST(test_open);
	RUN_TEST(test_full_write);
	RUN_TEST(test_overwrite);
	RUN_TEST(test_erase);
	RUN_TEST(test_end_rq_works);
	return UNITY_END();
}
