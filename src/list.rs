//! Doubly-linked list ported from `include/list.h` + `util/list.c`.
//!
//! The C ABI is preserved verbatim: `list_node_t` is `#[repr(C)]` with the
//! same three pointers, nodes are heap-allocated by `ffi_list_prepend`, and
//! released only by `ffi_list_remove` / `ffi_list_free`. C callers
//! (`ftl/page/page-write.c`, `ftl/page/page-gc.c`) walk `->next` and read
//! `->data` directly, so the node layout is part of the contract.
//!
//! Symbols carry the `ffi_` prefix so they can coexist with `util/list.c`
//! while both implementations are linked; the cutover is Task 14.
//!
//! Deliberate deviations from the C original:
//!
//! - `ffi_list_prepend` cannot return the original list on allocation
//!   failure: `Box` aborts on OOM instead of returning `NULL`.
//! - `ffi_list_sort` with a `NULL` comparator returns the list unchanged
//!   instead of dereferencing `NULL`.
//! - `ffi_list_sort` re-stitches a scratch `Vec<*mut list_node_t>` with
//!   `sort_by` (stable, like C's merge sort, so equal elements keep their
//!   order). ponytail: O(n) transient memory on the GC-time sort only —
//!   swap for an in-place merge if the sort ever lands on the write path.

use std::os::raw::{c_int, c_void};

/// Mirrors `list_node_t` in `include/list.h`. Field order and types are the
/// C contract; see `node_layout_matches_c`.
#[repr(C)]
pub struct list_node_t {
    /// Opaque payload owned by the caller; never read or freed here.
    pub data: *mut c_void,
    /// Next node towards the tail, `NULL` at the tail.
    pub next: *mut list_node_t,
    /// Previous node towards the head, `NULL` at the head.
    pub prev: *mut list_node_t,
}

/// Prepend a node holding `data`. Returns the new head.
///
/// # Safety
/// `list` is either `NULL` or a head previously returned by this module and
/// not yet freed. `data` is stored as-is and never dereferenced or freed.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_prepend(
    list: *mut list_node_t,
    data: *mut c_void,
) -> *mut list_node_t {
    let node = Box::into_raw(Box::new(list_node_t {
        data,
        next: list,
        prev: std::ptr::null_mut(),
    }));
    if !list.is_null() {
        (*list).prev = node;
    }
    node
}

/// Remove the first node whose `data` pointer equals `data` (pointer
/// equality, not payload equality — the FTL stores LPNs as fake pointers).
/// Returns the head, unchanged if no node matched.
///
/// # Safety
/// `list` is `NULL` or a live list from this module. Every node must have
/// been allocated by `ffi_list_prepend`.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_remove(
    mut list: *mut list_node_t,
    data: *mut c_void,
) -> *mut list_node_t {
    let mut curr = list;
    while !curr.is_null() {
        if (*curr).data == data {
            let prev = (*curr).prev;
            let next = (*curr).next;
            if !prev.is_null() {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
            if std::ptr::eq(curr, list) {
                list = next;
            }
            drop(Box::from_raw(curr));
            break;
        }
        curr = (*curr).next;
    }
    list
}

/// Free every node. Payloads are not freed.
///
/// # Safety
/// `list` is `NULL` or a live list from this module, passed exactly once.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_free(mut list: *mut list_node_t) {
    while !list.is_null() {
        let next = (*list).next;
        drop(Box::from_raw(list));
        list = next;
    }
}

/// Tail node, or `NULL` for an empty list.
///
/// # Safety
/// `list` is `NULL` or a live list from this module.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_last(list: *mut list_node_t) -> *mut list_node_t {
    let mut curr = list;
    while !curr.is_null() && !(*curr).next.is_null() {
        curr = (*curr).next;
    }
    curr
}

/// Number of nodes.
///
/// # Safety
/// `list` is `NULL` or a live list from this module.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_length(list: *mut list_node_t) -> usize {
    let mut len = 0usize;
    let mut curr = list;
    while !curr.is_null() {
        len += 1;
        curr = (*curr).next;
    }
    len
}

/// Stable merge sort of the list, ordering nodes by `cmp` over their `data`.
/// Returns the new head.
///
/// # Safety
/// `list` is `NULL` or a live list from this module. `cmp`, when present,
/// must be a valid comparator that can read both payloads.
#[no_mangle]
pub unsafe extern "C" fn ffi_list_sort(
    list: *mut list_node_t,
    cmp: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> c_int>,
) -> *mut list_node_t {
    let cmp = match cmp {
        Some(cmp) => cmp,
        None => return list,
    };
    if list.is_null() || (*list).next.is_null() {
        return list;
    }

    let mut nodes: Vec<*mut list_node_t> = Vec::new();
    let mut curr = list;
    while !curr.is_null() {
        nodes.push(curr);
        curr = (*curr).next;
    }

    nodes.sort_by(|a, b| cmp((**a).data, (**b).data).cmp(&0));

    let last = nodes.len() - 1;
    for (i, node) in nodes.iter().enumerate() {
        (**node).prev = if i == 0 { std::ptr::null_mut() } else { nodes[i - 1] };
        (**node).next = if i == last { std::ptr::null_mut() } else { nodes[i + 1] };
    }

    nodes[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    unsafe extern "C" fn int_cmp(a: *const c_void, b: *const c_void) -> c_int {
        *(a as *const i32) - *(b as *const i32)
    }

    /// Prepends `values` in reverse so the head is `values[0]`, matching the
    /// `make_int_list` helper in `test/list-test.c`.
    unsafe fn make_int_list(values: &mut [i32]) -> *mut list_node_t {
        let mut list = std::ptr::null_mut();
        for v in values.iter_mut().rev() {
            list = ffi_list_prepend(list, v as *mut i32 as *mut c_void);
        }
        list
    }

    unsafe fn assert_sorted_and_linked(mut list: *mut list_node_t) {
        let mut prev: *mut list_node_t = std::ptr::null_mut();
        let mut last: *const i32 = std::ptr::null();
        while !list.is_null() {
            assert_eq!((*list).prev, prev, "backward link broken");
            let val = (*list).data as *const i32;
            if !last.is_null() {
                assert!(*last <= *val, "not non-decreasing");
            }
            last = val;
            prev = list;
            list = (*list).next;
        }
    }

    #[test]
    fn node_layout_matches_c() {
        let ptr = size_of::<*mut c_void>();
        assert_eq!(offset_of!(list_node_t, data), 0);
        assert_eq!(offset_of!(list_node_t, next), ptr);
        assert_eq!(offset_of!(list_node_t, prev), 2 * ptr);
        assert_eq!(size_of::<list_node_t>(), 3 * ptr);
    }

    #[test]
    fn empty_list_has_no_length_or_tail() {
        unsafe {
            let list: *mut list_node_t = std::ptr::null_mut();
            assert_eq!(ffi_list_length(list), 0);
            assert!(ffi_list_last(list).is_null());
            assert!(ffi_list_sort(list, Some(int_cmp)).is_null());
            ffi_list_free(list);
        }
    }

    #[test]
    fn sort_single_node_keeps_payload() {
        let mut v = 42i32;
        unsafe {
            let list = ffi_list_prepend(std::ptr::null_mut(), &mut v as *mut i32 as *mut c_void);
            let list = ffi_list_sort(list, Some(int_cmp));
            assert_eq!((*list).data as *const i32, &v as *const i32);
            assert!((*list).next.is_null());
            assert!((*list).prev.is_null());
            ffi_list_free(list);
        }
    }

    #[test]
    fn sort_with_null_comparator_is_a_noop() {
        let mut values = [2, 1];
        unsafe {
            let list = make_int_list(&mut values);
            let sorted = ffi_list_sort(list, None);
            assert_eq!(sorted, list);
            let payload = (*sorted).data as *const i32;
            assert_eq!(*payload, 2);
            ffi_list_free(list);
        }
    }

    #[test]
    fn sort_already_sorted_reverse_and_duplicates() {
        let mut sorted = [1, 2, 3, 4, 5];
        let mut reversed = [5, 4, 3, 2, 1];
        let mut duplicates = [3, 1, 2, 3, 2, 1];
        unsafe {
            for values in [&mut sorted[..], &mut reversed[..], &mut duplicates[..]] {
                let list = make_int_list(values);
                let list = ffi_list_sort(list, Some(int_cmp));
                assert_sorted_and_linked(list);
                ffi_list_free(list);
            }
        }
    }

    #[test]
    fn sort_is_stable() {
        #[repr(C)]
        struct Pair {
            key: i32,
            id: i32,
        }

        unsafe extern "C" fn key_cmp(a: *const c_void, b: *const c_void) -> c_int {
            (*(a as *const Pair)).key - (*(b as *const Pair)).key
        }

        let mut pairs = [
            Pair { key: 1, id: 1 },
            Pair { key: 1, id: 2 },
            Pair { key: 0, id: 3 },
        ];
        unsafe {
            let mut list = std::ptr::null_mut();
            for p in pairs.iter_mut().rev() {
                list = ffi_list_prepend(list, p as *mut Pair as *mut c_void);
            }
            let list = ffi_list_sort(list, Some(key_cmp));
            let ids: Vec<i32> = {
                let mut ids = Vec::new();
                let mut curr = list;
                while !curr.is_null() {
                    let pair = (*curr).data as *const Pair;
                    ids.push((*pair).id);
                    curr = (*curr).next;
                }
                ids
            };
            assert_eq!(ids, vec![3, 1, 2], "equal keys must keep insertion order");
            ffi_list_free(list);
        }
    }

    #[test]
    fn prepend_length_and_last() {
        let mut values = [10, 20, 30];
        unsafe {
            let list = make_int_list(&mut values);
            assert_eq!(ffi_list_length(list), 3);
            assert_eq!((*list).data as *const i32, &values[0] as *const i32);
            assert_eq!((*ffi_list_last(list)).data as *const i32, &values[2] as *const i32);
            assert!((*list).prev.is_null());
            assert!((*(*list).next).prev == list, "backward link broken");
            ffi_list_free(list);
        }
    }

    #[test]
    fn remove_head_middle_tail_and_missing() {
        let mut values = [10, 20, 30];
        unsafe {
            let list = make_int_list(&mut values);

            // Missing payload leaves the list untouched.
            let mut other = 99i32;
            let list = ffi_list_remove(list, &mut other as *mut i32 as *mut c_void);
            assert_eq!(ffi_list_length(list), 3);

            // Middle node: neighbours must be relinked both ways.
            let middle = (*list).next;
            let list = ffi_list_remove(list, (*middle).data);
            assert_eq!(ffi_list_length(list), 2);
            assert_eq!((*(*list).next).data as *const i32, &values[2] as *const i32);
            assert!((*(*list).next).prev == list);

            // Tail node.
            let list = ffi_list_remove(list, &mut values[2] as *mut i32 as *mut c_void);
            assert_eq!(ffi_list_length(list), 1);
            assert!((*list).next.is_null());

            // Last node empties the list.
            let list = ffi_list_remove(list, &mut values[0] as *mut i32 as *mut c_void);
            assert!(list.is_null());
            ffi_list_free(list);
        }
    }

    #[test]
    fn remove_matches_first_equal_payload_only() {
        let mut v = 7i32;
        unsafe {
            let payload = &mut v as *mut i32 as *mut c_void;
            let list = ffi_list_prepend(std::ptr::null_mut(), payload);
            let list = ffi_list_prepend(list, payload);
            let list = ffi_list_remove(list, payload);
            assert_eq!(ffi_list_length(list), 1);
            ffi_list_free(list);
        }
    }
}
