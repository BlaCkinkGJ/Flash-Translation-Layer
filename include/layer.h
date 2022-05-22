#include <stdlib.h>
#include <string.h>

#define ftl_malloc(size) malloc(size)
#define ftl_free(ptr) free(ptr)
#define ftl_memset(s, c, n) memset(s, c, n)
#define ftl_memcpy(dest, src, n) memcpy(dest, src, n)
