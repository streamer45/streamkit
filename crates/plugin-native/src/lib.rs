// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Native Plugin Runtime for StreamKit
//!
//! This crate provides the host-side runtime for loading and executing native plugins
//! that use the C ABI interface.

pub mod metrics;
pub mod wrapper;

use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use streamkit_core::{NodeRegistry, PinCardinality};
use streamkit_plugin_sdk_native::types::{CNativePluginAPI, NATIVE_PLUGIN_API_VERSION};
use streamkit_plugin_sdk_native::{conversions, types::PLUGIN_API_SYMBOL};
use tracing::{info, warn};

/// Global registry of loaded native plugins, used for diagnostics.
static PLUGIN_REGISTRY: std::sync::LazyLock<LoadedPluginRegistry> =
    std::sync::LazyLock::new(LoadedPluginRegistry::new);

/// Returns a reference to the global [`LoadedPluginRegistry`].
pub fn plugin_registry() -> &'static LoadedPluginRegistry {
    &PLUGIN_REGISTRY
}

/// Silent log callback used only during source-config probing (no actual instance work).
// Cannot be `const`: `const extern "C" fn` is not supported by the compiler.
#[allow(clippy::missing_const_for_fn)]
extern "C" fn plugin_log_callback_noop(
    _level: streamkit_plugin_sdk_native::types::CLogLevel,
    _target: *const std::os::raw::c_char,
    _message: *const std::os::raw::c_char,
    _user_data: *mut std::ffi::c_void,
) {
}

/// A loaded native plugin
#[derive(Clone)]
pub struct LoadedNativePlugin {
    library: Arc<Library>,
    api: &'static CNativePluginAPI,
    metadata: PluginMetadata,
    call_timeout: Option<std::time::Duration>,
}

/// Metadata extracted from a plugin
#[derive(Debug, Clone)]
pub struct PluginMetadata {
    pub kind: String,
    pub description: Option<String>,
    pub inputs: Vec<streamkit_core::InputPin>,
    pub outputs: Vec<streamkit_core::OutputPin>,
    pub param_schema: serde_json::Value,
    pub categories: Vec<String>,
    /// `true` when the plugin exports `get_source_config` and reports `is_source = true`.
    /// Source plugins use the tick loop instead of the input-driven processing loop.
    pub is_source: bool,
    /// Tick interval for source plugins (microseconds).  Only meaningful when `is_source` is `true`.
    pub tick_interval_us: u64,
    /// Maximum number of ticks (0 = infinite).  Only meaningful when `is_source` is `true`.
    pub max_ticks: u64,
}

impl LoadedNativePlugin {
    /// Load a plugin from a dynamic library file
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The library file cannot be loaded
    /// - The plugin doesn't export the required API symbol
    /// - The API version is incompatible
    /// - Plugin metadata is invalid or cannot be read
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        const MIN_SUPPORTED_API_VERSION: u32 = 6;

        let path = path.as_ref();

        info!(?path, "Loading native plugin");

        // Load the dynamic library
        // SAFETY: Loading a dynamic library is inherently unsafe as we're executing code
        // from an external source. The plugin is trusted code (verified by the user/admin).
        let library = unsafe {
            Library::new(path).map_err(|e| {
                let path_display = path.display();
                // libloading::Error contains detailed information about what went wrong
                anyhow!("Failed to load library '{path_display}': {e}.",)
            })?
        };

        // Get the plugin API symbol
        // SAFETY: Looking up symbols in the loaded library. The function signature must match
        // the plugin's export. The native_plugin_entry! macro ensures this contract is upheld.
        let api_fn: Symbol<extern "C" fn() -> *const CNativePluginAPI> = unsafe {
            library.get(PLUGIN_API_SYMBOL).map_err(|e| {
                anyhow!(
                    "Plugin does not export '{}' symbol: {}. \
                         Make sure the plugin was built with the native_plugin_entry! macro.",
                    std::str::from_utf8(PLUGIN_API_SYMBOL).unwrap_or("streamkit_native_plugin_api"),
                    e
                )
            })?
        };

        let api_ptr = api_fn();
        if api_ptr.is_null() {
            return Err(anyhow!("Plugin API function returned null pointer"));
        }

        // SAFETY: We've verified the pointer is non-null. The plugin API struct is valid for
        // the lifetime of the loaded library, which we keep alive via Arc<Library>.
        let api = unsafe { &*api_ptr };

        // Check API version compatibility — accept v6 through v9.
        // v6: pre-BinaryWithMeta.
        // v7: added BinaryWithMeta.
        // v8: added EncodedAudio metadata.
        // v9: zero-copy binary packets (buffer_handle), logger overhaul
        //     (set_log_enabled_callback, target = plugin kind).
        // v6–v8 are wire-compatible; v9 adds a Binary→BinaryWithMeta wire
        // upgrade (see v9 notes in types.rs).  Version-gated features use
        // runtime api_version checks (e.g. buffer_handle for v9, downgrade
        // for v6).
        if api.version < MIN_SUPPORTED_API_VERSION || api.version > NATIVE_PLUGIN_API_VERSION {
            let plugin_version = api.version;
            return Err(anyhow!(
                "Plugin API version mismatch: plugin has v{plugin_version}, \
                 host supports v{MIN_SUPPORTED_API_VERSION}–v{NATIVE_PLUGIN_API_VERSION}"
            ));
        }

        // Extract metadata
        let mut metadata = Self::extract_metadata(api)?;

        // Detect source plugin capability from the v3 API fields.
        // If the plugin provides `get_source_config`, we probe it with a temporary
        // instance to read tick parameters.  If instance creation fails we fall back
        // to treating it as a processor plugin.
        if let Some(get_source_config) = api.get_source_config {
            // Create a temporary instance with no params to query source config
            let temp_handle = (api.create_instance)(
                std::ptr::null(),
                plugin_log_callback_noop,
                std::ptr::null_mut(),
            );
            if temp_handle.is_null() {
                warn!(
                    kind = %metadata.kind,
                    "Source config probe failed: plugin returned null from create_instance \
                     with no params — treating as processor. Source plugins must support \
                     parameterless construction for probing."
                );
            } else {
                let cfg = get_source_config(temp_handle);
                if cfg.is_source {
                    metadata.is_source = true;
                    metadata.tick_interval_us = cfg.tick_interval_us;
                    metadata.max_ticks = cfg.max_ticks;
                    info!(
                        kind = %metadata.kind,
                        tick_interval_us = cfg.tick_interval_us,
                        max_ticks = cfg.max_ticks,
                        "Detected source plugin"
                    );
                }
                (api.destroy_instance)(temp_handle);
            }
        }

        info!(kind = %metadata.kind, "Successfully loaded native plugin");

        PLUGIN_REGISTRY.register(&metadata.kind, path, api.version);

        Ok(Self {
            library: Arc::new(library),
            api,
            metadata,
            call_timeout: Some(wrapper::DEFAULT_CALL_TIMEOUT),
        })
    }

    /// Extract metadata from the plugin
    fn extract_metadata(api: &CNativePluginAPI) -> Result<PluginMetadata> {
        let c_metadata = (api.get_metadata)();
        if c_metadata.is_null() {
            return Err(anyhow!("Plugin metadata is null"));
        }

        // SAFETY: We've verified the pointer is non-null. The metadata struct is valid for
        // the lifetime of the plugin API call.
        let c_meta = unsafe { &*c_metadata };

        // Extract kind
        // SAFETY: c_meta.kind is a valid C string pointer provided by the plugin.
        let kind = unsafe {
            conversions::c_str_to_string(c_meta.kind)
                .map_err(|e| anyhow!("Failed to read plugin kind: {e}"))?
        };

        // Extract description (optional)
        // SAFETY: c_meta.description is either a valid C string pointer or null.
        let description = if c_meta.description.is_null() {
            None
        } else {
            Some(unsafe {
                conversions::c_str_to_string(c_meta.description)
                    .map_err(|e| anyhow!("Failed to read plugin description: {e}"))?
            })
        };

        // Extract inputs
        let mut inputs = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the inputs array.
        let c_inputs = unsafe { std::slice::from_raw_parts(c_meta.inputs, c_meta.inputs_count) };

        for c_input in c_inputs {
            // SAFETY: c_input.name is a valid C string pointer provided by the plugin.
            let name = unsafe {
                conversions::c_str_to_string(c_input.name)
                    .map_err(|e| anyhow!("Failed to read input pin name: {e}"))?
            };

            // SAFETY: The plugin provides valid pointer and count for the accepts_types array.
            let accepts_types_slice = unsafe {
                std::slice::from_raw_parts(c_input.accepts_types, c_input.accepts_types_count)
            };

            let accepts_types = accepts_types_slice
                .iter()
                .map(|t| {
                    conversions::packet_type_from_c(*t)
                        .map_err(|e| anyhow!("Failed to read accepted packet type: {e}"))
                })
                .collect::<Result<Vec<_>>>()?;

            inputs.push(streamkit_core::InputPin {
                name,
                accepts_types,
                cardinality: PinCardinality::One,
            });
        }

        // Extract outputs
        let mut outputs = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the outputs array.
        let c_outputs = unsafe { std::slice::from_raw_parts(c_meta.outputs, c_meta.outputs_count) };

        for c_output in c_outputs {
            // SAFETY: c_output.name is a valid C string pointer provided by the plugin.
            let name = unsafe {
                conversions::c_str_to_string(c_output.name)
                    .map_err(|e| anyhow!("Failed to read output pin name: {e}"))?
            };

            outputs.push(streamkit_core::OutputPin {
                name,
                produces_type: conversions::packet_type_from_c(c_output.produces_type)
                    .map_err(|e| anyhow!("Failed to read produced packet type: {e}"))?,
                cardinality: PinCardinality::Broadcast,
            });
        }

        // Extract param schema
        // SAFETY: c_meta.param_schema is a valid C string pointer provided by the plugin.
        let param_schema_str = unsafe {
            conversions::c_str_to_string(c_meta.param_schema)
                .map_err(|e| anyhow!("Failed to read param schema: {e}"))?
        };

        let param_schema = if param_schema_str.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&param_schema_str).context("Failed to parse param schema JSON")?
        };

        // Extract categories
        let mut categories = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the categories array.
        let c_categories =
            unsafe { std::slice::from_raw_parts(c_meta.categories, c_meta.categories_count) };

        for c_cat_ptr in c_categories {
            // SAFETY: Each category pointer is a valid C string provided by the plugin.
            let cat = unsafe {
                conversions::c_str_to_string(*c_cat_ptr)
                    .map_err(|e| anyhow!("Failed to read category: {e}"))?
            };
            categories.push(cat);
        }

        Ok(PluginMetadata {
            kind,
            description,
            inputs,
            outputs,
            param_schema,
            categories,
            is_source: false,
            tick_interval_us: 0,
            max_ticks: 0,
        })
    }

    /// Get the plugin metadata
    pub const fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    /// Get the plugin API
    pub const fn api(&self) -> &'static CNativePluginAPI {
        self.api
    }

    /// Get a reference to the loaded library
    pub const fn library(&self) -> &Arc<Library> {
        &self.library
    }

    /// Override the reply-side timeout for FFI calls (process_packet, flush,
    /// tick).  This controls how long the async side waits for the worker
    /// thread's oneshot reply.
    ///
    /// Pass `None` to wait indefinitely for the FFI call to complete.
    /// The channel-send timeout (backpressure guard) is always bounded
    /// regardless of this setting.
    ///
    /// # Warning
    ///
    /// Setting `None` (infinite wait) can wedge the host's async executor
    /// (the main tokio runtime pool) if a plugin worker hangs or deadlocks.
    /// Prefer a finite timeout in production and use `None` only for
    /// debugging or known-slow operations (e.g. large-model ML inference).
    pub const fn set_call_timeout(&mut self, timeout: Option<std::time::Duration>) {
        self.call_timeout = timeout;
    }

    /// Create a new node instance from this plugin
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Parameter serialization fails
    /// - The plugin fails to create an instance
    pub fn create_node(
        &self,
        params: Option<&serde_json::Value>,
    ) -> Result<Box<dyn streamkit_core::ProcessorNode>, streamkit_core::StreamKitError> {
        let wrapper = wrapper::NativeNodeWrapper::new(
            self.library.clone(),
            self.api,
            self.metadata.clone(),
            params,
            self.call_timeout,
        )?;

        Ok(Box::new(wrapper))
    }
}

impl Drop for LoadedNativePlugin {
    fn drop(&mut self) {
        PLUGIN_REGISTRY.unregister(&self.metadata.kind);
    }
}

/// Register a list of native plugins with the node registry
///
/// Returns the number of plugins registered
///
/// # Errors
///
/// This function currently does not return errors, but returns `Result`
/// for future extensibility.
pub fn register_plugins(
    registry: &mut NodeRegistry,
    plugins: Vec<LoadedNativePlugin>,
) -> Result<usize> {
    let mut count = 0;

    for plugin in plugins {
        let metadata = plugin.metadata();
        let original_kind = metadata.kind.clone();
        let kind = namespaced_kind(&original_kind)?;
        let param_schema = metadata.param_schema.clone();
        let categories = metadata.categories.clone();
        let is_source = metadata.is_source;

        // Source plugins register with empty inputs (they produce data, not consume it).
        let inputs = if is_source { Vec::new() } else { metadata.inputs.clone() };
        let outputs = metadata.outputs.clone();

        // Debug: Log what we're registering
        tracing::info!(
            kind = %kind,
            inputs = ?inputs,
            outputs = ?outputs,
            "Registering native plugin with pins"
        );

        // Create the factory closure
        let plugin_arc = Arc::new(plugin);
        let factory = move |params: Option<&serde_json::Value>| plugin_arc.create_node(params);

        // Register with static pins (extracted from plugin metadata)
        let static_pins = streamkit_core::registry::StaticPins { inputs, outputs };
        registry.register_static(&kind, factory, param_schema, static_pins, categories, false);

        info!(kind = %kind, "Registered native plugin");
        count += 1;
    }

    Ok(count)
}

/// Helper function to add the `plugin::native::` prefix to plugin kinds
///
/// # Errors
///
/// Returns an error if:
/// - The original kind contains `::` (namespace separator is reserved)
/// - The original kind starts with reserved prefix `core::`
pub fn namespaced_kind(original_kind: &str) -> Result<String> {
    const PLUGIN_KIND_PREFIX: &str = "plugin::native::";
    const RESERVED_PREFIX: &str = "core::";

    // Validate: reject if already has a namespace prefix
    if original_kind.starts_with(PLUGIN_KIND_PREFIX) {
        return Ok(original_kind.to_string());
    }

    // Validate: reject if contains namespace separator
    if original_kind.contains("::") {
        return Err(anyhow!(
            "Plugin kind '{original_kind}' contains '::' which is reserved for namespace prefixes. \
             Plugin kinds must be simple names like 'gain', 'reverb', etc."
        ));
    }

    // Validate: reject if uses reserved prefix
    if original_kind.starts_with(RESERVED_PREFIX) {
        return Err(anyhow!(
            "Plugin kind '{original_kind}' uses reserved prefix '{RESERVED_PREFIX}'. \
             This prefix is reserved for built-in core nodes."
        ));
    }

    Ok(format!("{PLUGIN_KIND_PREFIX}{original_kind}"))
}

// ── Loaded Plugin Registry ─────────────────────────────────────────────────

/// Information about a loaded native plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub kind: String,
    pub plugin_path: std::path::PathBuf,
    pub api_version: u32,
    pub load_count: usize,
}

/// Registry tracking all currently loaded native plugins.
///
/// Intended for future diagnostics endpoints (e.g. `/debug/plugins`).
/// Thread-safe via interior `RwLock`.
pub struct LoadedPluginRegistry {
    plugins: RwLock<HashMap<String, PluginInfo>>,
}

impl LoadedPluginRegistry {
    pub fn new() -> Self {
        Self { plugins: RwLock::new(HashMap::new()) }
    }

    pub fn register(&self, kind: &str, path: &std::path::Path, api_version: u32) {
        let Ok(mut map) = self.plugins.write() else {
            warn!("LoadedPluginRegistry: write lock poisoned during register");
            return;
        };
        let entry = map.entry(kind.to_string()).or_insert_with(|| PluginInfo {
            kind: kind.to_string(),
            plugin_path: path.to_path_buf(),
            api_version,
            load_count: 0,
        });
        entry.load_count += 1;
    }

    pub fn unregister(&self, kind: &str) {
        let Ok(mut map) = self.plugins.write() else {
            warn!("LoadedPluginRegistry: write lock poisoned during unregister");
            return;
        };
        if let Some(info) = map.get_mut(kind) {
            info.load_count = info.load_count.saturating_sub(1);
            if info.load_count == 0 {
                map.remove(kind);
            }
        }
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        let Ok(map) = self.plugins.read() else {
            warn!("LoadedPluginRegistry: read lock poisoned during list");
            return Vec::new();
        };
        map.values().cloned().collect()
    }
}

impl Default for LoadedPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
