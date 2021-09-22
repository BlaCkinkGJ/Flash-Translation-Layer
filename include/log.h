#ifndef LOG_H
#define LOG_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdio.h>

#define STRINGIFY(x) #x
#define TOSTRING(x) STRINGIFY(x)

#ifdef DEBUG
#define pr_debug(fmt, ...)                                                     \
	fprintf(stdout,                                                        \
		"DEBUG:[" __FILE__ ":%s(" TOSTRING(__LINE__) ")] " fmt,        \
		__func__, ##__VA_ARGS__)
#else
#define pr_debug(...) (void)0
#endif

#define pr_info(fmt, ...)                                                      \
	fprintf(stdout, "INFO:[" __FILE__ ":%s(" TOSTRING(__LINE__) ")] " fmt, \
		__func__, ##__VA_ARGS__)

#define pr_warn(fmt, ...)                                                      \
	fprintf(stderr,                                                        \
		"WARNING:[" __FILE__ ":%s(" TOSTRING(__LINE__) ")] " fmt,      \
		__func__, ##__VA_ARGS__)

#define pr_err(fmt, ...)                                                       \
	fprintf(stderr,                                                        \
		"ERROR:[" __FILE__ ":%s(" TOSTRING(__LINE__) ")] " fmt,        \
		__func__, ##__VA_ARGS__)

#ifdef __cplusplus
}
#endif

#endif
