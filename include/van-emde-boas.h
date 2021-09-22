/**
 * @file van-emde-boas.h
 * @brief van-emde-boas tree's data structures and macros
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#ifndef VAN_EMDE_BOAS_H
#define VAN_EMDE_BOAS_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>

#define NIL (-1)

#define vEB_root_up(u) ((int)pow(2, ceil(log2(u) / 2.0)))
#define vEB_root_down(u) ((int)pow(2, floor(log2(u) / 2.0)))

#define vEB_high(u, x) ((int)floor(x / vEB_root_down(u)))
#define vEB_low(u, y) (x % vEB_root_down(u))
#define vEB_index(u, x, y) (x * vEB_root_down(u) + y)

/**
 * @brief swap a and b data
 * 
 */
#define vEB_swap(a, b)                                                         \
	do {                                                                   \
		unsigned char                                                  \
			temp[sizeof(a) == sizeof(b) ? (signed)sizeof(a) : -1]; \
		memcpy(temp, &b, sizeof(a));                                   \
		memcpy(&b, &a, sizeof(a));                                     \
		memcpy(&a, temp, sizeof(a));                                   \
	} while (0)

/**
 * @brief structure of the van-emde-boas
 * 
 */
struct vEB {
	int u; /**< size of current set(number of bits)*/
	int min; /**< minimum location written 1 (-1 means that Nil) */
	int max; /**< maximum location written 1 (-1 means that Nil) */
	struct vEB *summary; /**< contains summary */
	struct vEB *cluster[0]; /**< next cluster values */
};

struct vEB *vEB_init(const int u);
int vEB_tree_insert(struct vEB *V, int x);
int vEB_tree_member(struct vEB *V, const int x);
int vEB_tree_successor(struct vEB *V, const int x);
int vEB_tree_predecessor(struct vEB *V, const int x);
void vEB_tree_delete(struct vEB *V, int x);
void vEB_free(struct vEB *v);

/**
 * @brief represent the size of set to power of two.
 * 
 * @param u size of the current set
 * @return int power of 2 formed value for vEB
 */
static inline int vEB_get_valid_size(const int u)
{
	const int is_power_of_two = u && (!(u & (u - 1)));
	if (u <= 0) {
		return NIL;
	}

	if (is_power_of_two) {
		return u;
	}

	return ((int)pow(2, ceil(log2(u))));
}

/**
 * @brief if there doesn't have any data in vEB node, executes the `min = max = x`
 * 
 * @param V represent the specific vEB node
 * @param x represent item wants to insert
 */
static inline void vEB_empty_tree_insert(struct vEB *V, const int x)
{
	V->min = V->max = x;
}

/**
 * @brief returns the vEB node's minimum value
 * 
 * @param V represent the specific vEB node
 * @return int represent the node's minimum value
 */
static inline int vEB_tree_minimum(struct vEB *V)
{
	return V->min;
}

/**
 * @brief returns the vEB node's maximum value
 * 
 * @param V represent the specific vEB node
 * @return int represent the node's maximum value
 */
static inline int vEB_tree_maximum(struct vEB *V)
{
	return V->max;
}

#ifdef __cplusplus
}
#endif

#endif
