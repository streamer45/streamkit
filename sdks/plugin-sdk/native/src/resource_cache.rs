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
//! lifetime of the process — there is currently no bridge between the two.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Errors returned by [`ResourceCache`] operations.
#[derive(Debug)]
pub enum CacheError {
    /// The internal mutex is poisoned (a thread panicked while holding it).
    Poisoned,
    /// The user-supplied init closure failed.
    Init(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Poisoned => write!(f, "resource cache mutex poisoned"),
            Self::Init(msg) => write!(f, "resource init failed: {msg}"),
        }
    }
}

impl std::error::Error for CacheError {}

/// Statistics for a resource cache instance.
///
/// # Counter semantics
///
/// * **`hits`** — `get_or_init` found the key in the map and returned
///   immediately without calling the init closure.
/// * **`misses`** — `get_or_init` did **not** find the key on the fast
///   path, called the init closure, and inserted the result (no race).
/// * **`init_races`** — the init closure ran, but another thread had
///   already inserted the same key by the time re-lock occurred.  The
///   losing value is dropped.  This counter lets dashboards spot
///   redundant resource loads under contention.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits (returned existing value without calling init).
    pub hits: u64,
    /// Number of cache misses (called init and inserted the result).
    pub misses: u64,
    /// Number of init races (called init but another thread won insertion).
    pub init_races: u64,
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
/// let engine: Arc<MyEngine> = ENGINE_CACHE
///     .get_or_init("model-v2".to_string(), |key| {
///         MyEngine::load(key).map_err(|e| e.to_string())
///     })?;
/// ```
pub struct ResourceCache<K, V> {
    inner: LazyLock<Mutex<HashMap<K, Arc<V>>>>,
    hits: AtomicU64,
    misses: AtomicU64,
    init_races: AtomicU64,
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
            init_races: AtomicU64::new(0),
        }
    }

    /// Returns the number of cached entries.
    ///
    /// Returns `0` if the internal mutex is poisoned (a thread panicked
    /// while holding the lock).  Callers that need to distinguish
    /// "empty" from "poisoned" should use [`stats`](Self::stats) or
    /// inspect [`get_or_init`](Self::get_or_init) errors instead.
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |guard| guard.len())
    }

    /// Returns `true` if the cache contains no entries.
    ///
    /// Returns `true` on a poisoned mutex (same as [`len`](Self::len)
    /// returning `0`).  Use [`is_poisoned`](Self::is_poisoned) first if
    /// the distinction matters.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the internal mutex has been poisoned.
    ///
    /// A poisoned mutex means a thread panicked while holding the lock.
    /// After poisoning, [`len`](Self::len) and [`stats`](Self::stats)
    /// return zero-valued results and [`get_or_init`](Self::get_or_init)
    /// returns [`CacheError::Poisoned`].
    pub fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }

    /// Removes all cached entries.
    ///
    /// Existing `Arc` clones held by node instances remain alive until those
    /// clones are dropped.  An init closure that started **before** this call
    /// may also re-insert its key after `clear` returns (the insert happens
    /// under a second lock acquisition).  This is intended for tests and
    /// best-effort cache maintenance, not as a synchronization primitive.
    ///
    /// No-ops silently if the mutex is poisoned.  Use
    /// [`is_poisoned`](Self::is_poisoned) to check beforehand.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }

    /// Returns current cache statistics.
    ///
    /// Counters are read with `Relaxed` ordering independently of one
    /// another and of `entries` (which acquires the mutex), so the
    /// returned snapshot is **not** globally consistent — e.g. `hits +
    /// misses + init_races` may not equal the total number of
    /// `get_or_init` calls observed so far.  This is acceptable for
    /// diagnostic dashboards; callers needing exact accounting should
    /// synchronize externally.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            init_races: self.init_races.load(Ordering::Relaxed),
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
    /// one value is stored; the other is dropped.  See [`CacheStats`] for
    /// the precise hit / miss / init-race counting semantics.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Poisoned`] if the mutex is poisoned, or
    /// [`CacheError::Init`] if `init` fails.
    pub fn get_or_init<F>(&self, key: K, init: F) -> Result<Arc<V>, CacheError>
    where
        F: FnOnce(&K) -> Result<V, String>,
    {
        // Fast path: check if key already exists.
        {
            let guard = self.inner.lock().map_err(|_| CacheError::Poisoned)?;
            if let Some(value) = guard.get(&key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Arc::clone(value));
            }
        }
        // Lock dropped — run init outside the lock.

        let value = init(&key).map_err(CacheError::Init)?;

        // Re-lock and insert if still missing (another thread may have won).
        let mut guard = self.inner.lock().map_err(|_| CacheError::Poisoned)?;

        let arc = match guard.entry(key) {
            Entry::Occupied(e) => {
                self.init_races.fetch_add(1, Ordering::Relaxed);
                Arc::clone(e.get())
            },
            Entry::Vacant(e) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                let arc = Arc::new(value);
                e.insert(Arc::clone(&arc));
                arc
            },
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
        assert_eq!(stats.init_races, 0);
        assert_eq!(stats.entries, 2);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Barrier;
        use std::thread;

        let cache = Arc::new(ResourceCache::<u32, String>::new());
        let barrier = Arc::new(Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let b = barrier.clone();
                let c = cache.clone();
                thread::spawn(move || {
                    b.wait();
                    c.get_or_init(0, |_| Ok(format!("from-thread-{i}"))).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for r in &results {
            assert!(Arc::ptr_eq(r, &results[0]));
        }
    }

    #[test]
    fn init_races_counted() {
        use std::sync::Barrier;
        use std::thread;

        let cache = Arc::new(ResourceCache::<u32, String>::new());
        let thread_count = 10;
        let start = Arc::new(Barrier::new(thread_count));
        let inside_init = Arc::new(Barrier::new(thread_count));
        let handles: Vec<_> = (0..thread_count)
            .map(|i| {
                let s = start.clone();
                let ii = inside_init.clone();
                let c = cache.clone();
                thread::spawn(move || {
                    s.wait();
                    c.get_or_init(42, |_| {
                        ii.wait();
                        Ok(format!("from-{i}"))
                    })
                    .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for r in &results {
            assert!(Arc::ptr_eq(r, &results[0]));
        }
        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.misses, 1);
        assert!(stats.init_races >= 1, "expected at least 1 init race, got {}", stats.init_races);
        assert_eq!(stats.misses + stats.init_races, thread_count as u64);
    }
}
