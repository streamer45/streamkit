// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Generic buffer pooling for frame/sample reuse.
//!
//! The pool is intentionally simple:
//! - fixed size buckets (by element count)
//! - bounded buffers per bucket
//! - `PooledFrameData<T>` returns its backing buffer to the pool on drop
//!
//! This is primarily used to amortize per-frame allocations in hot paths like Opus decode.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug, Clone)]
pub struct PoolStats {
    pub hits: u64,
    pub misses: u64,
    pub buckets: Vec<BucketStats>,
}

#[derive(Debug, Clone)]
pub struct BucketStats {
    pub bucket_size: usize,
    pub available: usize,
    pub max_per_bucket: usize,
}

#[derive(Clone)]
pub struct PoolHandle<T>(Weak<Mutex<PoolInner<T>>>);

impl<T> PoolHandle<T> {
    fn upgrade(&self) -> Option<Arc<Mutex<PoolInner<T>>>> {
        self.0.upgrade()
    }
}

struct PoolInner<T> {
    bucket_sizes: Vec<usize>,
    max_per_bucket: usize,
    buckets: Vec<Vec<Vec<T>>>,
    hits: u64,
    misses: u64,
}

impl<T> PoolInner<T> {
    fn bucket_index_for_min_len(&self, min_len: usize) -> Option<usize> {
        self.bucket_sizes.iter().position(|&size| size >= min_len)
    }

    fn bucket_index_for_storage_len(&self, storage_len: usize) -> Option<usize> {
        self.bucket_sizes.iter().position(|&size| size == storage_len)
    }
}

/// Thread-safe pool for `Vec<T>` buffers.
pub struct FramePool<T> {
    inner: Arc<Mutex<PoolInner<T>>>,
}

impl<T> Clone for FramePool<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T> FramePool<T> {
    /// Create a pool with fixed buckets.
    ///
    /// `bucket_sizes` should be sorted ascending; this function will sort/dedup defensively.
    pub fn with_buckets(mut bucket_sizes: Vec<usize>, max_per_bucket: usize) -> Self {
        bucket_sizes.sort_unstable();
        bucket_sizes.dedup();
        let buckets = (0..bucket_sizes.len()).map(|_| Vec::new()).collect();
        Self {
            inner: Arc::new(Mutex::new(PoolInner {
                bucket_sizes,
                max_per_bucket,
                buckets,
                hits: 0,
                misses: 0,
            })),
        }
    }

    pub fn handle(&self) -> PoolHandle<T> {
        PoolHandle(Arc::downgrade(&self.inner))
    }

    pub fn stats(&self) -> PoolStats {
        let Ok(guard) = self.inner.lock() else {
            return PoolStats { hits: 0, misses: 0, buckets: Vec::new() };
        };
        PoolStats {
            hits: guard.hits,
            misses: guard.misses,
            buckets: guard
                .bucket_sizes
                .iter()
                .enumerate()
                .map(|(idx, &bucket_size)| BucketStats {
                    bucket_size,
                    available: guard.buckets[idx].len(),
                    max_per_bucket: guard.max_per_bucket,
                })
                .collect(),
        }
    }
}

impl<T: Clone + Default> FramePool<T> {
    pub fn preallocated(bucket_sizes: &[usize], buffers_per_bucket: usize) -> Self {
        let pool = Self::with_buckets(bucket_sizes.to_vec(), buffers_per_bucket);
        let Ok(mut guard) = pool.inner.lock() else {
            return pool;
        };

        for idx in 0..guard.bucket_sizes.len() {
            let bucket_size = guard.bucket_sizes[idx];
            for _ in 0..buffers_per_bucket {
                guard.buckets[idx].push(vec![T::default(); bucket_size]);
            }
        }
        drop(guard);
        pool
    }

    /// Get pooled storage for at least `min_len` elements.
    ///
    /// If `min_len` doesn't fit in any bucket, returns a non-pooled buffer of exact size.
    pub fn get(&self, min_len: usize) -> PooledFrameData<T> {
        let (handle, bucket_idx, bucket_size, maybe_buf) = {
            let Ok(mut guard) = self.inner.lock() else {
                return PooledFrameData::from_vec(vec![T::default(); min_len]);
            };
            let Some(bucket_idx) = guard.bucket_index_for_min_len(min_len) else {
                guard.misses += 1;
                return PooledFrameData::from_vec(vec![T::default(); min_len]);
            };
            let bucket_size = guard.bucket_sizes[bucket_idx];
            let buf = guard.buckets[bucket_idx].pop();
            if buf.is_some() {
                guard.hits += 1;
            } else {
                guard.misses += 1;
            }
            (self.handle(), bucket_idx, bucket_size, buf)
        };

        let data = maybe_buf.unwrap_or_else(|| vec![T::default(); bucket_size]);
        PooledFrameData::from_pool(data, min_len, handle, bucket_idx)
    }
}

/// A pooled buffer with a logical length.
///
/// For pooled instances, `data.len()` is the bucket size and `len` is the logical slice length.
pub struct PooledFrameData<T> {
    data: Vec<T>,
    len: usize,
    pool: Option<PoolHandle<T>>,
    bucket_idx: Option<usize>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for PooledFrameData<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledFrameData")
            .field("len", &self.len)
            .field("storage_len", &self.data.len())
            .field("pooled", &self.pool.is_some())
            .finish_non_exhaustive()
    }
}

impl<T> PooledFrameData<T> {
    pub const fn from_vec(data: Vec<T>) -> Self {
        let len = data.len();
        Self { data, len, pool: None, bucket_idx: None }
    }

    fn from_pool(data: Vec<T>, len: usize, pool: PoolHandle<T>, bucket_idx: usize) -> Self {
        let len = len.min(data.len());
        Self { data, len, pool: Some(pool), bucket_idx: Some(bucket_idx) }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub const fn storage_len(&self) -> usize {
        self.data.len()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data[..self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data[..self.len]
    }

    /// Set the logical length (must be <= storage length).
    pub fn truncate(&mut self, new_len: usize) {
        self.len = new_len.min(self.data.len());
    }

    /// Consume into a detached Vec of exactly the logical length.
    pub fn into_vec(mut self) -> Vec<T> {
        self.pool = None;
        self.bucket_idx = None;
        let logical_len = self.len;
        let mut data = std::mem::take(&mut self.data);
        data.truncate(logical_len);
        data
    }
}

impl<T: Clone + Default> Clone for PooledFrameData<T> {
    fn clone(&self) -> Self {
        // Fast path: if pooled and pool is alive, try to allocate from a bucket to avoid heap alloc.
        if let Some(pool) = &self.pool {
            if let Some(inner) = pool.upgrade() {
                if let Ok(mut guard) = inner.lock() {
                    if let Some(bucket_idx) = guard.bucket_index_for_min_len(self.len) {
                        let bucket_size = guard.bucket_sizes[bucket_idx];
                        let mut data = guard
                            .buckets
                            .get_mut(bucket_idx)
                            .and_then(std::vec::Vec::pop)
                            .unwrap_or_else(|| vec![T::default(); bucket_size]);
                        guard.hits += 1;

                        data[..self.len].clone_from_slice(self.as_slice());
                        return Self::from_pool(data, self.len, pool.clone(), bucket_idx);
                    }
                }
            }
        }

        Self::from_vec(self.as_slice().to_vec())
    }
}

impl<T> Deref for PooledFrameData<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T> DerefMut for PooledFrameData<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<T> Drop for PooledFrameData<T> {
    fn drop(&mut self) {
        let Some(pool) = self.pool.take() else { return };
        let Some(bucket_idx) = self.bucket_idx.take() else { return };
        let Some(inner) = pool.upgrade() else { return };
        let Ok(mut guard) = inner.lock() else { return };

        // Only return buffers that match an existing bucket exactly.
        let Some(expected_bucket_idx) = guard.bucket_index_for_storage_len(self.data.len()) else {
            return;
        };
        if expected_bucket_idx != bucket_idx {
            return;
        }

        if guard.buckets[bucket_idx].len() >= guard.max_per_bucket {
            return;
        }

        // Restore logical length to full storage, to keep future clones/copies simple.
        self.len = self.data.len();
        guard.buckets[bucket_idx].push(std::mem::take(&mut self.data));
    }
}

pub type AudioFramePool = FramePool<f32>;
pub type PooledSamples = PooledFrameData<f32>;

pub const DEFAULT_AUDIO_BUCKET_SIZES: &[usize] = &[960, 1920, 3840, 7680];
pub const DEFAULT_AUDIO_BUFFERS_PER_BUCKET: usize = 32;

impl FramePool<f32> {
    pub fn audio_default() -> Self {
        Self::preallocated(DEFAULT_AUDIO_BUCKET_SIZES, DEFAULT_AUDIO_BUFFERS_PER_BUCKET)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn returns_to_pool_on_drop() {
        let pool = FramePool::<u8>::preallocated(&[10], 1);
        let handle = pool.handle();
        assert_eq!(pool.stats().buckets[0].available, 1);

        {
            let mut buf = pool.get(5);
            assert_eq!(buf.len(), 5);
            assert_eq!(buf.storage_len(), 10);
            buf.as_mut_slice().fill(7);
            assert_eq!(pool.stats().buckets[0].available, 0);
            drop(handle);
        }

        // Returned to pool.
        assert_eq!(pool.stats().buckets[0].available, 1);
    }

    #[test]
    fn clone_prefers_pool_when_available() {
        let pool = FramePool::<u8>::preallocated(&[4], 2);
        let mut a = pool.get(3);
        a.as_mut_slice().copy_from_slice(&[1, 2, 3]);
        let b = a.clone();
        assert_eq!(b.as_slice(), &[1, 2, 3]);
        // Dropping both should return 2 buffers back to the pool.
        drop(a);
        drop(b);
        assert_eq!(pool.stats().buckets[0].available, 2);
    }
}
