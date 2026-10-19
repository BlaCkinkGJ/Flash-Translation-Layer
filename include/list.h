/**
 * @file list.h
 * @brief the doubly-linked list
 * @author Gijun Oh
 * @version 0.1
 * @date 2026-05-02
 */
#ifndef LIST_H
#define LIST_H

// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct list_node {
    // cppcheck-suppress unusedStructMember
    void *data;
    // cppcheck-suppress unusedStructMember
    struct list_node *next;
    // cppcheck-suppress unusedStructMember
    struct list_node *prev;
} list_node_t;

/*
 * Guard the shared layout: `src/list.rs` declares the same struct as
 * `#[repr(C)]` and asserts these exact numbers in its
 * `node_layout_matches_c` test. Both sides pin the same numbers, so a
 * drift in either definition breaks a build instead of corrupting nodes
 * at runtime.
 */
#if defined(__cplusplus)
#define LIST_STATIC_ASSERT(cond, msg) static_assert(cond, msg)
#else
#define LIST_STATIC_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

LIST_STATIC_ASSERT(offsetof(list_node_t, data) == 0,
		   "list_node_t.data must be first");
LIST_STATIC_ASSERT(offsetof(list_node_t, next) == sizeof(void *),
		   "list_node_t.next must follow data");
LIST_STATIC_ASSERT(offsetof(list_node_t, prev) == 2 * sizeof(void *),
		   "list_node_t.prev must follow next");
LIST_STATIC_ASSERT(sizeof(list_node_t) == 3 * sizeof(void *),
		   "list_node_t must be exactly three pointers");

#undef LIST_STATIC_ASSERT

/*
 * Task 16a: `util/list.c` is gone, so the `list_*` prototypes went with it.
 * `src/list.rs` is the only implementation now, and these are the entry points
 * the callers use.
 */
list_node_t *ffi_list_prepend(list_node_t *list, void *data);
list_node_t *ffi_list_remove(list_node_t *list, void *data);
list_node_t *ffi_list_sort(list_node_t *list,
			   int (*cmp)(const void *, const void *));
void ffi_list_free(list_node_t *list);
list_node_t *ffi_list_last(list_node_t *list);
size_t ffi_list_length(list_node_t *list);

#ifdef __cplusplus
}
#endif

#endif
