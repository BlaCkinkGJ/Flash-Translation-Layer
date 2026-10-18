/**
 * @file page.h
 * @brief the page FTL's C-visible surface
 * @author Gijun Oh
 * @version 0.3
 * @date 2021-09-22
 */
#ifndef PAGE_H
#define PAGE_H

#include <stdint.h>

#include "flash.h"

/**
 * @brief the page FTL's extra ioctl commands, issued through `flash->f_op`
 */
enum {
	PAGE_FTL_IOCTL_TRIM = 0,
};

/*
 * Task 16b (#86): the page FTL is Rust owned. `struct page_ftl` and
 * `struct page_ftl_segment` are plain Rust types in `src/ftl/mod.rs`, no C
 * reads them by offset, and the layout assertions that pinned the mirror are
 * gone with the structs. `ffi_page_ftl_module_init` allocates the FTL and hands
 * it back through `flash->f_private`, so no C caller needs its size or a
 * create form of its own.
 *
 * What is left is what C still calls: the ioctl command above (`f_op->ioctl`,
 * `integration-test.c:162`, `example/rw_example.c:32`) and the module pair
 * (`interface/module.c:29` names the init, and `interface/module.c:73-74` calls
 * the exit through the installed `flash->f_submodule_exit`).
 */
int ffi_page_ftl_module_init(struct flash_device *, uint64_t flags);
int ffi_page_ftl_module_exit(struct flash_device *);

#endif
