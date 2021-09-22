/**
 * @file van-emde-boas.c
 * @brief implementation of the van-emde-boas tree
 * @author Gijun Oh
 * @version 1.0
 * @date 2021-09-22
 */
#include "include/log.h"
#include "include/van-emde-boas.h"

/**
 * @brief implementation to initialize the van-emde-boas tree
 * 
 * @param u size of the set in the van-emde-boas tree (number of bits)
 * @return struct vEB* returns pointer initialized the van-emde-boas tree
 */
static struct vEB *__vEB_init(int u)
{
	struct vEB *v = NULL;
	const int nr_cluster = vEB_root_up(u);
	const int cluster_size = sizeof(struct vEB *) * nr_cluster;
	int i;

	v = (struct vEB *)malloc(sizeof(struct vEB) + cluster_size);
	if (v == NULL) {
		pr_err("allocation failed\n");
		goto exit;
	}

	v->u = u;
	v->min = v->max = NIL;

	if (v->u > 2) {
		v->summary = __vEB_init(vEB_root_up(u));
		for (i = 0; i < nr_cluster; i++) {
			v->cluster[i] = __vEB_init(vEB_root_down(u));
		}
	} else {
		v->summary = NULL;
		for (i = 0; i < nr_cluster; i++) {
			v->cluster[i] = NULL;
		}
	}

exit:
	return v;
}

/**
 * @brief initialize the van-emde-boas tree
 * 
 * @param u size of the set in the van-emde-boas tree (number of bits)
 * @return struct vEB* returns pointer intialized the van-emde-boas tree
 */
struct vEB *vEB_init(const int u)
{
	int pow_of_2_u = vEB_get_valid_size(u);
	if (pow_of_2_u == NIL) {
		pr_err("invalid set size : %d\n", u);
		return NULL;
	}
	return __vEB_init(pow_of_2_u);
}

/**
 * @brief vEB insert the specific `x` value
 * 
 * @param V pointer of the van-emde-boase tree
 * @param x the value which wants to insert
 * @return 0 to success, negative value to fail
 */
int vEB_tree_insert(struct vEB *V, int x)
{
	int u = V->u;
	if (x >= u || x < 0) {
		pr_err("invalid value detected %d\n", x);
		return -1;
	}
	if (V->min == NIL) {
		vEB_empty_tree_insert(V, x);
	} else {
		if (x < V->min) {
			vEB_swap(x, V->min);
		} // end of x < V->min
		if (V->u > 2) {
			if (vEB_tree_minimum(V->cluster[vEB_high(u, x)]) ==
			    NIL) {
				vEB_tree_insert(V->summary, vEB_high(u, x));
				vEB_empty_tree_insert(
					V->cluster[vEB_high(u, x)],
					vEB_low(u, x));
			} else {
				vEB_tree_insert(V->cluster[vEB_high(u, x)],
						vEB_low(u, x));
			} // end of vEB_tree_minimum
		} // end of V->u > 2
		if (x > V->max) {
			V->max = x;
		} // end of x > V->max
	} // end of x > V->max
	return 0;
} // end of V->min == NIL

/**
 * @brief check the existence in the x position
 * 
 * @param V pointer of the van-emde-boas
 * @param x value of the position which exists
 * @return true if data find
 * @return 0 if data isn't found, 1 if data is found, -1 if fail
 */
int vEB_tree_member(struct vEB *V, const int x)
{
	if (x >= V->u || x < 0) {
		pr_err("invalid input detected => %d\n", x);
		return NIL;
	}
	if (x == V->min || x == V->max) {
		return 1;
	} else if (V->u == 2) {
		return 0;
	} // end of x == V->min || x == V->max

	return vEB_tree_member(V->cluster[vEB_high(V->u, x)], vEB_low(V->u, x));
}

/**
 * @brief find the successor item
 * 
 * @param V pointer of the van-emde-boas tree
 * @param x find successor starting location
 * @return int location of the successor (NIL means cannot find)
 */
int vEB_tree_successor(struct vEB *V, const int x)
{
	if (x >= V->u || x < 0) {
		pr_err("invalid input detected => %d\n", x);
		return NIL;
	}
	if (V->u == 2) {
		return ((x == 0 && V->max == 1) ? 1 : NIL);
	} else if (V->min != NIL && x < V->min) {
		return V->min;
	} else {
		const int max_low =
			vEB_tree_maximum(V->cluster[vEB_high(V->u, x)]);
		if (max_low != NIL && vEB_low(V->u, x) < max_low) {
			const int offset = vEB_tree_successor(
				V->cluster[vEB_high(V->u, x)],
				vEB_low(V->u, x));
			return vEB_index(V->u, vEB_high(V->u, x), offset);
		} else {
			const int succ_cluster = vEB_tree_successor(
				V->summary, vEB_high(V->u, x));
			if (succ_cluster == NIL) {
				return NIL;
			} else {
				const int offset = vEB_tree_minimum(
					V->cluster[succ_cluster]);
				return vEB_index(V->u, succ_cluster, offset);
			} // end of succ_cluster == NIL
		} // end of max_low != NIL && ~~
	} // end of V->u == 2
}

/**
 * @brief find the predecessor item
 * 
 * @param V pointer of the van-emde-boas tree
 * @param x find predecessor starting location
 * @return int location of the predecessor (NIL means cannot find)
 */
int vEB_tree_predecessor(struct vEB *V, const int x)
{
	if (x >= V->u || x < 0) {
		pr_err("invalid input detected => %d\n", x);
		return NIL;
	}
	if (V->u == 2) {
		return ((x == 1 && V->min == 0) ? 0 : NIL);
	} else if (V->max != NIL && x > V->max) {
		return V->max;
	} else {
		const int min_low =
			vEB_tree_minimum(V->cluster[vEB_high(V->u, x)]);
		if (min_low != NIL && vEB_low(V->u, x) > min_low) {
			const int offset = vEB_tree_predecessor(
				V->cluster[vEB_high(V->u, x)],
				vEB_low(V->u, x));
			return vEB_index(V->u, vEB_high(V->u, x), offset);
		} else {
			const int pred_cluster = vEB_tree_predecessor(
				V->summary, vEB_high(V->u, x));
			if (pred_cluster == NIL) {
				if (V->min != NIL && x > V->min) {
					return V->min;
				} else {
					return NIL;
				}
			} else {
				const int offset = vEB_tree_maximum(
					V->cluster[pred_cluster]);
				return vEB_index(V->u, pred_cluster, offset);
			} // end of pred_cluster == NIL
		} // end of min_low != NIL && ~~
	} // end of V->u == 2
}

/**
 * @brief delete the value in the position x
 *
 * @param V pointer of the van-emde-boas tree
 * @param x position of the deletion
 */
void vEB_tree_delete(struct vEB *V, int x)
{
	if (V->min == V->max) {
		V->max = V->min = NIL;
	} else if (V->u == 2) {
		V->max = V->min = (x == 0 ? 1 : 0);
	} else {
		if (x == V->min) {
			int first_cluster = vEB_tree_minimum(V->summary);
			x = vEB_index(
				V->u, first_cluster,
				vEB_tree_minimum(V->cluster[first_cluster]));
			V->min = x;
		} // end of x == V->min
		vEB_tree_delete(V->cluster[vEB_high(V->u, x)],
				vEB_low(V->u, x));

		if (vEB_tree_minimum(V->cluster[vEB_high(V->u, x)]) == NIL) {
			vEB_tree_delete(V->summary, vEB_high(V->u, x));
			if (x == V->max) {
				const int summary_max =
					vEB_tree_maximum(V->summary);
				if (summary_max == NIL) {
					V->max = V->min;
				} else {
					V->max = vEB_index(
						V->u, summary_max,
						vEB_tree_maximum(
							V->cluster[summary_max]));
				} // end fo summary_max == NIL
			} // end of x == V->max
		} else if (x == V->max) {
			V->max = vEB_index(
				V->u, vEB_high(V->u, x),
				vEB_tree_maximum(
					V->cluster[vEB_high(V->u, x)]));
		} // end of vEB_tree_minimum
	} // end of V->min == V->max
}

/**
 * @brief deallocate the van-emde-boas tree
 * 
 * @param V pointer of the van-emde-boas tree
 */
void vEB_free(struct vEB *V)
{
	const int nr_cluster = vEB_root_up(V->u);
	int i;

	if (V->summary == NULL) {
		goto dealloc;
	}
	vEB_free(V->summary);
	V->summary = NULL;

	for (i = 0; i < nr_cluster; i++) {
		if (V->cluster[i] != NULL) {
			vEB_free(V->cluster[i]);
			V->cluster[i] = NULL;
		}
	}

dealloc:
	free(V);
}
