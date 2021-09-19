#include <assert.h>

#include "include/module.h"
#include "include/flash.h"

int main(void)
{
	struct flash_device *flash = NULL;
	assert(0 == module_init(PAGE_FTL_MODULE, &flash, FLASH_DEFAULT_FLAG));
	assert(0 == module_exit(flash));
	return 0;
}
