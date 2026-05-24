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

#[cfg(test)]
// Tests intentionally panic via unwrap/expect to surface broken preconditions
// directly rather than propagating through `?`.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    fn make_manager(tmp: &TempDir) -> UnifiedPluginManager {
        let base = tmp.path().to_path_buf();
        let wasm = base.join("wasm");
        let native = base.join("native");
        let resource_manager =
            Arc::new(streamkit_core::ResourceManager::new(streamkit_core::ResourcePolicy {
                keep_loaded: true,
                max_memory_mb: None,
            }));
        let engine =
            Arc::new(streamkit_engine::Engine::with_resource_manager(resource_manager.clone()));
        UnifiedPluginManager::new(engine, resource_manager, base, wasm, native, None)
            .expect("manager builds from a fresh tmpdir")
    }

    /// Returns the cached `.so` path, panicking in CI when the fixture
    /// is unavailable so silent skips can't inflate reported coverage.
    /// Outside CI a missing fixture only logs a `tracing::warn!`.
    fn panicking_plugin_so_or_skip() -> Option<PathBuf> {
        let so = panicking_plugin_so_raw();
        if so.is_none() {
            let test_name = std::thread::current().name().unwrap_or("<unknown>").to_string();
            assert!(
                std::env::var_os("CI").is_none(),
                "panicking-plugin fixture missing in CI; test `{test_name}` would silently \
                 skip. The streamkit-plugin-native build script normally produces it in \
                 target/debug/build/streamkit-plugin-native-*/out/.",
            );
            tracing::warn!("skipping `{test_name}`: panicking-plugin .so not found");
        }
        so
    }

    /// Locates (or builds, once per process) the panicking-plugin `.so`
    /// that streamkit-plugin-native's build script produces in its OUT_DIR.
    /// We can't read that OUT_DIR from outside the crate, so we either
    /// reuse an existing build artefact or invoke `cargo build` on demand.
    fn panicking_plugin_so_raw() -> Option<PathBuf> {
        static CACHED: OnceLock<Option<PathBuf>> = OnceLock::new();
        CACHED
            .get_or_init(|| {
                // Walk the workspace target/ for the build script's output.
                // The hash in the path is the build script's fingerprint;
                // there can be multiple (different feature combos), so pick
                // the first matching .so.
                let workspace_root = workspace_root()?;
                if let Some(path) = find_existing_so(&workspace_root.join("target")) {
                    return Some(path);
                }
                // Fall back to building the fixture directly. The fixture
                // crate is a tiny cdylib with no heavy dependencies; in a
                // warm cargo cache this takes ~1-2s.
                let fixture_manifest = workspace_root
                    .join("crates/plugin-native/tests/fixtures/panicking-plugin/Cargo.toml");
                if !fixture_manifest.exists() {
                    return None;
                }
                let target_dir =
                    workspace_root.join("target/skit-plugin-tests/panicking-plugin-target");
                let status = Command::new(option_env!("CARGO").unwrap_or("cargo"))
                    .args(["build", "--manifest-path"])
                    .arg(&fixture_manifest)
                    .arg("--target-dir")
                    .arg(&target_dir)
                    .status()
                    .ok()?;
                if !status.success() {
                    return None;
                }
                let so = target_dir.join("debug").join("libpanicking_plugin.so");
                so.exists().then_some(so)
            })
            .clone()
    }

    fn workspace_root() -> Option<PathBuf> {
        // CARGO_MANIFEST_DIR points at apps/skit; the workspace root is two
        // levels up.
        let manifest_dir = option_env!("CARGO_MANIFEST_DIR")?;
        let p = Path::new(manifest_dir).parent()?.parent()?.to_path_buf();
        Some(p)
    }

    fn find_existing_so(target_dir: &Path) -> Option<PathBuf> {
        // The build script's output lives at
        //   {target}/debug/build/streamkit-plugin-native-*/out/...
        // Under coverage the effective target/ is target/coverage/, so try
        // every immediate subdirectory of target/ that contains a debug/build/
        // folder (covers both the plain layout and CARGO_TARGET_DIR overrides).
        let direct = target_dir.join("debug/build");
        if let Some(hit) = scan_build_dir(&direct) {
            return Some(hit);
        }
        let entries = std::fs::read_dir(target_dir).ok()?;
        for entry in entries.flatten() {
            let candidate_root = entry.path().join("debug/build");
            if let Some(hit) = scan_build_dir(&candidate_root) {
                return Some(hit);
            }
        }
        None
    }

    fn scan_build_dir(build_dir: &Path) -> Option<PathBuf> {
        let entries = std::fs::read_dir(build_dir).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("streamkit-plugin-native-") {
                continue;
            }
            let candidate =
                entry.path().join("out/panicking-plugin-target/debug/libpanicking_plugin.so");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    #[test]
    fn list_plugins_returns_empty_on_a_fresh_manager() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);
        assert!(mgr.list_plugins().is_empty());
        assert!(!mgr.is_plugin_loaded("plugin::native::panicking"));
    }

    #[test]
    fn new_creates_missing_wasm_and_native_directories() {
        // Manager::new must create both plugin subdirs even when only the
        // base dir exists; load_existing would otherwise fail on the first
        // read_dir call.
        let tmp = TempDir::new().unwrap();
        let _mgr = make_manager(&tmp);
        assert!(tmp.path().join("wasm").is_dir());
        assert!(tmp.path().join("native").is_dir());
    }

    #[test]
    fn unload_plugin_returns_not_loaded_error_for_unknown_kind() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let err = mgr
            .unload_plugin("plugin::native::nope", false)
            .expect_err("unloading an unknown plugin must fail");
        let msg = err.to_string();
        assert!(msg.contains("not currently loaded"), "unexpected error: {msg}");
    }

    #[test]
    fn load_from_path_returns_error_when_file_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bogus = tmp.path().join("does_not_exist.so");
        let err = mgr
            .load_from_path(PluginType::Native, &bogus)
            .expect_err("non-existent native plugin path must fail");
        // The error path is either the explicit `does not exist` from
        // load_native_plugin or the OS-level `No such file or directory`
        // raised by the chmod step in load_from_written_path on Unix.
        // Both indicate the missing file -- pin that we don't silently
        // succeed.
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist") || msg.to_lowercase().contains("no such file"),
            "unexpected native error: {msg}",
        );

        let bogus_wasm = tmp.path().join("does_not_exist.wasm");
        let err = mgr
            .load_from_path(PluginType::Wasm, &bogus_wasm)
            .expect_err("non-existent WASM plugin path must fail");
        assert!(
            err.to_string().contains("failed to compile WASM plugin")
                || err.to_string().contains("does not exist"),
            "unexpected WASM error: {err}"
        );
    }

    #[test]
    fn load_from_bytes_rejects_empty_file_name() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let err = mgr.load_from_bytes("", b"junk").expect_err("empty name must fail");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn load_from_bytes_rejects_path_traversal_in_file_name() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let err = mgr
            .load_from_bytes("../escape.wasm", b"")
            .expect_err("path-traversal must be rejected");
        assert!(err.to_string().contains("plain file name"), "got {err}");
    }

    #[test]
    fn load_from_bytes_rejects_unsupported_extension() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let err =
            mgr.load_from_bytes("evil.txt", b"").expect_err("unknown extension must be rejected");
        assert!(err.to_string().contains("valid extension"), "got {err}");
    }

    #[test]
    fn load_from_bytes_rejects_oversize_file_name() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let long = format!("{}.wasm", "a".repeat(300));
        let err = mgr.load_from_bytes(&long, b"").expect_err("oversize name must fail");
        assert!(err.to_string().contains("too long"), "got {err}");
    }

    #[test]
    fn load_from_bytes_cleans_up_temp_file_on_invalid_wasm_payload() {
        // Writing garbage to a .wasm file passes the upload-path validation
        // but trips the runtime compile -- the helper must remove the
        // partial file so a retry with the same name isn't blocked.
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let _ = mgr.load_from_bytes("garbage.wasm", b"not a wasm module");
        let target = tmp.path().join("wasm").join("garbage.wasm");
        assert!(!target.exists(), "partial wasm file must be cleaned up");
    }

    #[test]
    fn load_from_temp_file_rejects_invalid_extension() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let payload = tmp.path().join("payload");
        std::fs::write(&payload, b"x").unwrap();
        let err = mgr
            .load_from_temp_file("evil.exe", &payload)
            .expect_err("unsupported extension must be rejected");
        assert!(err.to_string().contains("valid extension"), "got {err}");
    }

    #[test]
    fn load_from_temp_file_rejects_missing_temp_file() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bogus = tmp.path().join("missing");
        let err = mgr
            .load_from_temp_file("ok.wasm", &bogus)
            .expect_err("missing temp file must be rejected");
        assert!(
            err.to_string().contains("failed to stat") || err.to_string().contains("not a file"),
            "got {err}",
        );
    }

    #[test]
    fn collect_plugin_asset_specs_returns_empty_with_no_loaded_plugins() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);
        assert!(mgr.collect_plugin_asset_specs().is_empty());
    }

    #[test]
    fn load_existing_returns_empty_when_no_plugins_present() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let summaries = mgr.load_existing().expect("load_existing on empty dirs is Ok");
        assert!(summaries.is_empty());
    }

    #[test]
    fn load_native_directory_warns_and_skips_bare_so_files() {
        // A bare .so directly in native_directory (not in a subdir) must be
        // ignored: load_existing must still return Ok, and the file must not
        // be loaded.
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bare_so = tmp.path().join("native").join("dropped.so");
        std::fs::write(&bare_so, b"not a real library").unwrap();

        let summaries = mgr.load_existing().expect("bare library must not be a hard failure");
        assert!(summaries.is_empty(), "bare .so must not register, got {summaries:?}");
        assert!(mgr.list_plugins().is_empty());
    }

    fn write_active_record(plugin_dir: &Path, record_file: &str, record_json: &str) -> PathBuf {
        let active = plugin_dir.join("active");
        std::fs::create_dir_all(&active).unwrap();
        let path = active.join(record_file);
        std::fs::write(&path, record_json).unwrap();
        path
    }

    #[test]
    fn load_active_plugin_record_skips_when_entrypoint_missing() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let record = serde_json::json!({
            "plugin_id": "ghost",
            "version": "0.1.0",
            "node_kind": "ghost",
            "kind": "native",
            "entrypoint": tmp.path().join("nope/libghost.so").display().to_string(),
            "installed_at_ms": 0u64,
        });
        let _record_path = write_active_record(tmp.path(), "ghost.json", &record.to_string());

        let summaries = mgr.load_existing().expect("missing entrypoint is logged, not propagated");
        assert!(
            summaries.is_empty(),
            "active record with missing entrypoint must not register, got {summaries:?}",
        );
    }

    #[test]
    fn load_active_plugin_record_skips_invalid_plugin_id() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let record = serde_json::json!({
            "plugin_id": "../escape",
            "version": "0.1.0",
            "node_kind": "ghost",
            "kind": "native",
            "entrypoint": "/dev/null",
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "bad.json", &record.to_string());
        let summaries = mgr.load_existing().expect("invalid id is logged, not propagated");
        assert!(summaries.is_empty());
    }

    #[test]
    fn load_active_plugin_record_skips_malformed_json() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        write_active_record(tmp.path(), "broken.json", "not json at all");
        let summaries = mgr.load_existing().expect("malformed record is logged, not propagated");
        assert!(summaries.is_empty());
    }

    #[test]
    fn load_native_plugin_happy_path_registers_then_dedups_then_unloads() {
        // This test exercises the only "real plugin load" path we can reach
        // from apps/skit: the panicking-plugin .so produced by
        // streamkit-plugin-native's build.rs.  When that artefact isn't
        // available (e.g. minimal build, sandbox without cargo, etc.) we
        // skip rather than fail -- the validation-path tests above still
        // cover everything else.
        let Some(so) = panicking_plugin_so_or_skip() else { return };

        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        // Install via load_from_path so we don't disturb the .so location.
        let summary = mgr
            .load_from_path(PluginType::Native, &so)
            .expect("happy-path load of panicking-plugin");

        assert_eq!(summary.plugin_type, PluginType::Native);
        assert_eq!(summary.original_kind, "panicking");
        assert_eq!(summary.kind, "plugin::native::panicking");
        assert!(mgr.is_plugin_loaded("plugin::native::panicking"));

        // list_plugins now contains the entry.
        let listed = mgr.list_plugins();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "plugin::native::panicking");

        // A second load with the same kind must trip check_kind_conflict
        // (or the wasm-side equivalent) and surface ERR_ALREADY_LOADED.
        let dup = mgr
            .load_from_path(PluginType::Native, &so)
            .expect_err("duplicate kind must be rejected");
        let msg = dup.to_string();
        assert!(
            msg.contains(ERR_ALREADY_LOADED) || msg.contains(ERR_ALREADY_REGISTERED),
            "duplicate-load error must carry the sentinel: {msg}",
        );

        // Unload round-trip.
        let removed = mgr
            .unload_plugin("plugin::native::panicking", false)
            .expect("unload of a freshly-loaded plugin must succeed");
        assert_eq!(removed.kind, "plugin::native::panicking");
        assert!(!mgr.is_plugin_loaded("plugin::native::panicking"));
        assert!(mgr.list_plugins().is_empty());

        // Second unload must report not-loaded.
        let err = mgr
            .unload_plugin("plugin::native::panicking", false)
            .expect_err("second unload must fail");
        assert!(err.to_string().contains("not currently loaded"));
    }

    #[test]
    fn list_plugins_includes_loaded_native_after_load_existing() {
        // Drop the fixture .so into a subdirectory of native_directory so
        // load_native_dir_plugins picks it up via the directory-bundle path
        // (phase 2 of load_all_native_plugins).
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bundle_dir = tmp.path().join("native").join("panicking");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let dest = bundle_dir.join("libpanicking_plugin.so");
        std::fs::copy(&so, &dest).unwrap();

        let summaries = mgr.load_existing().expect("load_existing succeeds with one bundle");
        assert_eq!(summaries.len(), 1, "expected one loaded plugin, got {summaries:?}");
        assert_eq!(summaries[0].kind, "plugin::native::panicking");
        assert!(mgr.is_plugin_loaded("plugin::native::panicking"));
    }

    #[test]
    fn load_existing_skips_directory_bundle_when_kind_already_loaded() {
        // Two bundles providing the same plugin kind: the first wins; the
        // second must be skipped via the ERR_ALREADY_LOADED sentinel branch
        // in load_native_dir_plugins.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        // bundle-a is alphabetically earlier and wins.
        for sub in ["bundle-a", "bundle-b"] {
            let dir = tmp.path().join("native").join(sub);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(&so, dir.join("libpanicking_plugin.so")).unwrap();
        }
        let summaries = mgr.load_existing().expect("load_existing succeeds");
        assert_eq!(summaries.len(), 1, "duplicate bundle must be deduplicated, got {summaries:?}");
        assert_eq!(mgr.list_plugins().len(), 1);
    }

    #[test]
    fn load_from_temp_file_native_happy_path_moves_and_loads() {
        // Exercise the move-probe-rename path in load_from_temp_file for a
        // real native plugin: temp file disappears, target file appears,
        // plugin is registered.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);

        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let temp_so = staging.join("libpanicking_plugin.so");
        std::fs::copy(&so, &temp_so).unwrap();

        let summary = mgr
            .load_from_temp_file("libpanicking_plugin.so", &temp_so)
            .expect("native plugin uploaded via temp file must load");

        assert_eq!(summary.kind, "plugin::native::panicking");
        assert!(!temp_so.exists(), "temp file must be moved into the plugin tree");
        let target = tmp.path().join("native/panicking_plugin/libpanicking_plugin.so");
        assert!(target.exists(), "plugin must land at {}", target.display());
        assert!(mgr.is_plugin_loaded("plugin::native::panicking"));
    }

    #[test]
    fn load_from_temp_file_detects_existing_kind_and_keeps_original_file() {
        // After a successful load via temp file, a second upload of the
        // *same* plugin must be rejected without clobbering the existing
        // file (check_native_upload_conflict path).
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);

        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let temp_so_1 = staging.join("a.so");
        std::fs::copy(&so, &temp_so_1).unwrap();
        mgr.load_from_temp_file("first.so", &temp_so_1).expect("first load");

        let temp_so_2 = staging.join("b.so");
        std::fs::copy(&so, &temp_so_2).unwrap();
        let err = mgr
            .load_from_temp_file("second.so", &temp_so_2)
            .expect_err("duplicate upload must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains(ERR_ALREADY_LOADED) || msg.contains(ERR_ALREADY_REGISTERED),
            "expected dedup sentinel, got: {msg}",
        );
        // The first plugin file must still be there.
        let first = tmp.path().join("native/first/first.so");
        assert!(first.exists(), "original file at {} must survive", first.display());
    }

    #[test]
    fn load_from_bytes_native_happy_path_writes_and_loads() {
        // load_from_bytes for a native plugin should write the bytes,
        // probe for conflicts, then register the plugin.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bytes = std::fs::read(&so).unwrap();

        let summary = mgr
            .load_from_bytes("libpanicking_plugin.so", &bytes)
            .expect("native plugin uploaded via bytes must load");
        assert_eq!(summary.kind, "plugin::native::panicking");
        let target = tmp.path().join("native/panicking_plugin/libpanicking_plugin.so");
        assert!(target.exists(), "plugin must land at {}", target.display());
    }

    #[test]
    fn load_from_bytes_native_dedup_keeps_existing_file_and_cleans_tmp() {
        // After a successful load_from_bytes, a second call with the same
        // payload must be rejected and must not leave the .tmp probe file
        // behind.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bytes = std::fs::read(&so).unwrap();
        mgr.load_from_bytes("a.so", &bytes).expect("first load");

        let err =
            mgr.load_from_bytes("b.so", &bytes).expect_err("duplicate upload must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains(ERR_ALREADY_LOADED) || msg.contains(ERR_ALREADY_REGISTERED),
            "expected dedup sentinel, got: {msg}",
        );
        let tmp_probe = tmp.path().join("native/b/b.tmp");
        assert!(!tmp_probe.exists(), "probe tmp file must be cleaned up");
    }

    #[test]
    fn unload_with_remove_file_deletes_library_and_empty_parent_dir() {
        // unload_plugin(_, remove_file=true) must delete the .so file and,
        // if the parent is now an empty subdir of native_directory, remove
        // it too (try_remove_empty_plugin_dir path).
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bundle_dir = tmp.path().join("native").join("panicking-bundle");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let dest = bundle_dir.join("libpanicking_plugin.so");
        std::fs::copy(&so, &dest).unwrap();

        mgr.load_existing().expect("load_existing succeeds");
        assert!(mgr.is_plugin_loaded("plugin::native::panicking"));

        mgr.unload_plugin("plugin::native::panicking", true).expect("unload with delete");

        assert!(!dest.exists(), "plugin file must be removed");
        assert!(!bundle_dir.exists(), "empty bundle dir must be removed");
    }

    #[test]
    fn load_active_plugin_record_skips_when_entrypoint_outside_base_dir() {
        // Use the real fixture under /tmp so canonicalize() succeeds. The
        // active record points outside the plugin_base_dir and must be
        // rejected with a warning rather than registered.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        // Place the .so outside plugin_base_dir.
        let outside = tempfile::tempdir().unwrap();
        let outside_so = outside.path().join("libpanicking_plugin.so");
        std::fs::copy(&so, &outside_so).unwrap();

        let record = serde_json::json!({
            "plugin_id": "panicking",
            "version": "0.1.0",
            "node_kind": "panicking",
            "kind": "native",
            "entrypoint": outside_so.display().to_string(),
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "panicking.json", &record.to_string());

        let summaries = mgr.load_existing().expect("load_existing returns Ok");
        assert!(
            summaries.is_empty(),
            "entrypoint outside base dir must be skipped, got {summaries:?}",
        );
    }

    #[test]
    fn load_active_plugin_record_happy_path_registers_with_version() {
        // Full active-record path: entrypoint exists under plugin_base_dir
        // and the record's node_kind matches what the .so reports. Plugin
        // is registered with the version from the record.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bundle_dir = tmp.path().join("bundles/panicking-0.1.0");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let entrypoint = bundle_dir.join("libpanicking_plugin.so");
        std::fs::copy(&so, &entrypoint).unwrap();

        let record = serde_json::json!({
            "plugin_id": "panicking",
            "version": "0.1.0",
            "node_kind": "panicking",
            "kind": "native",
            "entrypoint": entrypoint.display().to_string(),
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "panicking.json", &record.to_string());

        let summaries = mgr.load_existing().expect("load_existing succeeds");
        assert_eq!(summaries.len(), 1, "active record must register, got {summaries:?}");
        let s = &summaries[0];
        assert_eq!(s.kind, "plugin::native::panicking");
        assert_eq!(s.version.as_deref(), Some("0.1.0"));
        assert!(mgr.is_plugin_loaded("plugin::native::panicking"));
    }

    #[test]
    fn load_active_plugin_record_skips_on_invalid_version() {
        // validate_path_component should reject versions with path separators,
        // so the record is logged and skipped.
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let record = serde_json::json!({
            "plugin_id": "ok",
            "version": "../etc",
            "node_kind": "ok",
            "kind": "native",
            "entrypoint": "/tmp/missing.so",
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "bad-version.json", &record.to_string());
        let summaries = mgr.load_existing().expect("invalid version is logged, not propagated");
        assert!(summaries.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_load_existing_loads_bundle_with_prewarm_failure_logged() {
        // spawn_load_existing kicks off background loading; the prewarm
        // path tries `prewarm_engine_plugin` first with primary params,
        // then with fallback_params if provided. The panicking-plugin
        // fixture intentionally panics on `process()` (which is what
        // create_node calls indirectly), so prewarm will fail — we want
        // to verify the background task still completes cleanly.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mgr = {
            let m = make_manager(&tmp);
            Arc::new(Mutex::new(m))
        };
        let bundle_dir = tmp.path().join("native").join("panicking");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        std::fs::copy(&so, bundle_dir.join("libpanicking_plugin.so")).unwrap();

        let prewarm = crate::config::PrewarmConfig {
            enabled: true,
            plugins: vec![crate::config::PrewarmPluginConfig {
                kind: "plugin::native::panicking".to_string(),
                params: Some(serde_json::json!({"primary": true})),
                fallback_params: Some(serde_json::json!({"fallback": true})),
            }],
        };
        let registry = crate::plugin_assets::PluginAssetRegistry::new();

        UnifiedPluginManager::spawn_load_existing(Arc::clone(&mgr), prewarm, registry);

        // Wait for the spawned task to load the plugin. Poll for up to a
        // few seconds — the actual load is fast (single .so) but the task
        // includes a prewarm attempt that may take a moment to fail.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let loaded = {
                let guard = mgr.lock().await;
                guard.is_plugin_loaded("plugin::native::panicking")
            };
            if loaded {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "spawn_load_existing failed to register the plugin in time",
            );
        }
        // Give the prewarm path (primary + fallback) time to run.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_load_existing_no_op_when_no_plugins_found() {
        // No plugins on disk -> spawn_load_existing must finish without
        // panicking and leave the manager with zero plugins.
        let tmp = TempDir::new().unwrap();
        let mgr = Arc::new(Mutex::new(make_manager(&tmp)));
        let prewarm = crate::config::PrewarmConfig::default();
        let registry = crate::plugin_assets::PluginAssetRegistry::new();

        UnifiedPluginManager::spawn_load_existing(Arc::clone(&mgr), prewarm, registry);
        // Give the task a moment to run.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let is_empty = {
            let guard = mgr.lock().await;
            guard.list_plugins().is_empty()
        };
        assert!(is_empty);
    }

    #[test]
    fn load_active_plugin_record_skips_when_kind_does_not_match_record() {
        // The fixture reports node_kind `panicking`, but we declare the
        // record's node_kind as something else. After load_from_path
        // succeeds, the kind mismatch must trigger an unload + skip.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bundle_dir = tmp.path().join("bundles/mismatched-0.1.0");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let entrypoint = bundle_dir.join("libpanicking_plugin.so");
        std::fs::copy(&so, &entrypoint).unwrap();

        let record = serde_json::json!({
            "plugin_id": "mismatched",
            "version": "0.1.0",
            "node_kind": "something_else",
            "kind": "native",
            "entrypoint": entrypoint.display().to_string(),
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "mismatched.json", &record.to_string());

        let summaries = mgr.load_existing().expect("load_existing returns Ok");
        assert!(summaries.is_empty(), "kind mismatch must trigger skip, got {summaries:?}");
        assert!(mgr.list_plugins().is_empty(), "the temporarily-loaded plugin must be unloaded");
    }

    #[test]
    fn load_existing_warns_and_logs_version_when_active_record_dedupes_directory_bundle() {
        // First the active record loads the panicking plugin. Then a
        // directory bundle also providing the same kind is encountered
        // in phase 2 — this exercises the ERR_ALREADY_LOADED log branch
        // which also reads the bundle's plugin.yml for version reporting.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);

        // Active record load (phase 1).
        let bundle1 = tmp.path().join("bundles/panicking-1.0.0");
        std::fs::create_dir_all(&bundle1).unwrap();
        let entry1 = bundle1.join("libpanicking_plugin.so");
        std::fs::copy(&so, &entry1).unwrap();
        let record = serde_json::json!({
            "plugin_id": "panicking",
            "version": "1.0.0",
            "node_kind": "panicking",
            "kind": "native",
            "entrypoint": entry1.display().to_string(),
            "installed_at_ms": 0u64,
        });
        write_active_record(tmp.path(), "panicking.json", &record.to_string());

        // Directory bundle (phase 2) with manifest declaring an older version.
        let bundle2 = tmp.path().join("native/panicking-old");
        std::fs::create_dir_all(&bundle2).unwrap();
        std::fs::copy(&so, bundle2.join("libpanicking_plugin.so")).unwrap();
        std::fs::write(
            bundle2.join("plugin.yml"),
            "id: panicking\nversion: 0.1.0\nnode_kind: panicking\nkind: native\nentrypoint: libpanicking_plugin.so\n",
        )
        .unwrap();

        let summaries = mgr.load_existing().expect("load_existing returns Ok");
        assert_eq!(summaries.len(), 1, "active record wins, got {summaries:?}");
        assert_eq!(summaries[0].version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn load_wasm_directory_skips_non_wasm_files_silently() {
        // load_wasm_plugins_from_dir lists the wasm directory and filters
        // entries by extension; non-.wasm files must be skipped without
        // surfacing an error.
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        std::fs::write(tmp.path().join("wasm").join("notes.txt"), "ignored").unwrap();
        let summaries = mgr.load_existing().expect("non-wasm files must be skipped quietly");
        assert!(summaries.is_empty(), "no wasm plugins to load, got {summaries:?}");
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn load_wasm_directory_logs_and_continues_on_invalid_wasm_payload() {
        // An invalid .wasm file in the wasm directory must cause
        // load_wasm_plugin to fail; the loop should log the error and
        // proceed without aborting load_existing.
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        std::fs::write(tmp.path().join("wasm").join("garbage.wasm"), b"not valid wasm").unwrap();
        let summaries = mgr.load_existing().expect("invalid wasm must not abort load_existing");
        assert!(summaries.is_empty(), "no successful loads, got {summaries:?}");
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn collect_plugin_asset_specs_includes_assets_from_loaded_plugin_manifest() {
        // After loading a native plugin, drop a plugin.yml next to the .so
        // declaring an asset spec. collect_plugin_asset_specs must include it.
        let Some(so) = panicking_plugin_so_or_skip() else { return };
        let tmp = TempDir::new().unwrap();
        let mut mgr = make_manager(&tmp);
        let bundle_dir = tmp.path().join("native").join("panicking");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        std::fs::copy(&so, bundle_dir.join("libpanicking_plugin.so")).unwrap();
        let manifest = r#"
id: panicking
version: 0.1.0
node_kind: panicking
kind: native
entrypoint: libpanicking_plugin.so
assets:
  - type_id: model_file
    label: "Model File"
    extensions: [bin]
"#;
        std::fs::write(bundle_dir.join("plugin.yml"), manifest).unwrap();

        mgr.load_existing().expect("load_existing succeeds");
        let specs = mgr.collect_plugin_asset_specs();
        assert_eq!(specs.len(), 1, "manifest with assets must yield one entry: {specs:?}");
        let (plugin_id, node_kind, asset_specs) = &specs[0];
        assert_eq!(plugin_id, "panicking");
        assert_eq!(node_kind, "plugin::native::panicking");
        assert_eq!(asset_specs.len(), 1);
        assert_eq!(asset_specs[0].type_id, "model_file");
    }
}
