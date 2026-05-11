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

use crate::{
    marketplace::PluginKind,
    plugin_assets::read_local_plugin_manifest,
    plugin_paths,
    plugin_records::{active_dir as plugin_active_dir, namespaced_kind as active_namespaced_kind},
};

/// Sentinel substring used in conflict-detection errors to distinguish
/// expected dedup skips from genuine failures.  Referenced by both the
/// producer (`load_native_plugin` / `check_kind_conflict`) and the
/// consumer (`load_native_dir_plugins`).
const ERR_ALREADY_LOADED: &str = "already loaded";
const ERR_ALREADY_REGISTERED: &str = "already registered";

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
    pub version: Option<String>,
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
            version: entry.version.clone(),
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
    version: Option<String>,
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
            version: None,
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
            version: None,
        }
    }
}

/// Unified plugin manager that orchestrates loading/unloading both WASM and native plugins
pub struct UnifiedPluginManager {
    wasm_runtime: PluginRuntime,
    plugins: HashMap<String, ManagedPlugin>,
    plugin_base_dir: PathBuf,
    wasm_directory: PathBuf,
    native_directory: PathBuf,
    native_call_timeout: Option<std::time::Duration>,
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
        plugin_base_dir: PathBuf,
        wasm_directory: PathBuf,
        native_directory: PathBuf,
        native_call_timeout: Option<std::time::Duration>,
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
            plugin_base_dir,
            wasm_directory,
            native_directory,
            native_call_timeout,
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

    /// Unified native plugin loader.
    ///
    /// Discovers native plugins from two sources, loaded in priority order:
    ///
    /// 1. **Active records** (`.plugins/active/*.json`) — marketplace-installed
    ///    bundles whose entrypoints live under `.plugins/bundles/`.
    /// 2. **Directory bundles** (`.plugins/native/<id>/`) — local directory
    ///    layout where each subdirectory contains a `plugin.yml` manifest and
    ///    the plugin library.
    ///
    /// A plugin kind that was already loaded by an earlier source is skipped so
    /// that marketplace versions always take precedence.
    ///
    /// Errors reading plugin directories are logged but do not propagate;
    /// callers cannot distinguish "no plugins found" from "directory unreadable"
    /// except via the `warn!` log output.
    fn load_all_native_plugins(&mut self) -> Vec<PluginSummary> {
        let mut summaries = Vec::new();

        info!("Loading native plugins (unified)...");

        // Phase 1: active records (marketplace-installed bundles).
        summaries.extend(self.load_active_plugin_records());

        // Phase 2: directory bundles from the native directory.
        // Bare library files are warned about and skipped.
        // Plugins already loaded in phase 1 are automatically skipped
        // (load_native_plugin checks the map).
        summaries.extend(self.load_native_dir_plugins());

        summaries
    }

    /// Phase 1: load marketplace-managed plugins from active records.
    fn load_active_plugin_records(&mut self) -> Vec<PluginSummary> {
        let active_dir = plugin_active_dir(&self.plugin_base_dir);
        if !active_dir.exists() {
            return Vec::new();
        }

        let base_real = std::fs::canonicalize(&self.plugin_base_dir).ok();
        let entries = match std::fs::read_dir(&active_dir) {
            Ok(entries) => entries,
            Err(err) => {
                warn!(
                    error = %err,
                    dir = %active_dir.display(),
                    "Failed to read active plugins directory"
                );
                return Vec::new();
            },
        };

        let mut summaries = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    warn!(error = %err, "Failed to read active plugin entry");
                    continue;
                },
            };
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Some(summary) = self.load_active_plugin_record(&path, base_real.as_deref()) {
                info!(
                    plugin = %summary.kind,
                    source = "active-record",
                    "Loaded marketplace plugin"
                );
                summaries.push(summary);
            }
        }
        summaries
    }

    /// Phase 2: scan the native directory for directory bundles, loading
    /// plugins from subdirectories of `.plugins/native/<id>/`.
    fn load_native_dir_plugins(&mut self) -> Vec<PluginSummary> {
        let mut lib_paths: Vec<std::path::PathBuf> = Vec::new();

        match std::fs::read_dir(&self.native_directory) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        // Warn about bare library files that will be ignored.
                        if Self::is_native_lib(&path) {
                            warn!(
                                file = ?path,
                                "Bare plugin file found in native directory; \
                                 move it into a subdirectory (e.g. .plugins/native/<id>/)"
                            );
                        }
                        continue;
                    }
                    // Scan one level of subdirectories for plugin libraries.
                    if let Ok(sub_entries) = std::fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if Self::is_native_lib(&sub_path) {
                                lib_paths.push(sub_path);
                            }
                        }
                    }
                }
            },
            Err(err) => {
                warn!(
                    error = %err,
                    dir = %self.native_directory.display(),
                    "Failed to read native plugins directory"
                );
            },
        }

        // Sort for deterministic load order when multiple bundles provide
        // the same plugin kind.
        lib_paths.sort();

        // Skip any plugin whose kind is already loaded (from active records
        // or an earlier library in this phase).
        let mut summaries = Vec::new();
        for path in lib_paths {
            match self.load_native_plugin(&path) {
                Ok(summary) => {
                    info!(
                        plugin = %summary.kind,
                        file = ?path,
                        "Loaded native plugin from directory bundle"
                    );
                    summaries.push(summary);
                },
                Err(err) => {
                    // Uses the shared sentinel constants ERR_ALREADY_LOADED /
                    // ERR_ALREADY_REGISTERED produced by check_kind_conflict.
                    // A pre-check is not feasible because the plugin kind is
                    // only known after dlopen (LoadedNativePlugin::load).
                    let msg = err.to_string();
                    if msg.contains(ERR_ALREADY_LOADED) || msg.contains(ERR_ALREADY_REGISTERED) {
                        // Read both versions from manifests so operators can
                        // spot outdated marketplace installs vs patched locals.
                        let local_manifest = read_local_plugin_manifest(&path);
                        let local_version =
                            local_manifest.as_ref().map_or("unknown", |m| m.version.as_str());
                        let (loaded_kind, loaded_version) = local_manifest
                            .as_ref()
                            .and_then(|m| {
                                let kind =
                                    streamkit_plugin_native::namespaced_kind(&m.node_kind).ok()?;
                                let ver = self
                                    .plugins
                                    .get(&kind)
                                    .and_then(|p| p.version.as_deref())
                                    .unwrap_or("unknown");
                                Some((kind, ver.to_owned()))
                            })
                            .unwrap_or_else(|| ("unknown".into(), "unknown".into()));
                        warn!(
                            file = ?path,
                            plugin = %loaded_kind,
                            loaded_version = %loaded_version,
                            skipped_version = %local_version,
                            "Skipping local plugin (already loaded by higher-priority source)"
                        );
                    } else {
                        warn!(error = %err, file = ?path, "Failed to load native plugin from disk");
                    }
                },
            }
        }
        summaries
    }

    /// Returns `true` if the path looks like a native plugin library.
    fn is_native_lib(path: &std::path::Path) -> bool {
        let extension = path.extension().and_then(|ext| ext.to_str());
        matches!(extension, Some("so" | "dylib" | "dll")) && !path.to_string_lossy().ends_with(".d")
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

    fn load_active_plugin_record(
        &mut self,
        record_path: &Path,
        base_real: Option<&Path>,
    ) -> Option<PluginSummary> {
        let record = Self::read_active_record(record_path)?;
        if let Err(err) = plugin_paths::validate_path_component("plugin id", &record.plugin_id) {
            warn!(error = %err, file = ?record_path, "Invalid plugin id in active record");
            return None;
        }
        if let Err(err) = plugin_paths::validate_path_component("plugin version", &record.version) {
            warn!(error = %err, file = ?record_path, "Invalid plugin version in active record");
            return None;
        }
        let entrypoint_path = Self::validate_active_entrypoint(record_path, &record, base_real)?;

        let expected_kind = active_namespaced_kind(&record);
        let plugin_type = match record.kind {
            PluginKind::Wasm => PluginType::Wasm,
            PluginKind::Native => PluginType::Native,
        };

        let mut summary = match self.load_from_path(plugin_type, &entrypoint_path) {
            Ok(summary) => summary,
            Err(err) => {
                warn!(
                    error = %err,
                    kind = %expected_kind,
                    entrypoint = %entrypoint_path.display(),
                    "Failed to load active plugin"
                );
                return None;
            },
        };

        if summary.kind != expected_kind {
            let actual_kind = summary.kind;
            warn!(
                expected = %expected_kind,
                actual = %actual_kind,
                "Active plugin kind does not match record"
            );
            let _ = self.unload_plugin(&actual_kind, false);
            return None;
        }

        let version = record.version;
        if let Some(managed) = self.plugins.get_mut(&summary.kind) {
            managed.version = Some(version.clone());
        }
        summary.version = Some(version);

        Some(summary)
    }

    fn read_active_record(record_path: &Path) -> Option<crate::plugin_records::ActivePluginRecord> {
        let record_bytes = match std::fs::read(record_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(error = %err, file = ?record_path, "Failed to read active plugin record");
                return None;
            },
        };
        match serde_json::from_slice(&record_bytes) {
            Ok(record) => Some(record),
            Err(err) => {
                warn!(error = %err, file = ?record_path, "Failed to parse active plugin record");
                None
            },
        }
    }

    fn validate_active_entrypoint(
        record_path: &Path,
        record: &crate::plugin_records::ActivePluginRecord,
        base_real: Option<&Path>,
    ) -> Option<PathBuf> {
        let entrypoint_path = PathBuf::from(&record.entrypoint);
        if !entrypoint_path.exists() {
            warn!(
                file = ?record_path,
                entrypoint = %entrypoint_path.display(),
                "Active plugin entrypoint missing"
            );
            return None;
        }
        if let (Some(base_real), Ok(entrypoint_real)) =
            (base_real, std::fs::canonicalize(&entrypoint_path))
        {
            if !entrypoint_real.starts_with(base_real) {
                warn!(
                    file = ?record_path,
                    entrypoint = %entrypoint_real.display(),
                    "Active plugin entrypoint is outside plugin directory"
                );
                return None;
            }
        }

        Some(entrypoint_path)
    }

    /// Loads all existing plugins from both native and WASM directories.
    ///
    /// Native plugins (including marketplace-installed bundles) are loaded
    /// first via the unified loader, then WASM plugins.
    ///
    /// # Errors
    ///
    /// Returns an error if the WASM plugin directory cannot be read.
    /// Individual plugin load failures are logged but do not prevent other plugins from loading.
    pub fn load_existing(&mut self) -> Result<Vec<PluginSummary>> {
        let mut summaries = self.load_all_native_plugins();
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
        plugin_asset_registry: crate::plugin_assets::PluginAssetRegistry,
    ) {
        tokio::spawn(async move {
            info!("Starting background plugin loading");

            let result = match tokio::task::spawn_blocking({
                let manager = Arc::clone(&manager);
                move || {
                    let mut mgr = manager.blocking_lock();
                    let summaries = mgr.load_existing()?;
                    let asset_specs = mgr.collect_plugin_asset_specs();
                    drop(mgr);
                    Ok::<_, anyhow::Error>((summaries, asset_specs))
                }
            })
            .await
            {
                Ok(result) => result,
                Err(err) => {
                    warn!(error = %err, "Plugin load task panicked");
                    return;
                },
            };

            match result {
                Ok((summaries, asset_specs)) => {
                    if summaries.is_empty() {
                        info!("Background plugin loading completed (no plugins found)");
                    } else {
                        info!(
                            count = summaries.len(),
                            plugins = ?summaries.iter().map(|s| (s.kind.as_str(), s.plugin_type)).collect::<Vec<_>>(),
                            "Completed background plugin loading"
                        );
                    }

                    // Register plugin asset types
                    for (plugin_id, node_kind, specs) in &asset_specs {
                        plugin_asset_registry.register(plugin_id, node_kind, specs).await;
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

        let mut plugin = LoadedNativePlugin::load(path)
            .map_err(|e| {
                tracing::error!(error = %e, path = ?path, "Detailed native plugin load error");
                e
            })
            .with_context(|| format!("failed to load native plugin {}", path.to_string_lossy()))?;
        plugin.set_call_timeout(self.native_call_timeout);

        let metadata = plugin.metadata();
        let original_kind = metadata.kind.clone();
        let kind = streamkit_plugin_native::namespaced_kind(&original_kind)
            .with_context(|| format!("invalid plugin kind '{original_kind}'"))?;
        let categories = metadata.categories.clone();

        self.check_kind_conflict(&kind, &original_kind)?;

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

        let plugin_type = match managed.plugin_type {
            PluginType::Wasm => "wasm",
            PluginType::Native => "native",
        };

        self.plugin_operations_counter.add(
            1,
            &[KeyValue::new("operation", "unload"), KeyValue::new("plugin_type", plugin_type)],
        );
        self.update_loaded_gauge();

        // Capture file path before dropping managed, as we need to delete the file
        // AFTER the library is unloaded to avoid dlopen caching issues on reload.
        let file_path = managed.file_path.clone();
        let summary = PluginSummary::from_entry(kind.to_string(), &managed);

        // Explicitly drop managed to ensure the library (Arc<Library>) is fully
        // unloaded (dlclose called) BEFORE we delete the file. This prevents race
        // conditions where dlopen during reload might return a cached handle to
        // the old library if the file is deleted while the library is still loaded.
        drop(managed);

        if remove_file {
            if let Err(err) = std::fs::remove_file(&file_path) {
                warn!(
                    error = %err,
                    file = ?file_path,
                    "Failed to remove plugin file during unload"
                );
            }

            self.try_remove_empty_plugin_dir(&file_path);
        }

        Ok(summary)
    }

    /// Returns all loaded plugins as summaries.
    pub fn list_plugins(&self) -> Vec<PluginSummary> {
        self.plugins
            .iter()
            .map(|(kind, entry)| PluginSummary::from_entry(kind.clone(), entry))
            .collect()
    }

    /// Returns true if the plugin kind is currently loaded.
    pub fn is_plugin_loaded(&self, kind: &str) -> bool {
        self.plugins.contains_key(kind)
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

    /// Remove the parent directory of a plugin file if it is a now-empty
    /// subdirectory inside the native plugins dir (directory-bundle layout).
    fn try_remove_empty_plugin_dir(&self, file_path: &Path) {
        if let Some(parent) = file_path.parent() {
            if parent != self.native_directory && parent.starts_with(&self.native_directory) {
                let is_empty = parent.read_dir().is_ok_and(|mut entries| entries.next().is_none());
                if is_empty {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }

    /// Checks whether the given plugin kind conflicts with an already-loaded
    /// plugin or an already-registered node kind.
    ///
    /// Error messages intentionally contain `ERR_ALREADY_LOADED` /
    /// `ERR_ALREADY_REGISTERED` so that `load_native_dir_plugins` can
    /// distinguish expected dedup skips from genuine failures.
    fn check_kind_conflict(&self, kind: &str, original_kind: &str) -> Result<()> {
        if self.plugins.contains_key(kind) {
            return Err(anyhow!(
                "A plugin providing node '{original_kind}' (registered as '{kind}') is {ERR_ALREADY_LOADED}"
            ));
        }

        {
            let registry =
                self.engine.registry.read().map_err(|e| anyhow!("Registry lock poisoned: {e}"))?;
            if registry.contains(kind) {
                return Err(anyhow!(
                    "Node kind '{kind}' is {ERR_ALREADY_REGISTERED}; refusing to overwrite it with a plugin"
                ));
            }
        }

        Ok(())
    }

    /// Probes a native plugin library to discover its kind and checks for
    /// conflicts with already-loaded plugins, without registering anything.
    ///
    /// Used by the upload path to detect kind conflicts *before* moving the
    /// uploaded file to its final location, preventing accidental overwriting
    /// of an existing plugin's library.
    fn check_native_upload_conflict(&self, probe_path: &Path) -> Result<()> {
        // Set executable permissions so dlopen can load the probe file.
        // load_from_written_path sets permissions again on the final path
        // after the move — the two calls operate on different files.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(probe_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(probe_path, perms)?;
        }

        let plugin = LoadedNativePlugin::load(probe_path)
            .with_context(|| format!("failed to probe native plugin {}", probe_path.display()))?;
        let metadata = plugin.metadata();
        let original_kind = metadata.kind.clone();
        let kind = streamkit_plugin_native::namespaced_kind(&original_kind)
            .with_context(|| format!("invalid plugin kind '{original_kind}'"))?;
        // Close the library before the file is moved and re-opened from the
        // final path.
        drop(plugin);

        self.check_kind_conflict(&kind, &original_kind)
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
    #[allow(dead_code)] // Public API for non-streaming callers; HTTP handler uses load_from_temp_file.
    pub fn load_from_bytes(&mut self, file_name: &str, bytes: &[u8]) -> Result<PluginSummary> {
        let (target_path, plugin_type) = self.validate_plugin_upload_target(file_name)?;

        // Track whether we actually placed a file at target_path so the error
        // handler only cleans up files *we* created, not a pre-existing plugin.
        let mut file_placed = false;

        let result = (|| {
            if plugin_type == PluginType::Native {
                // Write to a temporary file first, probe for kind conflicts,
                // then move to the final location.  This mirrors the pattern
                // used by load_from_temp_file and prevents overwriting an
                // existing plugin's library when the kind is already loaded.
                let tmp_path = target_path.with_extension("tmp");
                std::fs::write(&tmp_path, bytes).with_context(|| {
                    format!("failed to write temp plugin file {}", tmp_path.display())
                })?;

                if let Err(e) = self.check_native_upload_conflict(&tmp_path) {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(e);
                }

                std::fs::rename(&tmp_path, &target_path).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    anyhow!(
                        "failed to move temp plugin file from {} to {}: {e}",
                        tmp_path.display(),
                        target_path.display()
                    )
                })?;
            } else {
                std::fs::write(&target_path, bytes).with_context(|| {
                    format!("failed to write plugin file {}", target_path.display())
                })?;
            }
            file_placed = true;

            self.load_from_written_path(plugin_type, target_path.clone())
        })();

        if result.is_err() {
            if file_placed {
                let _ = std::fs::remove_file(&target_path);
            }
            self.try_remove_empty_plugin_dir(&target_path);
        }
        result
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

        // Track whether we actually placed a file at target_path so the error
        // handler only cleans up files *we* created, not a pre-existing plugin.
        let mut file_placed = false;

        let result = (|| {
            let meta = std::fs::metadata(temp_path).with_context(|| {
                format!("failed to stat temp plugin file {}", temp_path.display())
            })?;
            if !meta.is_file() {
                return Err(anyhow!("temp plugin path is not a file: {}", temp_path.display()));
            }

            if plugin_type == PluginType::Native {
                // Move the temp file into the plugin subdirectory *before*
                // probing.  The original temp file may live on a noexec mount
                // (e.g. /tmp), which would cause dlopen to fail even though
                // .plugins/native/ is fine.  Using a .tmp extension keeps it
                // separate from the final target so a pre-existing plugin is
                // never overwritten until the probe passes.
                let probe_path = target_path.with_extension("tmp");
                if let Err(e) = std::fs::rename(temp_path, &probe_path) {
                    debug!(
                        error = %e,
                        from = %temp_path.display(),
                        to = %probe_path.display(),
                        "rename to probe path failed; falling back to copy+remove"
                    );
                    std::fs::copy(temp_path, &probe_path).with_context(|| {
                        format!(
                            "failed to copy temp plugin file from {} to {}",
                            temp_path.display(),
                            probe_path.display()
                        )
                    })?;
                    let _ = std::fs::remove_file(temp_path);
                }

                if let Err(e) = self.check_native_upload_conflict(&probe_path) {
                    let _ = std::fs::remove_file(&probe_path);
                    return Err(e);
                }

                std::fs::rename(&probe_path, &target_path).map_err(|e| {
                    let _ = std::fs::remove_file(&probe_path);
                    anyhow!(
                        "failed to move probe file from {} to {}: {e}",
                        probe_path.display(),
                        target_path.display()
                    )
                })?;
            } else {
                // WASM plugins don't need dlopen probing; move directly.
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
            }
            file_placed = true;

            self.load_from_written_path(plugin_type, target_path.clone())
        })();

        if result.is_err() {
            if file_placed {
                let _ = std::fs::remove_file(&target_path);
            }
            self.try_remove_empty_plugin_dir(&target_path);
        }
        result
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
                // Place native plugins inside a subdirectory derived from the
                // library stem (e.g. `libgain.so` → `native/gain/libgain.so`).
                let stem = Path::new(sanitized)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.strip_prefix("lib").unwrap_or(s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or(sanitized);
                if stem == "." || stem == ".." {
                    return Err(anyhow!(
                        "Cannot derive a valid plugin directory name from '{sanitized}'"
                    ));
                }
                let subdir = self.native_directory.join(stem);
                std::fs::create_dir_all(&subdir).with_context(|| {
                    format!("failed to create native plugin directory {}", subdir.display())
                })?;
                (subdir.join(sanitized), PluginType::Native)
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

    /// Loads a plugin from an existing on-disk path without moving it.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin fails to load.
    pub fn load_from_path<P: AsRef<Path>>(
        &mut self,
        plugin_type: PluginType,
        path: P,
    ) -> Result<PluginSummary> {
        self.load_from_written_path(plugin_type, path.as_ref().to_path_buf())
    }

    /// Collect asset type declarations from all loaded plugins.
    ///
    /// For each loaded plugin, attempts to read a `plugin.yml` manifest from the
    /// same directory as the plugin library.  Returns `(plugin_id, node_kind, specs)`
    /// tuples for plugins that declare asset types.
    pub fn collect_plugin_asset_specs(
        &self,
    ) -> Vec<(String, String, Vec<crate::marketplace::PluginAssetSpec>)> {
        let mut result = Vec::new();

        for (kind, managed) in &self.plugins {
            let manifest = crate::plugin_assets::read_local_plugin_manifest(&managed.file_path);
            if let Some(manifest) = manifest {
                if !manifest.assets.is_empty() {
                    result.push((manifest.id.clone(), kind.clone(), manifest.assets));
                }
            }
        }

        result
    }
}

/// Convenience alias for sharing the unified plugin manager behind an async mutex.
pub type SharedUnifiedPluginManager = Arc<Mutex<UnifiedPluginManager>>;
