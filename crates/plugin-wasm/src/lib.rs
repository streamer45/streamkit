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
        engine_config.wasm_simd(config.enable_simd);
        engine_config.wasm_threads(config.enable_threads);

        let engine = Engine::new(&engine_config)?;
        let mut linker = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
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

    fn extract_metadata(&self, component: &Component) -> Result<wit_types::NodeMetadata> {
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

    #[cfg(test)]
    pub(crate) fn engine_for_test(&self) -> Engine {
        self.engine.clone()
    }

    #[cfg(test)]
    pub(crate) fn linker_for_test(&self) -> Arc<Linker<HostState>> {
        Arc::clone(&self.linker)
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

#[cfg(test)]
// Tests rely on expect/unwrap to fail fast with readable assertion context.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plugin_runtime_config_default_values() {
        let cfg = PluginRuntimeConfig::default();
        assert_eq!(cfg.max_memory_bytes, 64 * 1024 * 1024);
        assert!(cfg.enable_simd);
        assert!(!cfg.enable_threads);
    }

    #[test]
    fn plugin_runtime_new_builds_with_default_config() {
        // Smoke test: the wasmtime engine + linker should set up cleanly with default config.
        let _runtime = PluginRuntime::new(PluginRuntimeConfig::default())
            .expect("runtime must initialize with default config");
    }

    #[test]
    fn plugin_runtime_new_honors_custom_memory_limit() {
        let cfg = PluginRuntimeConfig {
            max_memory_bytes: 16 * 1024 * 1024,
            enable_simd: true,
            enable_threads: false,
        };
        let _runtime =
            PluginRuntime::new(cfg).expect("runtime must initialize with custom memory limit");
    }

    #[test]
    fn plugin_runtime_new_rejects_disabled_simd_due_to_relaxed_simd_default() {
        // BUG (tracked in #469): PluginRuntimeConfig exposes `enable_simd: false`, but
        // wasmtime enables the relaxed-simd proposal by default, which requires the base
        // SIMD proposal. The resulting config error ("cannot disable the simd proposal but
        // enable the relaxed simd proposal") means every `enable_simd: false` config —
        // including the threads combination — fails initialization. Pin current (broken)
        // behavior until fixed.
        for enable_threads in [false, true] {
            let cfg = PluginRuntimeConfig {
                max_memory_bytes: 16 * 1024 * 1024,
                enable_simd: false,
                enable_threads,
            };
            assert!(
                PluginRuntime::new(cfg).is_err(),
                "expected init to fail when enable_simd=false (enable_threads={enable_threads})"
            );
        }
    }

    #[test]
    fn namespaced_kind_adds_prefix_to_simple_name() {
        assert_eq!(namespaced_kind("gain").as_deref(), Ok("plugin::wasm::gain"));
        assert_eq!(namespaced_kind("reverb").as_deref(), Ok("plugin::wasm::reverb"));
    }

    #[test]
    fn namespaced_kind_is_idempotent_for_already_prefixed_kinds() {
        let already = format!("{PLUGIN_KIND_PREFIX}gain");
        assert_eq!(namespaced_kind(&already), Ok(already.clone()));
    }

    #[test]
    fn namespaced_kind_rejects_kinds_containing_namespace_separator() {
        let err = namespaced_kind("foo::bar").expect_err("must reject `::` in kind");
        assert!(err.contains("reserved for namespace prefixes"));
    }

    #[test]
    fn namespaced_kind_rejects_reserved_core_prefix() {
        let err = namespaced_kind("core::audio").expect_err("must reject `core::` kinds");
        // BUG (tracked in #470): the `core::` check is unreachable because any string
        // containing `::` is already rejected by the namespace-separator guard. Pin the
        // current behavior — the error message references the namespace-separator rule,
        // not the reserved-prefix rule — so the test fails if the contract is revisited.
        assert!(
            err.contains("reserved for namespace prefixes"),
            "expected namespace-separator error, got: {err}"
        );
    }

    #[test]
    fn plugin_kind_prefix_constant_is_stable() {
        assert_eq!(PLUGIN_KIND_PREFIX, "plugin::wasm::");
    }

    #[test]
    fn load_plugins_from_directory_returns_empty_for_missing_dir() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let missing = std::env::temp_dir().join("streamkit-plugin-wasm-tests-missing-dir");
        let _ = fs::remove_dir_all(&missing);
        let plugins = runtime.load_plugins_from_directory(&missing);
        assert!(plugins.is_empty());
    }

    #[test]
    fn load_plugins_from_directory_skips_non_wasm_files_and_invalid_wasm() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let dir = tempfile::TempDir::new().expect("temp dir creates");

        fs::write(dir.path().join("README.txt"), b"not a wasm file").expect("write txt");
        fs::write(dir.path().join("bogus.wasm"), b"not really wasm bytes")
            .expect("write bogus wasm");

        let plugins = runtime.load_plugins_from_directory(dir.path());
        assert!(
            plugins.is_empty(),
            "non-wasm files and malformed .wasm files must be skipped, got {} plugins",
            plugins.len()
        );
    }

    #[test]
    fn load_plugins_from_directory_returns_empty_for_empty_dir() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let dir = tempfile::TempDir::new().expect("temp dir creates");
        assert!(runtime.load_plugins_from_directory(dir.path()).is_empty());
    }

    #[test]
    fn load_plugin_returns_error_for_non_existent_path() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let missing = std::env::temp_dir().join("streamkit-wasm-no-such-file.wasm");
        let _ = fs::remove_file(&missing);
        let Err(err) = runtime.load_plugin(&missing) else {
            panic!("missing .wasm file must yield an error")
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Failed to load component"),
            "expected wrapped load error, got: {msg}"
        );
    }

    #[test]
    fn load_plugin_returns_error_for_malformed_wasm_bytes() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let dir = tempfile::TempDir::new().expect("temp dir creates");
        let path = dir.path().join("bad.wasm");
        // Random non-wasm bytes — wasmtime should refuse to parse this.
        fs::write(&path, b"definitely not a wasm component").expect("write bytes");
        let Err(err) = runtime.load_plugin(&path) else {
            panic!("malformed wasm must yield an error")
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Failed to load component"),
            "expected wrapped load error message, got: {msg}"
        );
    }

    fn empty_component_loaded_plugin(runtime: &PluginRuntime, kind: &str) -> LoadedPlugin {
        // Construct a LoadedPlugin by hand for tests that only exercise the
        // metadata accessors and `create_node`. The component bytes are valid
        // wasm (parsed via WAT), so `Component::new` accepts them; no plugin
        // execution is required by the assertions below.
        let engine = runtime.engine_for_test();
        let component = Component::new(&engine, b"(component)")
            .expect("trivial WAT component must compile for tests");
        LoadedPlugin {
            component,
            metadata: wit_types::NodeMetadata {
                kind: kind.to_string(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                param_schema: String::new(),
                categories: Vec::new(),
            },
            engine,
            linker: runtime.linker_for_test(),
            max_memory_bytes: PluginRuntimeConfig::default().max_memory_bytes,
        }
    }

    #[test]
    fn loaded_plugin_metadata_accessor_returns_stored_metadata() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let plugin = empty_component_loaded_plugin(&runtime, "demo");
        let metadata = plugin.metadata();
        assert_eq!(metadata.kind, "demo");
        assert!(metadata.inputs.is_empty());
        assert!(metadata.outputs.is_empty());
    }

    #[test]
    fn loaded_plugin_create_node_returns_processor_node_with_pin_shape_from_metadata() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let mut plugin = empty_component_loaded_plugin(&runtime, "demo-create");
        plugin.metadata.inputs.push(wit_types::InputPin {
            name: "in".into(),
            accepts_types: vec![wit_types::PacketType::Text],
        });
        plugin.metadata.outputs.push(wit_types::OutputPin {
            name: "out".into(),
            produces_type: wit_types::PacketType::Text,
        });
        let node = plugin
            .create_node(Some(&serde_json::json!({"k": "v"})))
            .expect("create_node must succeed for valid metadata");
        let inputs = node.input_pins();
        let outputs = node.output_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
    }

    #[test]
    fn loaded_plugin_create_node_accepts_none_params() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let plugin = empty_component_loaded_plugin(&runtime, "demo-none");
        let node = plugin.create_node(None).expect("create_node accepts None params");
        // Empty metadata means no pins exposed on the constructed node.
        assert!(node.input_pins().is_empty());
        assert!(node.output_pins().is_empty());
    }

    #[test]
    fn register_plugins_skips_plugins_with_kinds_containing_reserved_separator() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let bad = empty_component_loaded_plugin(&runtime, "foo::bar");
        let mut registry = NodeRegistry::new();
        register_plugins(&mut registry, vec![bad]);
        assert!(
            registry.get_definition("plugin::wasm::foo::bar").is_none(),
            "invalid kind must NOT be registered"
        );
        assert!(
            registry.get_definition("foo::bar").is_none(),
            "raw invalid kind must NOT be registered either"
        );
    }

    #[test]
    fn register_plugins_namespaces_valid_kinds_and_registers_factory() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let plugin = empty_component_loaded_plugin(&runtime, "gain");
        let mut registry = NodeRegistry::new();
        register_plugins(&mut registry, vec![plugin]);
        let def = registry
            .get_definition("plugin::wasm::gain")
            .expect("valid plugin kind must be registered under namespaced name");
        assert_eq!(def.kind, "plugin::wasm::gain");
    }

    #[test]
    fn register_plugins_uses_empty_object_when_param_schema_is_not_valid_json() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let mut plugin = empty_component_loaded_plugin(&runtime, "noisy");
        // Force the schema string to non-JSON so the unwrap_or_else fallback is hit.
        plugin.metadata.param_schema = "not json {{ at all".to_string();
        let mut registry = NodeRegistry::new();
        register_plugins(&mut registry, vec![plugin]);
        let def = registry
            .get_definition("plugin::wasm::noisy")
            .expect("plugin with non-JSON schema must still register");
        assert_eq!(def.param_schema, serde_json::json!({}));
    }

    #[test]
    fn register_plugins_preserves_param_schema_when_well_formed_json() {
        let runtime =
            PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime must initialize");
        let mut plugin = empty_component_loaded_plugin(&runtime, "configured");
        plugin.metadata.param_schema =
            r#"{"type":"object","properties":{"gain":{"type":"number"}}}"#.to_string();
        let mut registry = NodeRegistry::new();
        register_plugins(&mut registry, vec![plugin]);
        let def = registry
            .get_definition("plugin::wasm::configured")
            .expect("plugin with well-formed schema must register");
        assert_eq!(
            def.param_schema,
            serde_json::json!({"type":"object","properties":{"gain":{"type":"number"}}})
        );
    }

    #[tokio::test]
    async fn host_state_send_output_returns_error_when_no_sender_is_attached() {
        // Direct exercise of the Host trait impl: without an output_sender set,
        // any send_output call must surface a clear error rather than panicking.
        let mut state = HostState {
            wasi: wasmtime_wasi::WasiCtx::builder().build(),
            resource_table: wasmtime::component::ResourceTable::new(),
            output_sender: None,
            limits: wasmtime::StoreLimitsBuilder::new().memory_size(1 << 20).build(),
        };
        let packet = wit_types::Packet::Text("hi".to_string());
        let err = <HostState as Host>::send_output(&mut state, "out".to_string(), packet)
            .await
            .expect_err("send_output without sender must error");
        assert!(
            err.contains("Output sender not initialized"),
            "error message must mention missing sender, got: {err}"
        );
    }

    #[tokio::test]
    async fn host_state_log_does_not_panic_for_each_log_level() {
        // The log implementation just forwards to `tracing`; exercise every branch
        // so any future regression that panics in a single arm is caught.
        let mut state = HostState {
            wasi: wasmtime_wasi::WasiCtx::builder().build(),
            resource_table: wasmtime::component::ResourceTable::new(),
            output_sender: None,
            limits: wasmtime::StoreLimitsBuilder::new().memory_size(1 << 20).build(),
        };
        for (level, label) in [
            (LogLevel::Debug, "dbg"),
            (LogLevel::Info, "info"),
            (LogLevel::Warn, "warn"),
            (LogLevel::Error, "err"),
        ] {
            <HostState as Host>::log(&mut state, level, label.to_string()).await;
        }
    }
}
