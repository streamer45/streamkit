// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Generic resource cache for native plugins.
//!
//! Replaces the hand-rolled `LazyLock<Mutex<HashMap<K, Arc<V>>>>` pattern
//! used across all native plugins for caching expensive shared resources
//! (ML models, inference engines, etc.).
//!
//! # Relationship to `ResourceManager`
//!
//! [`crate::streamkit_core::ResourceManager`] provides server-side LRU
//! eviction and memory-budget accounting.  `ResourceCache` is intentionally
//! simpler: it lives inside the plugin `.so` and owns resources for the
//! lifetime of the process.  A future bridge between the two (e.g. having
//! `ResourceManager` call [`ResourceCache::clear`]) is planned but not yet
//! implemented.
//!
//! # Migration note
//!
//! The previous `ResourceSupport` trait required `Resource: Resource + 'static`
//! (with `size_bytes()` / `resource_type()`) and offered a `deinit_resource`
//! hook.  The new design drops both: cleanup is handled via `Drop` on the
//! cached value, and the `Resource` trait bound is replaced by `Send + Sync`
//! to remove the coupling to the server-side resource manager.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Statistics for a resource cache instance.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits (get_or_init returned existing value).
    pub hits: u64,
    /// Number of cache misses (get_or_init called init closure).
    pub misses: u64,
    /// Current number of cached entries.
    pub entries: usize,
}

/// A thread-safe, lazily-initialized cache for expensive shared resources.
///
/// Designed for blocking FFI contexts — uses `std::sync::Mutex` (not tokio).
/// The init closure runs **outside** the lock to avoid blocking other threads
/// during slow model loads.
///
/// # Example
///
/// ```ignore
/// use streamkit_plugin_sdk_native::resource_cache::ResourceCache;
/// use std::sync::Arc;
///
/// static ENGINE_CACHE: ResourceCache<String, MyEngine> = ResourceCache::new();
///
/// let engine: Arc<MyEngine> = ENGINE_CACHE.get_or_init(
///     "model-v2".to_string(),
///     |key| Ok(MyEngine::load(key)?),
/// )?;
/// ```
pub struct ResourceCache<K, V> {
    inner: LazyLock<Mutex<HashMap<K, Arc<V>>>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<K, V> Default for ResourceCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> ResourceCache<K, V> {
    /// Creates a new, empty cache.
    ///
    /// This is `const` so it can be used in `static` declarations.
    pub const fn new() -> Self {
        Self {
            inner: LazyLock::new(|| Mutex::new(HashMap::new())),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Returns the number of cached entries.
    ///
    /// Returns `0` if the internal mutex is poisoned (a thread panicked
    /// while holding the lock).  Callers that need to distinguish
    /// "empty" from "poisoned" should use [`stats`](Self::stats) or
    /// inspect `get_or_init` errors instead.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes all cached entries.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }

    /// Returns current cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: self.len(),
        }
    }
}

impl<K: Eq + Hash + Clone, V> ResourceCache<K, V> {
    /// Returns the cached value for `key`, or initializes it with `init`.
    ///
    /// The init closure runs **outside** the mutex lock so that slow
    /// resource loads (model files, GPU init) do not block other threads.
    /// If two threads race on the same key, both may call `init` but only
    /// one value is stored; the other is dropped.  Both threads are
    /// counted as **misses** because each paid the full init cost.
    ///
    /// # Errors
    ///
    /// Returns an error if the mutex is poisoned or if `init` fails.
    pub fn get_or_init<F>(&self, key: K, init: F) -> Result<Arc<V>, String>
    where
        F: FnOnce(&K) -> Result<V, String>,
    {
        // Fast path: check if key already exists.
        {
            let guard =
                self.inner.lock().map_err(|e| format!("Failed to lock resource cache: {e}"))?;
            if let Some(value) = guard.get(&key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Arc::clone(value));
            }
        }
        // Lock dropped — run init outside the lock.

        let value = init(&key)?;

        // Re-lock and insert if still missing (another thread may have won).
        let mut guard =
            self.inner.lock().map_err(|e| format!("Failed to lock resource cache: {e}"))?;

        // Another thread may have inserted while we were initializing.
        // Count as a miss regardless — this thread paid the full init cost.
        self.misses.fetch_add(1, Ordering::Relaxed);
        #[allow(clippy::option_if_let_else)]
        let arc = if let Some(existing) = guard.get(&key) {
            Arc::clone(existing)
        } else {
            let arc = Arc::new(value);
            guard.insert(key, Arc::clone(&arc));
            arc
        };
        drop(guard);

        Ok(arc)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::needless_collect)]
mod tests {
    use super::*;

    #[test]
    fn get_or_init_caches_value() {
        let cache: ResourceCache<String, String> = ResourceCache::new();
        let v1 = cache.get_or_init("key".into(), |_| Ok("value".into())).unwrap();
        let v2 = cache.get_or_init("key".into(), |_| panic!("should not be called")).unwrap();
        assert!(Arc::ptr_eq(&v1, &v2));
    }

    #[test]
    fn different_keys_different_values() {
        let cache: ResourceCache<String, i32> = ResourceCache::new();
        let a = cache.get_or_init("a".into(), |_| Ok(1)).unwrap();
        let b = cache.get_or_init("b".into(), |_| Ok(2)).unwrap();
        assert_eq!(*a, 1);
        assert_eq!(*b, 2);
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn init_error_not_cached() {
        let cache: ResourceCache<String, String> = ResourceCache::new();
        let err = cache.get_or_init("key".into(), |_| Err("fail".into()));
        assert!(err.is_err());
        // Should retry on next call
        let ok = cache.get_or_init("key".into(), |_| Ok("recovered".into())).unwrap();
        assert_eq!(&*ok, "recovered");
    }

    #[test]
    fn clear_removes_entries() {
        let cache: ResourceCache<String, String> = ResourceCache::new();
        cache.get_or_init("a".into(), |_| Ok("x".into())).unwrap();
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn stats_tracks_hits_and_misses() {
        let cache: ResourceCache<String, String> = ResourceCache::new();
        cache.get_or_init("a".into(), |_| Ok("x".into())).unwrap();
        cache.get_or_init("a".into(), |_| panic!("no")).unwrap();
        cache.get_or_init("b".into(), |_| Ok("y".into())).unwrap();
        let stats = cache.stats();
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.entries, 2);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Barrier;
        use std::thread;

        static CACHE: ResourceCache<u32, String> = ResourceCache::new();
        let barrier = Arc::new(Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    // All threads try to init the same key
                    CACHE.get_or_init(0, |_| Ok(format!("from-thread-{i}"))).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // All threads should get the same Arc
        for r in &results {
            assert!(Arc::ptr_eq(r, &results[0]));
        }
    }
}
