// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Resource management for plugins.
//!
//! This module provides centralized management of shared resources (primarily ML models)
//! that can be expensive to load and should be shared across multiple node instances.
//!
//! # Key Features
//!
//! - **Automatic deduplication**: Resources are content-addressed by (plugin kind, params hash)
//! - **Reference counting**: Resources are kept alive while any node uses them
//! - **Configurable lifecycle**: Keep loaded until explicit unload, or use LRU eviction
//! - **Thread-safe**: Safe to use from multiple pipelines concurrently
//! - **Async initialization**: Resources can perform async I/O or blocking operations
//!
//! # Example
//!
//! ```rust,no_run
//! use streamkit_core::resource_manager::{
//!     ResourceManager, Resource, ResourcePolicy, ResourceKey
//! };
//! use std::sync::Arc;
//!
//! // Define a custom resource type
//! struct MyModel {
//!     data: Vec<f32>,
//! }
//!
//! impl Resource for MyModel {
//!     fn size_bytes(&self) -> usize {
//!         self.data.len() * std::mem::size_of::<f32>()
//!     }
//!
//!     fn resource_type(&self) -> &str {
//!         "ml_model"
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create resource manager
//!     let policy = ResourcePolicy {
//!         keep_loaded: true,
//!         max_memory_mb: None,
//!     };
//!     let manager = ResourceManager::new(policy);
//!
//!     // Get or create a resource using a ResourceKey
//!     let key = ResourceKey::new("my_plugin", "param_hash");
//!     let resource = manager.get_or_create(
//!         key,
//!         || async {
//!             // Load model (runs only once per unique key)
//!             Ok(Arc::new(MyModel { data: vec![0.0; 1000] }) as Arc<dyn Resource>)
//!         }
//!     ).await.unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A resource that can be shared across multiple node instances.
///
/// Resources are typically expensive to create (ML models, GPU contexts, etc.)
/// and benefit from sharing across multiple pipeline instances.
pub trait Resource: Send + Sync {
    /// Returns the approximate memory footprint in bytes.
    /// Used for LRU eviction when memory limits are configured.
    fn size_bytes(&self) -> usize;

    /// Returns a human-readable type identifier (e.g., "ml_model", "gpu_context").
    /// Used for observability and debugging.
    fn resource_type(&self) -> &str;
}

/// Configuration policy for resource lifecycle management.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// If true, resources are kept loaded until explicitly unloaded or server shutdown.
    /// If false, resources may be evicted based on other policies (e.g., LRU).
    pub keep_loaded: bool,

    /// Optional memory limit in megabytes. When exceeded, least-recently-used
    /// resources are evicted until memory usage is below the limit.
    /// Only applies when keep_loaded is false.
    pub max_memory_mb: Option<usize>,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self { keep_loaded: true, max_memory_mb: None }
    }
}

/// A unique key identifying a resource based on plugin kind and parameters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    /// The plugin kind (e.g., "plugin::native::kokoro", "plugin::native::whisper")
    pub plugin_kind: String,

    /// Hash of canonicalized parameters that affect resource creation
    pub params_hash: String,
}

impl ResourceKey {
    /// Create a new resource key from plugin kind and params hash.
    pub fn new(plugin_kind: impl Into<String>, params_hash: impl Into<String>) -> Self {
        Self { plugin_kind: plugin_kind.into(), params_hash: params_hash.into() }
    }
}

impl fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.plugin_kind, self.params_hash)
    }
}

/// Entry in the resource cache with metadata for LRU eviction.
struct ResourceEntry {
    resource: Arc<dyn Resource>,
    last_accessed: std::time::Instant,
}

/// Centralized manager for shared plugin resources.
///
/// The ResourceManager maintains a cache of resources that can be shared across
/// multiple node instances. It handles lifecycle management, deduplication, and
/// optional memory-based eviction.
pub struct ResourceManager {
    resources: Arc<Mutex<HashMap<ResourceKey, ResourceEntry>>>,
    policy: ResourcePolicy,
}

impl ResourceManager {
    /// Create a new ResourceManager with the specified policy.
    pub fn new(policy: ResourcePolicy) -> Self {
        Self { resources: Arc::new(Mutex::new(HashMap::new())), policy }
    }

    /// Get an existing resource or create it using the provided factory.
    ///
    /// If a resource with the given key already exists, it is returned immediately.
    /// Otherwise, the factory is called to create a new resource, which is then
    /// cached for future use.
    ///
    /// # Arguments
    ///
    /// * `key` - Unique identifier for the resource
    /// * `factory` - Async function that creates the resource if needed
    ///
    /// # Errors
    ///
    /// Returns an error if the factory function fails to create the resource.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use streamkit_core::resource_manager::{
    ///     ResourceManager, Resource, ResourcePolicy, ResourceKey, ResourceError
    /// };
    /// use std::sync::Arc;
    ///
    /// struct MyModel { data: Vec<f32> }
    ///
    /// impl Resource for MyModel {
    ///     fn size_bytes(&self) -> usize { self.data.len() * 4 }
    ///     fn resource_type(&self) -> &str { "ml_model" }
    /// }
    ///
    /// async fn example() -> Result<(), ResourceError> {
    ///     let manager = ResourceManager::new(ResourcePolicy::default());
    ///     let resource = manager.get_or_create(
    ///         ResourceKey::new("my_plugin", "model_v1"),
    ///         || async {
    ///             Ok(Arc::new(MyModel { data: vec![0.0; 1000] }) as Arc<dyn Resource>)
    ///         }
    ///     ).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn get_or_create<F, Fut>(
        &self,
        key: ResourceKey,
        factory: F,
    ) -> Result<Arc<dyn Resource>, ResourceError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<dyn Resource>, ResourceError>>,
    {
        // Fast path: resource already exists
        {
            let mut cache = self.resources.lock().await;
            if let Some(entry) = cache.get_mut(&key) {
                entry.last_accessed = std::time::Instant::now();
                return Ok(entry.resource.clone());
            }
        }

        // Slow path: create new resource
        let resource = factory().await?;

        // Check if we need to evict resources due to memory limit
        if !self.policy.keep_loaded {
            if let Some(max_mb) = self.policy.max_memory_mb {
                self.evict_if_needed(max_mb, resource.size_bytes()).await;
            }
        }

        // Insert into cache
        let mut cache = self.resources.lock().await;

        // Double-check: another task might have created it while we were waiting
        if let Some(entry) = cache.get_mut(&key) {
            entry.last_accessed = std::time::Instant::now();
            return Ok(entry.resource.clone());
        }

        let entry =
            ResourceEntry { resource: resource.clone(), last_accessed: std::time::Instant::now() };
        cache.insert(key, entry);
        drop(cache);

        Ok(resource)
    }

    /// Evict least-recently-used resources until memory usage is below the limit.
    ///
    /// This method minimizes lock contention by:
    /// 1. Taking a short lock to collect metadata and calculate eviction candidates
    /// 2. Sorting candidates outside the lock (O(n log n) operation)
    /// 3. Re-acquiring the lock only to perform the actual removals
    async fn evict_if_needed(&self, max_mb: usize, new_resource_bytes: usize) {
        let max_bytes = max_mb * 1024 * 1024;

        // Phase 1: Collect metadata under lock (fast)
        let (current_bytes, entries) = {
            let cache = self.resources.lock().await;
            let current_bytes: usize = cache.values().map(|e| e.resource.size_bytes()).sum();

            if current_bytes + new_resource_bytes <= max_bytes {
                return; // No eviction needed
            }

            // Collect entry metadata for sorting (clone keys, copy timestamps and sizes)
            let entries: Vec<_> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.last_accessed, v.resource.size_bytes()))
                .collect();

            // Explicitly drop lock before returning to avoid clippy::significant_drop_tightening
            drop(cache);

            (current_bytes, entries)
        };

        // Phase 2: Sort outside the lock (potentially expensive for large caches)
        let mut entries = entries;
        entries.sort_by_key(|(_, accessed, _)| *accessed);

        // Phase 3: Determine which keys to evict (no lock needed)
        let target_freed = (current_bytes + new_resource_bytes).saturating_sub(max_bytes);
        let mut bytes_to_free = 0;
        let keys_to_evict: Vec<_> = entries
            .into_iter()
            .take_while(|(_, _, size)| {
                if bytes_to_free >= target_freed {
                    return false;
                }
                bytes_to_free += size;
                true
            })
            .map(|(key, _, size)| (key, size))
            .collect();

        if keys_to_evict.is_empty() {
            return;
        }

        // Phase 4: Perform evictions under lock (fast - just HashMap removals)
        {
            let mut cache = self.resources.lock().await;
            for (key, size) in keys_to_evict {
                // Re-check that the key still exists (may have been removed by another task)
                if cache.remove(&key).is_some() {
                    tracing::info!(
                        "Evicting resource {} ({} bytes) due to memory limit",
                        key,
                        size
                    );
                }
            }
        }
    }

    /// Explicitly unload a resource by key.
    ///
    /// This removes the resource from the cache. If other node instances still hold
    /// references to the resource, it will remain in memory until they drop their
    /// references.
    ///
    /// # Errors
    ///
    /// Returns an error if the resource key is not found in the cache.
    pub async fn unload(&self, key: &ResourceKey) -> Result<(), ResourceError> {
        let mut cache = self.resources.lock().await;
        if cache.remove(key).is_some() {
            tracing::info!("Unloaded resource: {}", key);
            Ok(())
        } else {
            Err(ResourceError::NotFound(key.clone()))
        }
    }

    /// Get statistics about currently loaded resources.
    pub async fn stats(&self) -> ResourceStats {
        let cache = self.resources.lock().await;

        let total_size_bytes: usize = cache.values().map(|e| e.resource.size_bytes()).sum();
        let resource_types: HashMap<String, usize> =
            cache.values().fold(HashMap::new(), |mut acc, entry| {
                *acc.entry(entry.resource.resource_type().to_string()).or_insert(0) += 1;
                acc
            });

        ResourceStats { total_resources: cache.len(), total_size_bytes, resource_types }
    }

    /// Clear all cached resources.
    ///
    /// This removes all resources from the cache. Resources that are still in use
    /// by node instances will remain in memory until dropped.
    pub async fn clear(&self) {
        let mut cache = self.resources.lock().await;
        let count = cache.len();
        cache.clear();
        drop(cache);
        tracing::info!("Cleared {} resources from cache", count);
    }
}

/// Statistics about currently loaded resources.
#[derive(Debug, Clone)]
pub struct ResourceStats {
    /// Total number of cached resources
    pub total_resources: usize,

    /// Total memory footprint in bytes
    pub total_size_bytes: usize,

    /// Count of resources by type
    pub resource_types: HashMap<String, usize>,
}

/// Errors that can occur during resource management.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("Resource not found: {0}")]
    NotFound(ResourceKey),

    #[error("Resource initialization failed: {0}")]
    InitializationFailed(String),

    #[error("Resource error: {0}")]
    Other(String),
}

/// Type alias for boxed async resource factories.
pub type ResourceFactory = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Arc<dyn Resource>, ResourceError>> + Send>>
        + Send
        + Sync,
>;

#[cfg(test)]
mod tests {
    use super::*;

    struct TestResource {
        size: usize,
    }

    impl Resource for TestResource {
        fn size_bytes(&self) -> usize {
            self.size
        }

        fn resource_type(&self) -> &'static str {
            "test_resource"
        }
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_resource_deduplication() {
        let manager = ResourceManager::new(ResourcePolicy::default());
        let key = ResourceKey::new("test", "params1");

        let mut create_count = 0;

        // First call should create resource
        let r1 = manager
            .get_or_create(key.clone(), || async {
                create_count += 1;
                Ok(Arc::new(TestResource { size: 1000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        // Second call should reuse cached resource
        let r2 = manager
            .get_or_create(key.clone(), || async {
                create_count += 1;
                Ok(Arc::new(TestResource { size: 1000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        assert_eq!(create_count, 1, "Resource should only be created once");
        assert!(Arc::ptr_eq(&r1, &r2), "Should return same Arc instance");
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_lru_eviction() {
        let policy = ResourcePolicy {
            keep_loaded: false,
            max_memory_mb: Some(1), // 1 MB limit
        };
        let manager = ResourceManager::new(policy);

        // Create three 500KB resources
        let _r1 = manager
            .get_or_create(ResourceKey::new("test", "1"), || async {
                Ok(Arc::new(TestResource { size: 500_000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let _r2 = manager
            .get_or_create(ResourceKey::new("test", "2"), || async {
                Ok(Arc::new(TestResource { size: 500_000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // This should trigger eviction of r1 (oldest)
        let _r3 = manager
            .get_or_create(ResourceKey::new("test", "3"), || async {
                Ok(Arc::new(TestResource { size: 500_000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        let stats = manager.stats().await;
        assert!(
            stats.total_size_bytes <= 1_048_576,
            "Total size should be under 1MB after eviction"
        );
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_stats() {
        let manager = ResourceManager::new(ResourcePolicy::default());

        manager
            .get_or_create(ResourceKey::new("test", "1"), || async {
                Ok(Arc::new(TestResource { size: 1000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        manager
            .get_or_create(ResourceKey::new("test", "2"), || async {
                Ok(Arc::new(TestResource { size: 2000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        let stats = manager.stats().await;
        assert_eq!(stats.total_resources, 2);
        assert_eq!(stats.total_size_bytes, 3000);
        assert_eq!(stats.resource_types.get("test_resource"), Some(&2));
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_unload() {
        let manager = ResourceManager::new(ResourcePolicy::default());
        let key = ResourceKey::new("test", "1");

        manager
            .get_or_create(key.clone(), || async {
                Ok(Arc::new(TestResource { size: 1000 }) as Arc<dyn Resource>)
            })
            .await
            .unwrap();

        manager.unload(&key).await.unwrap();

        let stats = manager.stats().await;
        assert_eq!(stats.total_resources, 0);
    }
}
