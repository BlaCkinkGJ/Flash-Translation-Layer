#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <stdint.h>
#include <stdio.h>
#include <stdbool.h>
#include <unistd.h>
#include <string.h>
#if defined(__linux__) || defined(__APPLE__)
#include <sys/random.h>
#endif
#ifdef __linux__
#include <sched.h>
#endif
#ifdef __cplusplus
#define HAVE_DECL_BASENAME (1)
#endif
#include <unistd.h>
#include <fcntl.h>
#include <ctype.h>
#include <pthread.h>
#include <time.h>
// cppcheck-suppress missingIncludeSystem
#include <assert.h>
#include <errno.h>

#include "module.h"
#include "device.h"
#include "crc32.h"
#include "list.h"

#include <stdlib.h>

#ifdef USE_LEGACY_RANDOM
#pragma message "Disable linux kernel supported random generator"
#ifdef __linux__
#include <linux/random.h>
#include <syscall.h>
#endif
#else
#pragma message "Enable linux kernel supported random generator"
#endif



#define USE_CRC
#define USE_PER_CORE
// CRC32_INIT moved to crc32.h
#define PAGE_SIZE (0x1 << 12)
#define SEC_TO_NS (1000000000L)
#define NS_PER_MS (1000000L)
#define DEVICE_PATH_SIZE (PAGE_SIZE)

enum {
	WRITE = 0,
	READ,
	RAND_WRITE,
	RAND_READ,
};

static const char *module_str[] = {
	"pgftl",
	NULL,
};

static const char *device_str[] = {
	"ramdisk", "bluedbm", "zone", "raspberry", NULL,
};

static const char *workload_str[] = {
	"write", "read", "randwrite", "randread", NULL,
};

static const int module_list[] = {
	PAGE_FTL_MODULE,
};

static const int device_list[] = {
	RAMDISK_MODULE,
	BLUEDBM_MODULE,
	ZONE_MODULE,
	RASPBERRY_MODULE,
};

struct benchmark_parameter {
	int module_idx;
	int device_idx;

	int nr_jobs;
	int workload_idx;

	size_t block_sz;
	size_t nr_blocks;

	char device_path[DEVICE_PATH_SIZE];

	struct flash_device *flash;
	pthread_t *threads;

	uint32_t *crc32_list;
	bool *crc32_is_match;

	off_t *offset_sequence;
	int thread_id_allocator;
	size_t *wp;
	size_t *total_time;
	list_node_t **timer_list;
	bool do_warm_up;
};

static void make_sequence(struct benchmark_parameter *);
static void shuffling(off_t *sequence, size_t nr_blocks);

static struct benchmark_parameter *init_parameters(int argc, char **argv);
static void free_parameters(struct benchmark_parameter *);
static void print_parameters(const struct benchmark_parameter *parm);

#ifdef USE_CRC
static void fill_buffer_random(char *buffer, size_t block_sz);
#endif
static void *alloc_buffer(size_t block_sz);
static void free_buffer(void *buffer);

static void *write_data(void *);
static void *read_data(void *);

static void report_result(struct benchmark_parameter *parm);

int main(int argc, char **argv)
{
	struct flash_device *flash;
	struct benchmark_parameter *parm;
	char *path = NULL;

	int module, device;

	size_t idx;
	size_t wp;
	void *(*pthread_func)(void *) = NULL;

	setvbuf(stdout, NULL, _IONBF, 0);
	parm = init_parameters(argc, argv);
	module = module_list[parm->module_idx];
	device = device_list[parm->device_idx];
	path = parm->device_path;

	assert(module_init(module, &flash, (uint64_t)device) == 0);
	assert(flash->f_op->open(flash, path, O_CREAT | O_RDWR) == 0);
	parm->flash = flash;

	/* running part */
	print_parameters(parm);
	if (parm->do_warm_up || parm->workload_idx == RAND_READ ||
	    parm->workload_idx == READ) {
		printf("fill data start!\n");
		write_data(parm);
		for (idx = 0; idx < (size_t)parm->nr_jobs; idx++) {
			if (!parm->timer_list[idx]) {
				continue;
			}
			list_free(parm->timer_list[idx]);
			parm->timer_list[idx] = NULL;
			parm->wp[idx] = 0;
		}
		printf("ready to read!\n");
	}

	if (parm->workload_idx == RAND_WRITE ||
	    parm->workload_idx == RAND_READ) {
		shuffling(parm->offset_sequence, parm->nr_blocks);
	}

	if (parm->workload_idx == RAND_WRITE || parm->workload_idx == WRITE) {
		pthread_func = write_data;
	}

	if (parm->workload_idx == RAND_READ || parm->workload_idx == READ) {
		pthread_func = read_data;
	}

	__atomic_store_n(&parm->thread_id_allocator, 0, __ATOMIC_SEQ_CST);
	for (idx = 0; idx < (size_t)parm->nr_jobs; idx++) {
		int thread_id;
		thread_id = pthread_create(&parm->threads[idx], NULL,
					   pthread_func, (void *)parm);
		assert(thread_id >= 0);
	}

	do {
		size_t total_time = 0;
		wp = INT32_MAX;
		for (idx = 0; idx < (size_t)parm->nr_jobs; idx++) {
			if (wp < parm->wp[idx]) {
				continue;
			}
			wp = parm->wp[idx];
			total_time = parm->total_time[idx];
		}
		printf("\rProcessing: %.2lf%% [%.2lf MiB/s]",
		       ((double)wp / (double)(parm->nr_blocks - 1)) * 100.0,
		       ((double)wp * (double)parm->block_sz) /
			       (((double)total_time / (NS_PER_MS * 1000L)) *
				(0x1 << 20)));
		sleep(1);
	} while (wp < parm->nr_blocks - 1);
	printf("\n");

	for (idx = 0; idx < (size_t)parm->nr_jobs; idx++) {
		size_t status;
		pthread_join(parm->threads[idx], (void **)&status);
		printf("finish thread %zu\n", idx);
	}

	report_result(parm);

	/* deallocate the crc32 list */
	assert(flash->f_op->close(flash) == 0);
	assert(module_exit(flash) == 0);
	print_parameters(parm);
	free_parameters(parm);

	return 0;
}

static void print_list(FILE *stream, const char **ptr)
{
	while (1) {
		fprintf(stream, "%s", *ptr);
		ptr++;
		if (*ptr == NULL) {
			break;
		} else {
			fprintf(stream, ", ");
		}
	}
}

static void help_message(struct benchmark_parameter *parm, char **argv)
{
	int nr_jobs = parm->nr_jobs;
	size_t nr_blocks = parm->nr_blocks;
	char *device_path = parm->device_path;

	fprintf(stderr,
		"%s -m <module name> -d <device name> -t <workload> -j <# of jobs> -b <block size(bytes)> -n <# of blocks> -p <device path> [-w]\n",
		argv[0]);
	fprintf(stderr, "\t- modules     [");
	print_list(stderr, module_str);
	fprintf(stderr, "]\n");
	fprintf(stderr, "\t- devices     [");
	print_list(stderr, device_str);
	fprintf(stderr, "]\n");
	fprintf(stderr, "\t- workloads   [");
	print_list(stderr, workload_str);
	fprintf(stderr, "]\n");
	fprintf(stderr, "\t- jobs        (default: %d)\n", nr_jobs);
	fprintf(stderr, "\t- block size  (default: %zu)\n", (size_t)PAGE_SIZE);
	fprintf(stderr, "\t- # of block  (default: %zu)\n", nr_blocks);
	fprintf(stderr, "\t- path        (default: %s)\n",
		strlen(device_path) > 0 ? device_path : NULL);
	fprintf(stderr, "\t- w           disable warm-up phase (default: enabled)\n");
}

static void processing_parameters_error(char ch)
{
	switch (ch) {
	case 'm':
	case 'd':
	case 't':
	case 'j':
	case 'n':
	case 'b':
	case 'p':
		fprintf(stderr, "option -%c requires an arguments\n", ch);
		break;
	default:
		if (isprint(ch)) {
			fprintf(stderr, "unknown option character '-%c'\n", ch);
		} else {
			fprintf(stderr, "unknown option character '0x%x'\n",
				ch);
		}
		break;
	}
}

static size_t get_size_from_character(const char *rvalue)
{
	size_t block_size_suffix = 1;
	while (*rvalue != '\0') {
		if (*rvalue == 'M' || *rvalue == 'm') {
			block_size_suffix = ((size_t)1 << 20);
			break;
		}
		if (*rvalue == 'K' || *rvalue == 'k') {
			block_size_suffix = ((size_t)1 << 10);
			break;
		}
		rvalue++;
	}
	return block_size_suffix;
}

static int get_index_from_list(const char **str_list)
{
	int idx = 0;
	while (str_list[idx] != NULL) {
		if (!strcmp(optarg, str_list[idx])) {
			return idx;
		}
		idx += 1;
	}
	return -1;
}
static void make_sequence(struct benchmark_parameter *parm)
{
	size_t idx;
	for (idx = 0; idx < parm->nr_blocks; idx++) {
		parm->offset_sequence[idx] = (off_t)(idx * parm->block_sz);
	}
}

#ifndef USE_LEGACY_RANDOM
static uint64_t xorshift64_state;

static uint64_t xorshift64(void)
{
	uint64_t x = xorshift64_state;
	x ^= x << 13;
	x ^= x >> 7;
	x ^= x << 17;
	return xorshift64_state = x;
}
#endif

static void shuffling(off_t *sequence, size_t nr_blocks)
{
	size_t idx;
#ifdef USE_LEGACY_RANDOM
	struct timespec tv;
	uint64_t temp_seed;
	clock_gettime(CLOCK_MONOTONIC, &tv);

	temp_seed = (uint64_t)tv.tv_sec * SEC_TO_NS + (uint64_t)tv.tv_nsec;
	srand((unsigned int)temp_seed);
#else
	uint64_t seed = 0;
	if (getentropy(&seed, sizeof(seed)) != 0) {
		perror("getentropy failed");
		exit(EXIT_FAILURE);
	}
	if (seed == 0) {
		seed = 1;
	}
	xorshift64_state = seed;
#endif

	for (idx = 0; idx < nr_blocks; idx++) {
		off_t temp;
		size_t swap_pos;
#ifdef USE_LEGACY_RANDOM
		swap_pos = (size_t)rand();
#else
		swap_pos = (size_t)xorshift64();
#endif
		swap_pos = swap_pos % nr_blocks;
		temp = sequence[idx];
		sequence[idx] = sequence[swap_pos];
		sequence[swap_pos] = temp;
	}
}

static struct benchmark_parameter *init_parameters(int argc, char **argv)
{
	struct benchmark_parameter *parm;

	int module_idx = PAGE_FTL_MODULE;
	int device_idx = RAMDISK_MODULE;

	int nr_jobs = 0;
	int workload_idx = WRITE;

	size_t block_sz = (size_t)PAGE_SIZE;
	size_t nr_blocks = (size_t)1;
	bool do_warm_up = true;

	char *device_path;

	int c = 0;
	int i;

	parm = (struct benchmark_parameter *)malloc(
		sizeof(struct benchmark_parameter));
	memset(parm, 0, sizeof(struct benchmark_parameter));

	device_path = parm->device_path;
	memset(device_path, 0, (size_t)(DEVICE_PATH_SIZE - 1));
	nr_jobs = (int)sysconf(_SC_NPROCESSORS_ONLN);

	while ((c = getopt(argc, argv, "m:d:t:j:b:n:p:hw")) != -1) {
		switch (c) {
		case 'm':
			module_idx = get_index_from_list(module_str);
			if (module_idx == -1) {
				fprintf(stderr,
					"error: unexpected argument detected (%s)\n",
					optarg);
				help_message(parm, argv);
				exit(1);
			}
			break;
		case 'd':
			device_idx = get_index_from_list(device_str);
			if (device_idx == -1) {
				fprintf(stderr,
					"error: unexpected argument detected (%s)\n",
					optarg);
				help_message(parm, argv);
				exit(1);
			}
			break;
		case 't':
			workload_idx = get_index_from_list(workload_str);
			if (workload_idx == -1) {
				fprintf(stderr,
					"error: unexpected argument detected (%s)\n",
					optarg);
				help_message(parm, argv);
				exit(1);
			}
			break;
		case 'j':
			nr_jobs = atoi(optarg);
			break;
		case 'b':
			block_sz = (size_t)atoi(optarg);
			block_sz = block_sz * get_size_from_character(optarg);
			break;
		case 'n':
			nr_blocks = (size_t)atoi(optarg);
			nr_blocks = nr_blocks * get_size_from_character(optarg);
			break;
		case 'p':
			strncpy(device_path, optarg, DEVICE_PATH_SIZE - 1);
			break;
		case 'w':
			do_warm_up = false;
			break;
		case 'h':
			help_message(parm, argv);
			exit(0);
			break;
		case '?':
			processing_parameters_error((char)optopt);
			help_message(parm, argv);
			exit(1);
			break;
		default:
			abort();
		}
	}
	parm->module_idx = module_idx;
	parm->device_idx = device_idx;

	parm->nr_jobs = nr_jobs;
	parm->workload_idx = workload_idx;
	parm->do_warm_up = do_warm_up;

	parm->block_sz = block_sz;
	parm->nr_blocks = nr_blocks;

	/* initialize the crc32 list */
	parm->crc32_list =
		(uint32_t *)malloc(parm->nr_blocks * sizeof(uint32_t));
	assert(parm->crc32_list != NULL);
	memset(parm->crc32_list, 0, parm->nr_blocks * sizeof(uint32_t));

	parm->crc32_is_match = (bool *)malloc(parm->nr_blocks * sizeof(bool));
	assert(parm->crc32_is_match != NULL);
	memset(parm->crc32_is_match, true, parm->nr_blocks * sizeof(bool));

	parm->offset_sequence =
		(off_t *)malloc(parm->nr_blocks * sizeof(size_t));
	assert(parm->offset_sequence != NULL);
	make_sequence(parm);

	parm->threads =
		(pthread_t *)malloc((size_t)parm->nr_jobs * sizeof(pthread_t));
	assert(parm->threads != NULL);
	memset(parm->threads, 0, (size_t)parm->nr_jobs * sizeof(pthread_t));

	parm->timer_list =
		(list_node_t **)malloc((size_t)parm->nr_jobs * sizeof(list_node_t *));
	assert(parm->timer_list != NULL);
	for (i = 0; i < parm->nr_jobs; i++) {
		parm->timer_list[i] = NULL;
	}

	__atomic_store_n(&parm->thread_id_allocator, 0, __ATOMIC_SEQ_CST);

	parm->wp = (size_t *)malloc((size_t)parm->nr_jobs * sizeof(size_t));
	assert(parm->wp != NULL);
	memset(parm->wp, 0, (size_t)parm->nr_jobs * sizeof(size_t));

	parm->total_time =
		(size_t *)malloc((size_t)parm->nr_jobs * sizeof(size_t));
	assert(parm->total_time != NULL);
	memset(parm->total_time, 0, (size_t)parm->nr_jobs * sizeof(size_t));

	return parm;
}

static void print_parameters(const struct benchmark_parameter *parm)
{
	const char *path;
	path = strlen(parm->device_path) > 0 ? parm->device_path : NULL;
	printf("[parameters]\n");
	printf("\t- modules     %s\n", module_str[parm->module_idx]);
	printf("\t- devices     %s\n", device_str[parm->device_idx]);
	printf("\t- workloads   %s\n", workload_str[parm->workload_idx]);
	printf("\t- jobs        %d\n", parm->nr_jobs);
	printf("\t- block size  %zu\n", parm->block_sz);
	printf("\t- # of block  %zu\n", parm->nr_blocks);
	printf("\t- io size     %zuMiB\n",
	       (parm->nr_blocks * parm->block_sz) >> 20);
	printf("\t- path        %s\n", path);
	printf("\t- warm-up     %s\n", parm->do_warm_up ? "enabled" : "disabled");
}

static void free_parameters(struct benchmark_parameter *parm)
{
	if (!parm) {
		return;
	}
	if (parm->crc32_list) {
		free(parm->crc32_list);
	}
	if (parm->crc32_is_match) {
		free(parm->crc32_is_match);
	}
	if (parm->offset_sequence) {
		free(parm->offset_sequence);
	}
	if (parm->threads) {
		free(parm->threads);
	}
	if (parm->wp) {
		free(parm->wp);
	}
	if (parm->total_time) {
		free(parm->total_time);
	}
	if (parm->timer_list) {
		int idx;
		for (idx = 0; idx < parm->nr_jobs; idx++) {
			if (!parm->timer_list[idx]) {
				continue;
			}
			list_free(parm->timer_list[idx]);
		}
		free(parm->timer_list);
	}
	free(parm);
}

#ifdef USE_CRC
static void fill_buffer_random(char *buffer, size_t block_sz)
{
#ifdef USE_LEGACY_RANDOM
	static __thread unsigned int seed = 0;
	if (seed == 0) {
		struct timespec tv;
		clock_gettime(CLOCK_MONOTONIC, &tv);
		seed = (unsigned int)((uintptr_t)tv.tv_nsec ^ (uintptr_t)pthread_self());
		if (seed == 0) {
			seed = 1;
		}
	}
	size_t pos = 0;
	while (pos < block_sz) {
		if (block_sz - pos >= sizeof(int)) {
			int r = rand_r(&seed);
			memcpy(&buffer[pos], &r, sizeof(int));
			pos += sizeof(int);
		} else {
			buffer[pos] = (char)rand_r(&seed);
			pos++;
		}
	}
#else
	size_t pos = 0;
	while (pos < block_sz) {
		char *ptr = &buffer[pos];
		size_t len = (block_sz - pos) > 256 ? 256 : (block_sz - pos);
		if (getentropy(ptr, len) != 0) {
			perror("getentropy failed");
			exit(EXIT_FAILURE);
		}
		pos += len;
	}
#endif
}
#endif

static void *alloc_buffer(size_t block_sz)
{
	char *buffer;
	buffer = (char *)malloc(block_sz);
	memset(buffer, 0, block_sz);
	return (void *)buffer;
}

static void free_buffer(void *buffer)
{
	free(buffer);
}

static void *write_data(void *data)
{
	struct timespec start, end;
	size_t interval;
	ssize_t ret;
	unsigned char *buffer;
	int thread_id;
	struct flash_device *flash;
	struct benchmark_parameter *parm;
#ifdef USE_PER_CORE
#ifndef __APPLE__
	cpu_set_t cpuset;
#endif
#endif

	parm = (struct benchmark_parameter *)data;
	flash = parm->flash;

	thread_id = __atomic_fetch_add(&parm->thread_id_allocator, 1, __ATOMIC_SEQ_CST);

#ifdef USE_PER_CORE
#ifdef __APPLE__
	ret = 0;
#else
	CPU_ZERO(&cpuset);
	CPU_SET((size_t)thread_id, &cpuset);
	ret = pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
#endif
	assert(ret == 0);
#endif

	buffer = (unsigned char *)alloc_buffer(parm->block_sz);
	assert(buffer != NULL);

	for (int i = 0; i < (int)parm->nr_blocks; i++) {
		off_t offset = parm->offset_sequence[i];
#ifdef USE_CRC
		fill_buffer_random((char *)buffer, parm->block_sz);
		parm->crc32_list[(size_t)offset / parm->block_sz] =
			crc32(buffer, parm->block_sz, CRC32_INIT);
#endif
		clock_gettime(CLOCK_MONOTONIC, &start);
		ret = flash->f_op->write(flash, buffer, parm->block_sz, offset);
		clock_gettime(CLOCK_MONOTONIC, &end);
		assert(ret == (ssize_t)parm->block_sz);
		interval = (size_t)((end.tv_sec - start.tv_sec) * SEC_TO_NS) +
			   (unsigned long)(end.tv_nsec - start.tv_nsec);
		parm->total_time[thread_id] += interval;
		parm->timer_list[thread_id] =
			list_prepend(parm->timer_list[thread_id],
				       (void *)(uintptr_t)(interval));
		parm->wp[thread_id] = (size_t)i;
	}
	free_buffer(buffer);
	return NULL;
}

static void *read_data(void *data)
{
	struct timespec start, end;
	size_t interval;
	ssize_t ret;
	unsigned char *buffer;
	int thread_id;
	struct flash_device *flash;
	struct benchmark_parameter *parm;
#ifdef USE_PER_CORE
#ifndef __APPLE__
	cpu_set_t cpuset;
#endif
#endif

	parm = (struct benchmark_parameter *)data;

	flash = parm->flash;
	buffer = (unsigned char *)alloc_buffer(parm->block_sz);
	assert(buffer != NULL);

	thread_id = __atomic_fetch_add(&parm->thread_id_allocator, 1, __ATOMIC_SEQ_CST);
#ifdef USE_PER_CORE
#ifdef __APPLE__
	ret = 0;
#else
	CPU_ZERO(&cpuset);
	CPU_SET((size_t)thread_id, &cpuset);
	ret = pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
#endif
	assert(ret == 0);
#endif
	for (int i = 0; i < (int)parm->nr_blocks; i++) {
		off_t offset = parm->offset_sequence[i];
#ifdef USE_CRC
		memset(buffer, 0, parm->block_sz);
#endif
		clock_gettime(CLOCK_MONOTONIC, &start);
		ret = flash->f_op->read(flash, buffer, parm->block_sz, offset);
		clock_gettime(CLOCK_MONOTONIC, &end);
		assert(ret == (ssize_t)parm->block_sz);
		interval = (size_t)((end.tv_sec - start.tv_sec) * SEC_TO_NS) +
			   (unsigned long)(end.tv_nsec - start.tv_nsec);
		parm->total_time[thread_id] += interval;
		parm->timer_list[thread_id] =
			list_prepend(parm->timer_list[thread_id],
				       (void *)(uintptr_t)(interval));
		parm->wp[thread_id] = (size_t)i;
#ifdef USE_CRC
		{
			uint32_t crc32_val =
				crc32(buffer, parm->block_sz, CRC32_INIT);
			if (crc32_val !=
			    parm->crc32_list[(size_t)offset / parm->block_sz]) {
				parm->crc32_is_match[(size_t)offset /
						     parm->block_sz] = false;
			}
		}
#endif
	}
	free_buffer(buffer);
	return NULL;
}

static void report_result(struct benchmark_parameter *parm)
{
	list_node_t *node;
	size_t max_latency, min_latency;
	size_t idx = 0;
	size_t write_size;
#ifdef USE_CRC
	bool is_valid;
#endif

	max_latency = 0;
	min_latency = INT32_MAX;

	write_size = parm->block_sz * parm->nr_blocks;
	printf("[job information]\n");
	printf("%-4s%-10s%-10s%-10s%-10s%-10s%-10s\n", "id", "time(s)",
	       "bw(MiB/s)", "iops", "avg(ms)", "max(ms)", "min(ms)");
	printf("=====\n");
	/* check interval */
	for (idx = 0; idx < (size_t)parm->nr_jobs; idx++) {
		size_t total_time = 0;
		size_t iops = 0;
		node = parm->timer_list[idx];
		while (node != NULL) {
			size_t interval;
			interval = (size_t)(uintptr_t)(node->data);
			total_time += interval;
			max_latency =
				max_latency > interval ? max_latency : interval;
			min_latency =
				min_latency < interval ? min_latency : interval;
			node = node->next;
			iops += 1;
		}
		printf("%-4zu%-10.4lf%-10.4lf%-10zu%-10.4lf%-10.4lf%-10.4lf\n",
		       idx, ((double)total_time / (NS_PER_MS * 1000L)),
		       (double)(write_size) /
			       (((double)total_time / (NS_PER_MS * 1000L)) *
				(0x1 << 20)),
		       iops, ((double)total_time / NS_PER_MS) / (double)iops,
		       (double)max_latency / NS_PER_MS,
		       (double)min_latency / NS_PER_MS);
	}

#ifdef USE_CRC
	printf("[crc status]\n");
	/* check blocks */
	is_valid = true;
	if (parm->workload_idx == RAND_READ || parm->workload_idx == READ) {
		for (idx = 0; idx < parm->nr_blocks; idx++) {
			if (!parm->crc32_is_match[idx]) {
				is_valid = false;
				break;
			}
		}
	}
	if (is_valid == false) {
		printf("crc check failed\n");
	} else {
		printf("crc check success\n");
	}
#endif
}
