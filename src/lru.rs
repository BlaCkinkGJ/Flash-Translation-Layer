//! LRU cache ported from `include/lru.h` + `util/lru.c`.
//!
//! The C ABI is preserved: `lru_node` / `lru_cache` are `#[repr(C)]` mirrors
//! of the C structs, the `nil` sentinel is embedded in the cache exactly as
//! in C (so `head` points *inside* the cache allocation), and C reads
//! `cache->capacity` / `cache->size` directly (`lru_get_evict_size` inline in
//! the header, `test/lru-test.c`) — those offsets are the contract, pinned on
//! both sides by `layout_matches_c` here and the `LRU_STATIC_ASSERT`s in
//! `include/lru.h`.
//!
//! Symbols carry the `ffi_` prefix so they can coexist with `util/lru.c`
//! while both implementations are linked; the cutover is Task 14.
//!
//! Deliberate deviations from the C original:
//!
//! - `ffi_lru_init` / `ffi_lru_put` cannot fail with `NULL` / `-ENOMEM`:
//!   `Box` aborts on OOM, as in `src/list.rs`.
//! - Eviction stops (and logs) when the list is empty while `size` still says
//!   otherwise. C unlinks the sentinel there, hands it to the dealloc
//!   callback and `free()`s it — heap corruption. Reachable in both
//!   implementations by the same route: a failing dealloc callback returns
//!   early and skips the `size` decrement. See
//!   `eviction_on_desynced_size_does_not_free_the_sentinel`.
//! - `ffi_lru_free` frees the cache structure even when the dealloc callback
//!   fails (C returns early and leaks both the structure and the remaining
//!   nodes). The remaining nodes still leak, matching C.
//! - The issue's `LruCache<K, V>` generic is not provided: the C ABI pins a
//!   concrete `u64 -> uintptr_t` shape (raw `uintptr_t` values, sentinel node,
//!   C-visible fields) and no Rust consumer exists yet. Revisit alongside the
//!   `page_write.rs` / `page_gc.rs` ports (Tasks 10-11), as in `src/list.rs`.
//! - `get` is O(1) via a `HashMap` index keyed on the newest node, the issue's
//!   requested improvement over C's linear scan. It is equivalent: eviction
//!   takes the tail (oldest) first, so of two nodes sharing a key the newer
//!   one — the one the index points at, and the one the C scan would return —
//!   always outlives the older one.
//! - The eviction policy is C's, not a standard evict-one: a `put` into a full
//!   cache evicts `capacity` entries, i.e. drains it. Pinned by
//!   `eviction_drains_whole_cache_when_full`; the issue raised this as an open
//!   question and the port keeps C behaviour so the cutover stays invisible.

use crate::{pr_debug, pr_err};
use std::collections::HashMap;
use std::os::raw::c_int;
use std::ptr;

/// Callback invoked once per evicted / freed entry. Mirrors `lru_dealloc_fn`
/// in `include/lru.h` (renamed for Rust's `non_camel_case_types`; the ABI is
/// the same nullable function pointer). `0` means the entry was released
/// successfully.
pub type LruDeallocFn = Option<unsafe extern "C" fn(u64, usize) -> c_int>;

/// Mirrors `struct lru_node` in `include/lru.h`. Layout is the C contract;
/// see `layout_matches_c`.
#[repr(C)]
pub struct lru_node {
    pub key: u64,
    pub value: usize,
    pub next: *mut lru_node,
    pub prev: *mut lru_node,
}

/// Mirrors `struct lru_cache` in `include/lru.h`, plus a Rust-only index tail.
///
/// `index` sits after the C fields, so the C-visible prefix keeps its offsets
/// while `sizeof(struct lru_cache)` no longer matches. C must therefore not
/// size-allocate it — only `util/lru.c`'s `malloc` ever did, and that dies
/// with the C implementation at Task 14.
///
/// The embedded `nil` is self-referential: never move an `lru_cache` by value
/// after `ffi_lru_init`, or `head` / `nil.next` / `nil.prev` go stale.
#[repr(C)]
pub struct lru_cache {
    /// Total number of entries before a `put` triggers eviction.
    pub capacity: usize,
    /// Current number of entries; C reads this directly.
    pub size: usize,
    /// Sentinel node living in `nil`; `head->next` is MRU, `head->prev` LRU.
    pub head: *mut lru_node,
    /// Entry release callback, `None` when C passed `NULL`.
    pub deallocate: LruDeallocFn,
    /// Sentinel node, never read by C (`include/lru.h`: "don't access this").
    pub nil: lru_node,
    /// key -> newest node holding it. Rust-only, see the module docs.
    index: HashMap<u64, *mut lru_node>,
}

/// Insert `newnode` directly after `node`, as C's `lru_node_insert`.
///
/// # Safety
/// `node` and `newnode` are live nodes of the same cache; `node->next` is
/// non-`NULL` (only the embedded sentinel satisfies that unconditionally).
unsafe fn insert_after(node: *mut lru_node, newnode: *mut lru_node) {
    (*newnode).prev = node;
    (*newnode).next = (*node).next;
    (*(*node).next).prev = newnode;
    (*node).next = newnode;
}

/// Unlink `node` from its list, as C's `lru_delete_node` minus the sentinel
/// case (the caller refuses that input). Does not free `node`.
///
/// # Safety
/// `node` is a live, linked node that is not the sentinel.
unsafe fn unlink(node: *mut lru_node) {
    (*(*node).prev).next = (*node).next;
    (*(*node).next).prev = (*node).prev;
}

/// Evict the least recently used entry. Mirrors C's `__lru_do_evict`: unlink,
/// drop the index entry, run the callback, release the node — in that order,
/// and the node is released even when the callback reports failure.
///
/// # Safety
/// `cache` is live and its list is non-empty (`cache->head->next != head`).
unsafe fn evict_one(cache: &mut lru_cache) -> c_int {
    let target = (*cache.head).prev;
    unlink(target);

    let key = (*target).key;
    let value = (*target).value;
    if cache.index.get(&key) == Some(&target) {
        cache.index.remove(&key);
    }

    let mut ret = 0;
    if let Some(deallocate) = cache.deallocate {
        ret = deallocate(key, value);
    }
    drop(Box::from_raw(target));
    ret
}

/// Evict up to `nr_evict` entries. Mirrors C's `lru_do_evict`, including the
/// early return that leaves `size` too high when the callback fails.
///
/// # Safety
/// `cache` is live.
unsafe fn evict(cache: &mut lru_cache, nr_evict: usize) -> c_int {
    for _ in 0..nr_evict {
        let head = cache.head;
        if (*head).next == head {
            // C would target the sentinel here, call the dealloc callback with
            // it and free() it. Stop instead; the accounting stays as C left
            // it (see the module docs).
            pr_debug!("eviction skipped: list is empty (size: {})", cache.size);
            break;
        }
        let ret = evict_one(cache);
        if ret != 0 {
            return ret;
        }
        cache.size -= 1;
    }
    0
}

/// Initialize a cache holding at most `capacity` entries. Returns `NULL` for a
/// zero capacity, like C.
///
/// # Safety
/// `deallocate` is `NULL` or a callback valid for the whole cache lifetime.
#[no_mangle]
pub unsafe extern "C" fn ffi_lru_init(
    capacity: usize,
    deallocate: LruDeallocFn,
) -> *mut lru_cache {
    if capacity == 0 {
        pr_err!("capacity is zero");
        return ptr::null_mut();
    }

    let mut cache = Box::new(lru_cache {
        capacity,
        size: 0,
        head: ptr::null_mut(),
        deallocate,
        nil: lru_node {
            key: u64::MAX,
            value: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        },
        index: HashMap::new(),
    });

    // The Box allocation keeps `nil`'s address stable, so the sentinel can
    // point at itself exactly like C's `&cache->nil`.
    let nil = &mut cache.nil as *mut lru_node;
    cache.head = nil;
    cache.nil.next = nil;
    cache.nil.prev = nil;

    Box::into_raw(cache)
}

/// Insert `key` / `value`, evicting the whole cache first when it is full.
/// Returns `0`, like C (the `-ENOMEM` path cannot happen here).
///
/// # Safety
/// `cache` was returned by `ffi_lru_init` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn ffi_lru_put(
    cache: *mut lru_cache,
    key: u64,
    value: usize,
) -> c_int {
    let cache = &mut *cache;

    if cache.size >= cache.capacity {
        pr_debug!(
            "eviction is called (size: {}, cap: {})",
            cache.size,
            cache.capacity
        );
        // C ignores this return value; mirror that.
        let _ = evict(cache, cache.capacity);
    }

    let node = Box::into_raw(Box::new(lru_node {
        key,
        value,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));
    insert_after(cache.head, node);
    cache.size += 1;
    cache.index.insert(key, node);
    0
}

/// Return the value stored under `key` and promote it to most recently used,
/// or `0` when absent (C's `NULL`), like C.
///
/// # Safety
/// `cache` was returned by `ffi_lru_init` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn ffi_lru_get(cache: *mut lru_cache, key: u64) -> usize {
    let cache = &mut *cache;

    let node = match cache.index.get(&key) {
        Some(&node) => node,
        None => return 0,
    };

    unlink(node);
    insert_after(cache.head, node);
    (*node).value
}

/// Release every entry, then the cache itself. Returns the last dealloc
/// status (`0` on success, and for a `NULL` cache), like C.
///
/// # Safety
/// `cache` is `NULL` or was returned by `ffi_lru_init` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn ffi_lru_free(cache: *mut lru_cache) -> c_int {
    if cache.is_null() {
        return 0;
    }

    let mut ret = 0;
    {
        let cache_ref = &*cache;
        let head = cache_ref.head;
        let mut node = (*head).next;
        while node != head {
            let next = (*node).next;
            if let Some(deallocate) = cache_ref.deallocate {
                ret = deallocate((*node).key, (*node).value);
                if ret != 0 {
                    pr_err!(
                        "deallocate failed (key: {}, value: {})",
                        (*node).key,
                        (*node).value
                    );
                    // C returns here, leaking this node and the rest.
                    break;
                }
            }
            drop(Box::from_raw(node));
            node = next;
        }
    }
    drop(Box::from_raw(cache));
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};
    use std::os::raw::c_int;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// `-EINVAL` as `include/errno.h` (and `util/lru.c`) use it.
    const EINVAL: c_int = 22;

    /// Each recording callback owns its statics: `cargo test` runs tests in
    /// parallel, so sharing them across tests would race. Keys land in a
    /// fixed-size atomic slot array rather than a locked `Vec` to keep the
    /// callback allocation- and lock-free.
    const MAX_RECORDED: usize = 8;
    static DRAIN_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DRAIN_KEYS: [AtomicU64; MAX_RECORDED] =
        [const { AtomicU64::new(0) }; MAX_RECORDED];

    unsafe extern "C" fn draining_dealloc(key: u64, _value: usize) -> c_int {
        let n = DRAIN_CALLS.fetch_add(1, Ordering::SeqCst);
        assert!(n < MAX_RECORDED, "recording buffer overflow");
        DRAIN_KEYS[n].store(key, Ordering::SeqCst);
        0
    }

    fn drained_keys() -> Vec<u64> {
        let n = DRAIN_CALLS.load(Ordering::SeqCst);
        assert!(n <= MAX_RECORDED, "recording buffer overflow");
        (0..n).map(|i| DRAIN_KEYS[i].load(Ordering::SeqCst)).collect()
    }

    fn reset_drain() {
        DRAIN_CALLS.store(0, Ordering::SeqCst);
    }

    static BIGFILL_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BIGFILL_SENTINELS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn bigfill_dealloc(key: u64, _value: usize) -> c_int {
        BIGFILL_CALLS.fetch_add(1, Ordering::SeqCst);
        if key == u64::MAX {
            BIGFILL_SENTINELS.fetch_add(1, Ordering::SeqCst);
        }
        0
    }

    static FAILURE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FAILURE_SENTINELS: AtomicUsize = AtomicUsize::new(0);
    /// Number of leading calls that report failure.
    static FAILURE_FIRST: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn fail_twice_dealloc(key: u64, _value: usize) -> c_int {
        let n = FAILURE_CALLS.fetch_add(1, Ordering::SeqCst);
        if key == u64::MAX {
            FAILURE_SENTINELS.fetch_add(1, Ordering::SeqCst);
        }
        if n < FAILURE_FIRST.load(Ordering::SeqCst) {
            EINVAL
        } else {
            0
        }
    }

    #[test]
    fn layout_matches_c() {
        assert_eq!(offset_of!(lru_node, key), 0);
        assert_eq!(offset_of!(lru_node, value), size_of::<u64>());
        assert_eq!(
            offset_of!(lru_node, next),
            size_of::<u64>() + size_of::<usize>(),
            "lru_node.next offset"
        );
        assert_eq!(
            offset_of!(lru_node, prev),
            size_of::<u64>() + 2 * size_of::<usize>(),
            "lru_node.prev offset"
        );
        assert_eq!(
            size_of::<lru_node>(),
            size_of::<u64>() + 3 * size_of::<usize>(),
            "lru_node size"
        );

        // C-visible prefix of `lru_cache`; C never reads past `nil`, and the
        // Rust-only `index` tail is intentionally not asserted against C's
        // sizeof.
        assert_eq!(offset_of!(lru_cache, capacity), 0);
        assert_eq!(offset_of!(lru_cache, size), size_of::<usize>());
        assert_eq!(offset_of!(lru_cache, head), 2 * size_of::<usize>());
        assert_eq!(offset_of!(lru_cache, deallocate), 3 * size_of::<usize>());
        assert_eq!(offset_of!(lru_cache, nil), 4 * size_of::<usize>());
    }

    #[test]
    fn init_rejects_zero_capacity_and_free_null_is_noop() {
        unsafe {
            assert!(ffi_lru_init(0, None).is_null());
            assert_eq!(ffi_lru_free(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn put_get_roundtrip_and_get_missing() {
        unsafe {
            let cache = ffi_lru_init(10, None);
            assert!(!cache.is_null());
            assert_eq!((*cache).size, 0);
            assert_eq!(ffi_lru_get(cache, 1), 0, "empty cache must miss");

            for i in 1..=10u64 {
                assert_eq!(ffi_lru_put(cache, i, (i * 2) as usize), 0);
            }
            assert_eq!((*cache).size, 10, "C exposes size to callers");
            for i in 1..=10u64 {
                assert_eq!(ffi_lru_get(cache, i), (i * 2) as usize);
                assert_eq!((*cache).size, 10, "get must not resize");
            }

            // Full cache: each put evicts everything first.
            for i in 11..=20u64 {
                assert_eq!(ffi_lru_put(cache, i, (i * 2) as usize), 0);
            }
            assert_eq!(ffi_lru_get(cache, 5), 0, "old entry must be gone");
            assert_eq!(ffi_lru_get(cache, 20), 40);

            assert_eq!(ffi_lru_free(cache), 0);
        }
    }

    #[test]
    fn eviction_drains_whole_cache_when_full() {
        reset_drain();

        unsafe {
            let cache = ffi_lru_init(2, Some(draining_dealloc));
            assert_eq!(ffi_lru_put(cache, 1, 11), 0);
            assert_eq!(ffi_lru_put(cache, 2, 22), 0);
            assert_eq!((*cache).size, 2);

            // C evicts `capacity` entries, not one: both entries go, and the
            // callback sees them oldest first.
            assert_eq!(ffi_lru_put(cache, 3, 33), 0);
            assert_eq!((*cache).size, 1);
            assert_eq!(DRAIN_CALLS.load(Ordering::SeqCst), 2);
            assert_eq!(drained_keys(), vec![1, 2]);
            assert_eq!(ffi_lru_get(cache, 1), 0);
            assert_eq!(ffi_lru_get(cache, 2), 0);
            assert_eq!(ffi_lru_get(cache, 3), 33);

            assert_eq!(ffi_lru_free(cache), 0);
            assert_eq!(DRAIN_CALLS.load(Ordering::SeqCst), 3);
        }
    }

    #[test]
    fn big_fill_keeps_only_the_newest_capacity_entries() {
        const CAPACITY: usize = 1024;
        const TOTAL: usize = CAPACITY * 100;

        BIGFILL_CALLS.store(0, Ordering::SeqCst);
        BIGFILL_SENTINELS.store(0, Ordering::SeqCst);

        unsafe {
            let cache = ffi_lru_init(CAPACITY, Some(bigfill_dealloc));
            for i in 0..TOTAL as u64 {
                assert_eq!(ffi_lru_put(cache, i, i as usize), 0);
            }
            assert_eq!((*cache).size, CAPACITY);

            for i in 0..TOTAL as u64 {
                let value = ffi_lru_get(cache, i);
                if i < (TOTAL - CAPACITY) as u64 {
                    assert_eq!(value, 0, "key {i} should have been evicted");
                } else {
                    assert_eq!(value, i as usize, "key {i} should be resident");
                }
            }

            assert_eq!(BIGFILL_CALLS.load(Ordering::SeqCst), TOTAL - CAPACITY);
            assert_eq!(BIGFILL_SENTINELS.load(Ordering::SeqCst), 0);
            assert_eq!(ffi_lru_free(cache), 0);
        }
    }

    #[test]
    fn duplicate_keys_keep_c_behaviour() {
        // C inserts a second node for a repeated key; `get` returns the newest
        // one (its scan starts at the head, and the index points at the same
        // node), and eviction still takes the older node first. No dealloc
        // callback here, so this test shares no state with the recording one.
        unsafe {
            let cache = ffi_lru_init(3, None);
            assert_eq!(ffi_lru_put(cache, 7, 10), 0);
            assert_eq!(ffi_lru_put(cache, 7, 20), 0);
            assert_eq!((*cache).size, 2, "C counts both nodes");
            assert_eq!(ffi_lru_get(cache, 7), 20, "newest value wins");

            assert_eq!(ffi_lru_put(cache, 8, 80), 0);
            assert_eq!(ffi_lru_put(cache, 9, 90), 0, "drains the cache");

            assert_eq!(ffi_lru_get(cache, 7), 0, "both duplicates evicted");
            assert_eq!(ffi_lru_get(cache, 8), 0);
            assert_eq!(ffi_lru_get(cache, 9), 90);
            assert_eq!((*cache).size, 1);

            assert_eq!(ffi_lru_free(cache), 0);
        }
    }

    #[test]
    fn eviction_on_desynced_size_does_not_free_the_sentinel() {
        // Two failing callbacks leave `size` ahead of the list, exactly as C's
        // early return does. The next full `put` then runs out of real nodes
        // mid-eviction — C's `__lru_do_evict` would unlink the sentinel, call
        // the callback with it and free() it. The port must stop instead.
        FAILURE_CALLS.store(0, Ordering::SeqCst);
        FAILURE_SENTINELS.store(0, Ordering::SeqCst);
        FAILURE_FIRST.store(2, Ordering::SeqCst);

        unsafe {
            let cache = ffi_lru_init(2, Some(fail_twice_dealloc));
            for i in 1..=6u64 {
                assert_eq!(ffi_lru_put(cache, i, i as usize), 0, "put {i}");
            }
            assert_eq!(
                FAILURE_SENTINELS.load(Ordering::SeqCst),
                0,
                "the sentinel must never reach the dealloc callback"
            );
            assert_eq!((*cache).size, 3, "size accounting follows C");
            assert_eq!(ffi_lru_get(cache, 6), 6);
            assert_eq!(ffi_lru_get(cache, 5), 0);

            assert_eq!(ffi_lru_free(cache), 0);
        }
    }
}
