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
 * `integration-test.rs:322`, `example/rw_example.rs:97`). The module pair
 * below is reached from Rust: `ffi_module_init` dispatches to the init through
 * the table in `src/module.rs`, which also installs the exit as
 * `flash->f_submodule_exit`.
 * `interface/module.c`, the C table that used to name them, is gone (Task 16).
 */
int ffi_page_ftl_module_init(struct flash_device *, uint64_t flags);
int ffi_page_ftl_module_exit(struct flash_device *);

#endif
