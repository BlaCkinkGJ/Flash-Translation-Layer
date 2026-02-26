#ifndef LIST_H
#define LIST_H

// cppcheck-suppress missingIncludeSystem
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct list_node {
    void *data;
    struct list_node *next;
    struct list_node *prev;
} list_node_t;

list_node_t *list_prepend(list_node_t *list, void *data);
list_node_t *list_remove(list_node_t *list, void *data);
list_node_t *list_sort(list_node_t *list, int (*cmp)(const void *, const void *));
void list_free(list_node_t *list);
list_node_t *list_last(list_node_t *list);
size_t list_length(list_node_t *list);

#ifdef __cplusplus
}
#endif

#endif
