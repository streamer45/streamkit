// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! plugin: WASM-based plugin system for StreamKit using the Component Model.
//!
//! This crate provides the host-side runtime for loading and executing WASM plugins.
//! Plugins are defined using WebAssembly Interface Types (WIT) and compiled to
//! WebAssembly components.

use anyhow::Result;
use bindings::streamkit::plugin::host::LogLevel;
use std::path::Path;
use std::sync::Arc;
use streamkit_core::{NodeRegistry, StreamKitError};
use tokio::sync::Mutex;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "plugin",
        imports: { default: async },
        exports: { default: async },
    });
}

use bindings::streamkit::plugin::host::Host;
pub use bindings::streamkit::plugin::types as wit_types;
use bindings::Plugin;

mod conversions;
mod wrapper;
pub use wrapper::WasmNodeWrapper;

/// Configuration for the WASM plugin runtime
#[derive(Debug, Clone)]
pub struct PluginRuntimeConfig {
    /// Maximum memory in bytes (default: 64MB)
    pub max_memory_bytes: usize,
    /// Enable WASM SIMD instructions
    pub enable_simd: bool,
    /// Enable multi-threading (experimental)
    pub enable_threads: bool,
}

impl Default for PluginRuntimeConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024, // 64MB
            enable_simd: true,
            enable_threads: false,
        }
    }
}

/// The WASM runtime engine for loading and managing plugins
pub struct PluginRuntime {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    #[allow(dead_code)] // Stored for potential future use
    config: PluginRuntimeConfig,
}

impl PluginRuntime {
    /// Create a new plugin runtime with the given configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the WASM engine or linker cannot be initialized
    pub fn new(config: PluginRuntimeConfig) -> Result<Self> {
        let mut engine_config = Config::new();
        engine_config.wasm_component_model(true);
        engine_config.async_support(true);
        engine_config.wasm_simd(config.enable_simd);
        engine_config.wasm_threads(config.enable_threads);

        let engine = Engine::new(&engine_config)?;
        let mut linker = Linker::new(&engine);

        // Add WASI p2 support
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        // Add our host functions (WASI imports already linked above)
        bindings::streamkit::plugin::host::add_to_linker::<HostState, HasSelf<_>>(
            &mut linker,
            |s| s,
        )?;

        Ok(Self { engine, linker: Arc::new(linker), config })
    }

    /// Load a single plugin from a WASM file
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be read or parsed as a valid WASM component
    /// - The component's metadata cannot be extracted
    pub fn load_plugin(&self, path: &Path) -> Result<LoadedPlugin> {
        let component = Component::from_file(&self.engine, path)
            .map_err(|e| anyhow::anyhow!("Failed to load component from file: {e:#}"))?;

        // Extract metadata by instantiating temporarily
        let metadata = self.extract_metadata(&component)?;

        tracing::info!(
            path = ?path,
            kind = %metadata.kind,
            "Loaded WASM plugin"
        );

        Ok(LoadedPlugin {
            component,
            metadata,
            engine: self.engine.clone(),
            linker: Arc::clone(&self.linker),
            max_memory_bytes: self.config.max_memory_bytes,
        })
    }

    /// Extract metadata from a component without running its main logic
    fn extract_metadata(&self, component: &Component) -> Result<wit_types::NodeMetadata> {
        // Create a temporary store and instance
        let wasi = WasiCtx::builder().build();
        let host_state = HostState {
            wasi,
            resource_table: ResourceTable::new(),
            output_sender: None,
            limits: StoreLimitsBuilder::new().memory_size(self.config.max_memory_bytes).build(),
        };
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|s| &mut s.limits);

        let instance =
            futures::executor::block_on(self.linker.instantiate_async(&mut store, component))?;
        let plugin = Plugin::new(&mut store, &instance)?;

        // Call the metadata function
        let node = plugin.streamkit_plugin_node();
        let metadata = futures::executor::block_on(node.call_metadata(&mut store))?;

        Ok(metadata)
    }

    /// Load all plugins from a directory
    pub fn load_plugins_from_directory(&self, dir: &Path) -> Vec<LoadedPlugin> {
        let mut plugins = Vec::new();

        if !dir.exists() {
            tracing::warn!(?dir, "Plugin directory does not exist");
            return plugins;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(?dir, error = %e, "Failed to read plugin directory");
                return plugins;
            },
        };

        // Process each entry, loading WASM files as plugins
        for entry in entries.flatten() {
            if let Some(plugin) = self.try_load_plugin_from_entry(&entry.path()) {
                plugins.push(plugin);
            }
        }

        plugins
    }

    /// Helper to load a plugin from a file path if it's a WASM file
    ///
    /// Returns `None` if the file is not a WASM file or fails to load.
    fn try_load_plugin_from_entry(&self, path: &Path) -> Option<LoadedPlugin> {
        // Only process .wasm files
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            return None;
        }

        match self.load_plugin(path) {
            Ok(plugin) => {
                tracing::info!(path = ?path, kind = %plugin.metadata.kind, "Loaded plugin");
                Some(plugin)
            },
            Err(e) => {
                tracing::error!(path = ?path, error = %e, "Failed to load plugin");
                None
            },
        }
    }
}

/// A loaded WASM plugin ready to create node instances
pub struct LoadedPlugin {
    component: Component,
    metadata: wit_types::NodeMetadata,
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    max_memory_bytes: usize,
}

impl LoadedPlugin {
    /// Get the metadata for this plugin
    ///
    /// This is a const fn since it only returns a reference to stored data
    pub const fn metadata(&self) -> &wit_types::NodeMetadata {
        &self.metadata
    }

    /// Create a new node instance from this plugin
    ///
    /// # Errors
    ///
    /// Returns an error if the node cannot be created with the provided parameters
    pub fn create_node(
        &self,
        params: Option<&serde_json::Value>,
    ) -> Result<Box<dyn streamkit_core::ProcessorNode>, StreamKitError> {
        let node = WasmNodeWrapper::new(
            self.component.clone(),
            self.metadata.clone(),
            params.cloned(),
            self.engine.clone(),
            Arc::clone(&self.linker),
            self.max_memory_bytes,
        );
        Ok(Box::new(node))
    }
}

/// Host state that is accessible to WASM plugins
pub struct HostState {
    wasi: WasiCtx,
    resource_table: ResourceTable,
    output_sender: Option<Arc<Mutex<streamkit_core::OutputSender>>>,
    limits: StoreLimits,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.resource_table }
    }
}

impl Host for HostState {
    async fn send_output(
        &mut self,
        pin_name: String,
        packet: wit_types::Packet,
    ) -> Result<(), String> {
        if let Some(sender) = &self.output_sender {
            let core_packet = streamkit_core::types::Packet::try_from(packet)?;
            // Tighten lock scope: acquire lock only for the send operation
            sender.lock().await.send(&pin_name, core_packet).await.map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err("Output sender not initialized".to_string())
        }
    }

    async fn log(&mut self, level: LogLevel, message: String) {
        match level {
            LogLevel::Debug => tracing::debug!("[Plugin] {}", message),
            LogLevel::Info => tracing::info!("[Plugin] {}", message),
            LogLevel::Warn => tracing::warn!("[Plugin] {}", message),
            LogLevel::Error => tracing::error!("[Plugin] {}", message),
        }
    }
}

// Implement the (empty) generated host trait for the `types` interface to satisfy the linker.
impl bindings::streamkit::plugin::types::Host for HostState {}

/// Prefix applied to all plugin-provided node kinds when registering with the engine.
pub const PLUGIN_KIND_PREFIX: &str = "plugin::wasm::";

/// Returns the canonical, namespaced kind for a plugin-provided node.
///
/// # Errors
///
/// Returns an error if:
/// - The original kind contains `::` (namespace separator is reserved)
/// - The original kind starts with reserved prefix `core::`
pub fn namespaced_kind(kind: &str) -> Result<String, String> {
    const RESERVED_PREFIX: &str = "core::";

    // Validate: reject if already has a namespace prefix
    if kind.starts_with(PLUGIN_KIND_PREFIX) {
        return Ok(kind.to_string());
    }

    // Validate: reject if contains namespace separator
    if kind.contains("::") {
        return Err(format!(
            "Plugin kind '{kind}' contains '::' which is reserved for namespace prefixes. \
             Plugin kinds must be simple names like 'gain', 'reverb', etc."
        ));
    }

    // Validate: reject if uses reserved prefix
    if kind.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "Plugin kind '{kind}' uses reserved prefix '{RESERVED_PREFIX}'. \
             This prefix is reserved for built-in core nodes."
        ));
    }

    Ok(format!("{PLUGIN_KIND_PREFIX}{kind}"))
}

/// Register all loaded plugins with a NodeRegistry.
///
/// Plugins with invalid kind names (e.g., containing reserved prefixes like `core::` or `plugin::`)
/// are skipped and logged as errors rather than causing a panic.
pub fn register_plugins(registry: &mut NodeRegistry, plugins: Vec<LoadedPlugin>) {
    for plugin in plugins {
        let metadata = plugin.metadata.clone();

        // Validate plugin kind and skip invalid plugins instead of panicking
        let kind = match namespaced_kind(&metadata.kind) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(
                    plugin_kind = %metadata.kind,
                    error = %e,
                    "Skipping WASM plugin with invalid kind name"
                );
                continue;
            },
        };

        // Convert WIT types to core types for registration
        // Use unwrap_or_else to avoid unnecessary function call on success path
        let param_schema: serde_json::Value =
            serde_json::from_str(&metadata.param_schema).unwrap_or_else(|_| serde_json::json!({}));

        let categories = metadata.categories.clone();

        // Create a factory that captures the plugin
        let plugin = Arc::new(plugin);
        registry.register_dynamic(
            &kind,
            move |params| plugin.create_node(params),
            param_schema,
            categories,
            false,
        );

        tracing::info!(
            kind = %kind,
            plugin_kind = %metadata.kind,
            "Registered WASM plugin node type"
        );
    }
}
