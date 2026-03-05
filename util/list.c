#include "list.h"
// cppcheck-suppress missingIncludeSystem
#include <stdlib.h>
// cppcheck-suppress missingIncludeSystem
#include <assert.h>
#include "log.h"

list_node_t *list_prepend(list_node_t *list, void *data)
{
	list_node_t *node = (list_node_t *)malloc(sizeof(list_node_t));
	if (!node) {
		pr_err("list_prepend: memory allocation failed\n");
		return list;
	}
	node->data = data;
	node->next = list;
	node->prev = NULL;
	if (list)
		list->prev = node;
	return node;
}

list_node_t *list_remove(list_node_t *list, void *data)
{
	list_node_t *curr = list;
	while (curr) {
		if (curr->data == data) {
			if (curr->prev)
				curr->prev->next = curr->next;
			if (curr->next)
				curr->next->prev = curr->prev;
			if (curr == list)
				list = curr->next;
			free(curr);
			break;
		}
		curr = curr->next;
	}
	return list;
}

void list_free(list_node_t *list)
{
	while (list) {
		list_node_t *next = list->next;
		free(list);
		list = next;
	}
}

list_node_t *list_last(list_node_t *list)
{
	if (!list)
		return NULL;
	while (list->next)
		list = list->next;
	return list;
}

size_t list_length(list_node_t *list)
{
	size_t len = 0;
	while (list) {
		len++;
		list = list->next;
	}
	return len;
}

/* Merge sort implementation */
static list_node_t *merge(list_node_t *first, list_node_t *second,
			  int (*cmp)(const void *, const void *))
{
	if (!first)
		return second;
	if (!second)
		return first;

	list_node_t *head = NULL;
	list_node_t *tail = NULL;

	/* Merge nodes from both lists one by one */
	while (first && second) {
		list_node_t *node;
		if (cmp(first->data, second->data) <= 0) {
			node = first;
			first = first->next;
		} else {
			node = second;
			second = second->next;
		}
		/* Append selected node to the merged list */
		node->prev = tail;
		if (tail)
			tail->next = node;
		else
			head = node;
		tail = node;
	}

	/* Append any remaining nodes from either list in O(1) */
	/* Note: internal prev/next pointers within 'remaining' are already correct */
	{
		list_node_t *remaining = first ? first : second;
		if (remaining) {
			remaining->prev = tail;
			if (tail)
				tail->next = remaining;
			else
				head = remaining;
		}
	}

	/* Ensure head prev pointer is NULL (tail next is already NULL if end) */
	if (head)
		head->prev = NULL;

	return head;
}

static list_node_t *split(list_node_t *head)
{
	if (!head || !head->next)
		return NULL;
	list_node_t *fast = head, *slow = head;
	while (fast->next && fast->next->next) {
		fast = fast->next->next;
		slow = slow->next;
	}
	list_node_t *temp = slow->next;
	slow->next = NULL;
	if (temp)
		temp->prev = NULL;
	return temp;
}

list_node_t *list_sort(list_node_t *list,
		       int (*cmp)(const void *, const void *))
{
	if (!list || !list->next)
		return list;
	list_node_t *second = split(list);

	list = list_sort(list, cmp);
	second = list_sort(second, cmp);

	return merge(list, second, cmp);
}
