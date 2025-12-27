// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use opentelemetry::{global, KeyValue};
use serde::Serialize;
use streamkit_engine::Engine;
use streamkit_plugin_native::LoadedNativePlugin;
use streamkit_plugin_wasm::{
    namespaced_kind as wasm_namespaced_kind, LoadedPlugin as WasmLoadedPlugin, PluginRuntime,
};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// The type of plugin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Wasm,
    Native,
}

/// Summary of a loaded plugin exposed via the HTTP API.
#[derive(Debug, Clone, Serialize)]
pub struct PluginSummary {
    pub kind: String,
    pub original_kind: String,
    pub file_name: String,
    pub categories: Vec<String>,
    pub loaded_at_ms: u128,
    pub plugin_type: PluginType,
}

impl PluginSummary {
    fn from_entry(kind: String, entry: &ManagedPlugin) -> Self {
        let loaded_at_ms = entry.loaded_at.duration_since(UNIX_EPOCH).map_or_else(
            |e| {
                warn!("Failed to compute plugin load time: {}", e);
                0
            },
            |d| d.as_millis(),
        );

        let file_name = entry.file_path.file_name().map_or_else(
            || {
                warn!("Plugin has invalid file path");
                String::from("unknown")
            },
            |f| f.to_string_lossy().into_owned(),
        );

        Self {
            kind,
            original_kind: entry.original_kind.clone(),
            file_name,
            categories: entry.categories.clone(),
            loaded_at_ms,
            plugin_type: entry.plugin_type,
        }
    }
}

enum LoadedPluginInner {
    Wasm(Arc<WasmLoadedPlugin>),
    #[allow(dead_code)] // Kept alive to prevent plugin unloading
    Native(Arc<LoadedNativePlugin>),
}

struct ManagedPlugin {
    plugin: LoadedPluginInner,
    categories: Vec<String>,
    file_path: PathBuf,
    loaded_at: SystemTime,
    original_kind: String,
    plugin_type: PluginType,
}

impl ManagedPlugin {
    fn new_wasm(
        plugin: WasmLoadedPlugin,
        original_kind: String,
        categories: Vec<String>,
        file_path: PathBuf,
    ) -> Self {
        Self {
            plugin: LoadedPluginInner::Wasm(Arc::new(plugin)),
            categories,
            file_path,
            loaded_at: SystemTime::now(),
            original_kind,
            plugin_type: PluginType::Wasm,
        }
    }

    fn new_native(
        plugin: LoadedNativePlugin,
        original_kind: String,
        categories: Vec<String>,
        file_path: PathBuf,
    ) -> Self {
        Self {
            plugin: LoadedPluginInner::Native(Arc::new(plugin)),
            categories,
            file_path,
            loaded_at: SystemTime::now(),
            original_kind,
            plugin_type: PluginType::Native,
        }
    }
}

/// Unified plugin manager that orchestrates loading/unloading both WASM and native plugins
pub struct UnifiedPluginManager {
    wasm_runtime: PluginRuntime,
    plugins: HashMap<String, ManagedPlugin>,
    wasm_directory: PathBuf,
    native_directory: PathBuf,
    engine: Arc<Engine>,
    #[allow(dead_code)] // Will be used when plugins are migrated to new resource system
    resource_manager: Arc<streamkit_core::ResourceManager>,
    // Metrics
    plugins_loaded_gauge: opentelemetry::metrics::Gauge<u64>,
    plugin_operations_counter: opentelemetry::metrics::Counter<u64>,
}

impl UnifiedPluginManager {
    /// Create a new unified plugin manager
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Plugin directories cannot be created
    /// - WASM runtime initialization fails
    pub fn new(
        engine: Arc<Engine>,
        resource_manager: Arc<streamkit_core::ResourceManager>,
        wasm_directory: PathBuf,
        native_directory: PathBuf,
    ) -> Result<Self> {
        if !wasm_directory.exists() {
            std::fs::create_dir_all(&wasm_directory).with_context(|| {
                format!("failed to create WASM plugin directory {}", wasm_directory.display())
            })?;
        }

        if !native_directory.exists() {
            std::fs::create_dir_all(&native_directory).with_context(|| {
                format!("failed to create native plugin directory {}", native_directory.display())
            })?;
        }

        let wasm_runtime =
            PluginRuntime::new(streamkit_plugin_wasm::PluginRuntimeConfig::default())?;

        let meter = global::meter("skit_plugins");
        Ok(Self {
            wasm_runtime,
            plugins: HashMap::new(),
            wasm_directory,
            native_directory,
            engine,
            resource_manager,
            plugins_loaded_gauge: meter
                .u64_gauge("plugins.loaded")
                .with_description("Number of loaded plugins by type")
                .build(),
            plugin_operations_counter: meter
                .u64_counter("plugin.operations")
                .with_description("Plugin load/unload operations")
                .build(),
        })
    }

    /// Load all native plugins from the native directory
    fn load_native_plugins_from_dir(&mut self) -> Result<Vec<PluginSummary>> {
        let mut summaries = Vec::new();

        info!("Loading native plugins...");
        for entry in std::fs::read_dir(&self.native_directory).with_context(|| {
            format!("failed to read native plugin directory {}", self.native_directory.display())
        })? {
            let entry = entry?;
            let path = entry.path();

            // Check for native library extensions
            let extension = path.extension().and_then(|ext| ext.to_str());
            let is_native_lib = matches!(extension, Some("so" | "dylib" | "dll"));

            if !is_native_lib || path.to_string_lossy().ends_with(".d") {
                continue;
            }

            match self.load_native_plugin(&path) {
                Ok(summary) => {
                    info!(plugin = %summary.kind, file = ?path, plugin_type = ?summary.plugin_type, "Loaded plugin from disk");
                    summaries.push(summary);
                },
                Err(err) => {
                    warn!(error = %err, file = ?path, "Failed to load native plugin from disk");
                },
            }
        }

        Ok(summaries)
    }

    /// Load all WASM plugins from the WASM directory
    fn load_wasm_plugins_from_dir(&mut self) -> Result<Vec<PluginSummary>> {
        let mut summaries = Vec::new();

        info!("Loading WASM plugins...");
        for entry in std::fs::read_dir(&self.wasm_directory).with_context(|| {
            format!("failed to read WASM plugin directory {}", self.wasm_directory.display())
        })? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
                continue;
            }

            match self.load_wasm_plugin(&path) {
                Ok(summary) => {
                    info!(plugin = %summary.kind, file = ?path, plugin_type = ?summary.plugin_type, "Loaded plugin from disk");
                    summaries.push(summary);
                },
                Err(err) => {
                    warn!(error = %err, file = ?path, "Failed to load WASM plugin from disk");
                },
            }
        }

        Ok(summaries)
    }

    /// Loads all existing plugins from both WASM and native directories.
    /// Native plugins are loaded first as they are faster to initialize.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin directories cannot be read.
    /// Individual plugin load failures are logged but do not prevent other plugins from loading.
    pub fn load_existing(&mut self) -> Result<Vec<PluginSummary>> {
        let mut summaries = self.load_native_plugins_from_dir()?;
        summaries.extend(self.load_wasm_plugins_from_dir()?);
        Ok(summaries)
    }

    /// Pre-warms a plugin by creating a dummy node instance to trigger model loading.
    /// This reduces latency for the first real usage of the plugin.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The registry lock is poisoned
    /// - Creating a node instance for the plugin fails
    async fn prewarm_engine_plugin(
        engine: Arc<Engine>,
        kind: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        debug!(
            plugin = %kind,
            params_present = params.is_some(),
            "Pre-warming plugin (creating instance to load models)"
        );

        // Use spawn_blocking for safety - GPU initialization might throw C++ exceptions
        let kind = kind.to_string();
        let kind_clone = kind.clone(); // Clone for error message after move
        let registry = engine.registry.clone();

        // Capture current span to propagate tracing context to blocking thread
        let span = tracing::Span::current();

        tokio::task::spawn_blocking(move || {
            // Enter the span to ensure tracing logs work in blocking context
            let _enter = span.enter();

            // Create a node instance with specified params to trigger initialization
            let _node = {
                let registry =
                    registry.read().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;

                registry.create_node(&kind, params.as_ref()).with_context(|| {
                    format!("Failed to create warmup instance for plugin '{kind}'")
                })?
            };

            // Node is dropped immediately, but initialization side effects (model loading via Arc) persist
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Pre-warming task panicked")??;

        info!(plugin = %kind_clone, "Pre-warming completed successfully");
        Ok(())
    }

    /// Spawns a background task to load all existing plugins asynchronously.
    /// If pre-warming is configured, plugins will be pre-warmed after loading.
    pub fn spawn_load_existing(
        manager: SharedUnifiedPluginManager,
        prewarm_config: crate::config::PrewarmConfig,
    ) {
        tokio::spawn(async move {
            info!("Starting background plugin loading");

            let result = {
                let mut mgr = manager.lock().await;
                mgr.load_existing()
            };

            match result {
                Ok(summaries) => {
                    if summaries.is_empty() {
                        info!("Background plugin loading completed (no plugins found)");
                    } else {
                        info!(
                            count = summaries.len(),
                            plugins = ?summaries.iter().map(|s| (s.kind.as_str(), s.plugin_type)).collect::<Vec<_>>(),
                            "Completed background plugin loading"
                        );
                    }

                    // Pre-warm plugins if configured
                    if prewarm_config.enabled && !prewarm_config.plugins.is_empty() {
                        info!(count = prewarm_config.plugins.len(), "Starting plugin pre-warming");

                        let engine = {
                            let mgr = manager.lock().await;
                            mgr.engine.clone()
                        };

                        for plugin_config in &prewarm_config.plugins {
                            // Try primary params
                            match Self::prewarm_engine_plugin(
                                engine.clone(),
                                &plugin_config.kind,
                                plugin_config.params.clone(),
                            )
                            .await
                            {
                                Ok(()) => {
                                    info!(plugin = %plugin_config.kind, "Successfully pre-warmed plugin");
                                },
                                Err(err) => {
                                    warn!(plugin = %plugin_config.kind, error = %err, "Failed to pre-warm plugin with primary params");

                                    // Try fallback params if provided
                                    if let Some(fallback_params) = &plugin_config.fallback_params {
                                        info!(plugin = %plugin_config.kind, "Attempting pre-warm with fallback params");

                                        match Self::prewarm_engine_plugin(
                                            engine.clone(),
                                            &plugin_config.kind,
                                            Some(fallback_params.clone()),
                                        )
                                        .await
                                        {
                                            Ok(()) => {
                                                info!(plugin = %plugin_config.kind, "Successfully pre-warmed plugin with fallback params");
                                            },
                                            Err(fallback_err) => {
                                                warn!(
                                                    plugin = %plugin_config.kind,
                                                    primary_error = %err,
                                                    fallback_error = %fallback_err,
                                                    "Failed to pre-warm plugin with both primary and fallback params"
                                                );
                                            },
                                        }
                                    }
                                },
                            }
                        }

                        info!("Plugin pre-warming completed");
                    }
                },
                Err(err) => {
                    warn!(error = %err, "Failed to load plugins in background");
                },
            }
        });
    }

    /// Load a WASM plugin from a file path
    fn load_wasm_plugin<P: AsRef<Path>>(&mut self, path: P) -> Result<PluginSummary> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(anyhow!("Plugin file {} does not exist", path.to_string_lossy()));
        }

        let plugin = self
            .wasm_runtime
            .load_plugin(path)
            .map_err(|e| {
                tracing::error!(error = %e, path = ?path, "Detailed plugin load error");
                e
            })
            .with_context(|| format!("failed to compile WASM plugin {}", path.to_string_lossy()))?;

        let metadata = plugin.metadata().clone();
        let original_kind = metadata.kind.clone();
        let kind = wasm_namespaced_kind(&original_kind)
            .map_err(|e| anyhow!("Invalid plugin kind '{original_kind}': {e}"))?;

        if self.plugins.contains_key(&kind) {
            return Err(anyhow!(
                "A plugin providing node '{original_kind}' (registered as '{kind}') is already loaded"
            ));
        }

        let param_schema: serde_json::Value = serde_json::from_str(&metadata.param_schema)
            .with_context(|| format!("Plugin '{kind}' provided invalid param_schema JSON"))?;
        let categories = metadata.categories;

        // Ensure we don't override an existing node definition
        {
            let registry =
                self.engine.registry.read().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;
            if registry.contains(&kind) {
                return Err(anyhow!(
                    "Node kind '{kind}' is already registered; refusing to overwrite it with a plugin"
                ));
            }
        }

        let managed =
            ManagedPlugin::new_wasm(plugin, original_kind, categories.clone(), path.to_path_buf());

        let plugin_arc = match &managed.plugin {
            LoadedPluginInner::Wasm(p) => Arc::clone(p),
            LoadedPluginInner::Native(_) => {
                return Err(anyhow!(
                    "internal error: expected WASM plugin after successful WASM load"
                ));
            },
        };

        {
            let mut registry =
                self.engine.registry.write().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;

            let categories_for_registry = categories;
            registry.register_dynamic(
                &kind,
                move |params| plugin_arc.create_node(params),
                param_schema,
                categories_for_registry,
                false,
            );
        }

        let summary = PluginSummary::from_entry(kind.clone(), &managed);
        self.plugins.insert(kind, managed);

        // Update metrics
        self.plugin_operations_counter
            .add(1, &[KeyValue::new("operation", "load"), KeyValue::new("plugin_type", "wasm")]);
        self.update_loaded_gauge();

        Ok(summary)
    }

    /// Load a native plugin from a file path
    fn load_native_plugin<P: AsRef<Path>>(&mut self, path: P) -> Result<PluginSummary> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(anyhow!("Native plugin file {} does not exist", path.to_string_lossy()));
        }

        let plugin = LoadedNativePlugin::load(path)
            .map_err(|e| {
                tracing::error!(error = %e, path = ?path, "Detailed native plugin load error");
                e
            })
            .with_context(|| format!("failed to load native plugin {}", path.to_string_lossy()))?;

        let metadata = plugin.metadata();
        let original_kind = metadata.kind.clone();
        let kind = streamkit_plugin_native::namespaced_kind(&original_kind)
            .with_context(|| format!("invalid plugin kind '{original_kind}'"))?;
        let categories = metadata.categories.clone();

        if self.plugins.contains_key(&kind) {
            return Err(anyhow!(
                "A plugin providing node '{original_kind}' (registered as '{kind}') is already loaded"
            ));
        }

        // Ensure we don't override an existing node definition
        {
            let registry =
                self.engine.registry.read().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;
            if registry.contains(&kind) {
                return Err(anyhow!(
                    "Node kind '{kind}' is already registered; refusing to overwrite it with a plugin"
                ));
            }
        }

        // Register with the engine's node registry
        {
            let mut registry =
                self.engine.registry.write().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;

            streamkit_plugin_native::register_plugins(&mut registry, vec![plugin.clone()])
                .with_context(|| format!("failed to register plugin '{kind}'"))?;
        }

        let managed =
            ManagedPlugin::new_native(plugin, original_kind, categories, path.to_path_buf());

        let summary = PluginSummary::from_entry(kind.clone(), &managed);
        self.plugins.insert(kind, managed);

        // Update metrics
        self.plugin_operations_counter
            .add(1, &[KeyValue::new("operation", "load"), KeyValue::new("plugin_type", "native")]);
        self.update_loaded_gauge();

        Ok(summary)
    }

    /// Unloads a plugin by its node kind. Optionally removes the plugin file from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The plugin is not currently loaded
    /// - The registry lock is poisoned
    pub fn unload_plugin(&mut self, kind: &str, remove_file: bool) -> Result<PluginSummary> {
        let managed = self
            .plugins
            .remove(kind)
            .ok_or_else(|| anyhow!("Plugin '{kind}' is not currently loaded"))?;

        {
            let mut registry =
                self.engine.registry.write().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;

            if !registry.unregister(kind) {
                warn!(
                    "Plugin manager attempted to unregister node '{}' but it was not present",
                    kind
                );
            }
        }

        if remove_file {
            if let Err(err) = std::fs::remove_file(&managed.file_path) {
                warn!(
                    error = %err,
                    file = ?managed.file_path,
                    "Failed to remove plugin file during unload"
                );
            }
        }

        let plugin_type = match managed.plugin_type {
            PluginType::Wasm => "wasm",
            PluginType::Native => "native",
        };

        // Update metrics
        self.plugin_operations_counter.add(
            1,
            &[KeyValue::new("operation", "unload"), KeyValue::new("plugin_type", plugin_type)],
        );
        self.update_loaded_gauge();

        Ok(PluginSummary::from_entry(kind.to_string(), &managed))
    }

    /// Returns all loaded plugins as summaries.
    pub fn list_plugins(&self) -> Vec<PluginSummary> {
        self.plugins
            .iter()
            .map(|(kind, entry)| PluginSummary::from_entry(kind.clone(), entry))
            .collect()
    }

    /// Helper method to update the loaded plugins gauge by counting each type
    fn update_loaded_gauge(&self) {
        let wasm_count =
            self.plugins.values().filter(|p| p.plugin_type == PluginType::Wasm).count() as u64;
        let native_count =
            self.plugins.values().filter(|p| p.plugin_type == PluginType::Native).count() as u64;

        self.plugins_loaded_gauge.record(wasm_count, &[KeyValue::new("plugin_type", "wasm")]);
        self.plugins_loaded_gauge.record(native_count, &[KeyValue::new("plugin_type", "native")]);
    }

    /// Saves raw plugin bytes into the managed directory and loads the resulting plugin.
    /// Automatically detects plugin type based on file extension.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file name is empty or has an invalid extension
    /// - The plugin file cannot be written to disk
    /// - The plugin fails to load after being written
    /// - On Unix systems, setting executable permissions fails
    #[allow(dead_code)] // Useful for non-streaming callers; HTTP uses `load_from_temp_file`.
    pub fn load_from_bytes(&mut self, file_name: &str, bytes: &[u8]) -> Result<PluginSummary> {
        let (target_path, plugin_type) = self.validate_plugin_upload_target(file_name)?;

        std::fs::write(&target_path, bytes)
            .with_context(|| format!("failed to write plugin file {}", target_path.display()))?;

        self.load_from_written_path(plugin_type, target_path)
    }

    /// Moves an already-written plugin file into the managed directory and loads it.
    ///
    /// This avoids buffering large uploads in memory by allowing callers to stream the upload
    /// directly to a temporary file on disk, then atomically move it into place.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file name is invalid
    /// - The temp file does not exist or is not a regular file
    /// - The temp file cannot be moved into the plugins directory
    /// - The plugin fails to load after being moved
    pub fn load_from_temp_file(
        &mut self,
        file_name: &str,
        temp_path: &Path,
    ) -> Result<PluginSummary> {
        let (target_path, plugin_type) = self.validate_plugin_upload_target(file_name)?;

        let meta = std::fs::metadata(temp_path)
            .with_context(|| format!("failed to stat temp plugin file {}", temp_path.display()))?;
        if !meta.is_file() {
            return Err(anyhow!("temp plugin path is not a file: {}", temp_path.display()));
        }

        // Prefer atomic move; fall back to copy+remove for cross-device temp dirs.
        if let Err(e) = std::fs::rename(temp_path, &target_path) {
            debug!(
                error = %e,
                from = %temp_path.display(),
                to = %target_path.display(),
                "rename failed; falling back to copy+remove"
            );
            std::fs::copy(temp_path, &target_path).with_context(|| {
                format!(
                    "failed to copy temp plugin file from {} to {}",
                    temp_path.display(),
                    target_path.display()
                )
            })?;
            let _ = std::fs::remove_file(temp_path);
        }

        match self.load_from_written_path(plugin_type, target_path.clone()) {
            Ok(summary) => Ok(summary),
            Err(e) => {
                let _ = std::fs::remove_file(&target_path);
                Err(e)
            },
        }
    }

    fn validate_plugin_upload_target(&self, file_name: &str) -> Result<(PathBuf, PluginType)> {
        use std::path::Component;

        const MAX_PLUGIN_FILENAME_LEN: usize = 255;

        let sanitized = file_name.trim();
        if sanitized.is_empty() {
            return Err(anyhow!("Plugin file name must not be empty"));
        }
        if sanitized.len() > MAX_PLUGIN_FILENAME_LEN {
            return Err(anyhow!(
                "Plugin file name is too long (max {MAX_PLUGIN_FILENAME_LEN} characters)"
            ));
        }

        let path = Path::new(sanitized);
        let is_single_normal_component = {
            let mut components = path.components();
            matches!((components.next(), components.next()), (Some(Component::Normal(_)), None))
        };
        if path.is_absolute() || !is_single_normal_component || sanitized.contains("..") {
            return Err(anyhow!(
                "Plugin file name must be a plain file name (no paths or '..' segments)"
            ));
        }

        let extension = Path::new(sanitized).extension().and_then(|ext| ext.to_str());

        let (target_path, plugin_type) = match extension {
            Some("wasm") => (self.wasm_directory.join(sanitized), PluginType::Wasm),
            Some("so" | "dylib" | "dll") => {
                (self.native_directory.join(sanitized), PluginType::Native)
            },
            _ => {
                return Err(anyhow!(
                    "Plugin file must have a valid extension (.wasm for WASM plugins, .so/.dylib/.dll for native plugins)"
                ));
            },
        };

        Ok((target_path, plugin_type))
    }

    fn load_from_written_path(
        &mut self,
        plugin_type: PluginType,
        target_path: PathBuf,
    ) -> Result<PluginSummary> {
        // Set executable permissions for native libraries on Unix systems
        #[cfg(unix)]
        if plugin_type == PluginType::Native {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&target_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&target_path, perms)?;
        }

        match plugin_type {
            PluginType::Wasm => self.load_wasm_plugin(target_path),
            PluginType::Native => self.load_native_plugin(target_path),
        }
    }
}

/// Convenience alias for sharing the unified plugin manager behind an async mutex.
pub type SharedUnifiedPluginManager = Arc<Mutex<UnifiedPluginManager>>;
