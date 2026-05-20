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
use std::path::Path;
use std::sync::Arc;
use streamkit_core::{NodeRegistry, PinCardinality};
use streamkit_plugin_sdk_native::types::PLUGIN_SET_LOG_ENABLED_SYMBOL;
use streamkit_plugin_sdk_native::types::{
    CNativePluginAPI, CSetLogEnabledCallback, NATIVE_PLUGIN_API_VERSION,
};
use streamkit_plugin_sdk_native::{conversions, types::PLUGIN_API_SYMBOL};
use tracing::{info, warn};

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
    set_log_enabled_callback: Option<CSetLogEnabledCallback>,
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
                anyhow!("Failed to load library '{path_display}': {e}.")
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

        let set_log_enabled_callback = if api.version >= 9 {
            // SAFETY: Optional extension symbol. When present, the SDK macro
            // exports it with the exact `CSetLogEnabledCallback` signature.
            match unsafe { library.get::<CSetLogEnabledCallback>(PLUGIN_SET_LOG_ENABLED_SYMBOL) } {
                Ok(symbol) => Some(*symbol),
                Err(e) => {
                    warn!(
                        kind = %metadata.kind,
                        api_version = api.version,
                        error = %e,
                        "v9 plugin did not export log-enabled callback symbol"
                    );
                    None
                },
            }
        } else {
            None
        };

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

        Ok(Self {
            library: Arc::new(library),
            api,
            metadata,
            call_timeout: Some(wrapper::DEFAULT_CALL_TIMEOUT),
            set_log_enabled_callback,
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

        // SAFETY: c_meta.kind is a valid C string pointer provided by the plugin.
        let kind = unsafe {
            conversions::c_str_to_string(c_meta.kind)
                .map_err(|e| anyhow!("Failed to read plugin kind: {e}"))?
        };

        // SAFETY: c_meta.description is either null or a valid C string.
        let description = if c_meta.description.is_null() {
            None
        } else {
            Some(unsafe {
                conversions::c_str_to_string(c_meta.description)
                    .map_err(|e| anyhow!("Failed to read plugin description: {e}"))?
            })
        };

        let mut inputs = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the inputs array.
        let c_inputs = unsafe { std::slice::from_raw_parts(c_meta.inputs, c_meta.inputs_count) };

        for c_input in c_inputs {
            let name = unsafe {
                conversions::c_str_to_string(c_input.name)
                    .map_err(|e| anyhow!("Failed to read input pin name: {e}"))?
            };

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

        let mut outputs = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the outputs array.
        let c_outputs = unsafe { std::slice::from_raw_parts(c_meta.outputs, c_meta.outputs_count) };

        for c_output in c_outputs {
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

        // SAFETY: c_meta.param_schema is a valid C string pointer.
        let param_schema_str = unsafe {
            conversions::c_str_to_string(c_meta.param_schema)
                .map_err(|e| anyhow!("Failed to read param schema: {e}"))?
        };

        let param_schema = if param_schema_str.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&param_schema_str).context("Failed to parse param schema JSON")?
        };

        let mut categories = Vec::new();
        // SAFETY: The plugin provides a valid pointer and count for the categories array.
        let c_categories =
            unsafe { std::slice::from_raw_parts(c_meta.categories, c_meta.categories_count) };

        for c_cat_ptr in c_categories {
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
    /// Pass `None` to fall back to the default backstop timeout
    /// ([`DEFAULT_CALL_TIMEOUT`](crate::wrapper::DEFAULT_CALL_TIMEOUT),
    /// 300 s) instead of a caller-chosen duration.  The reply side is
    /// **never** truly unbounded — the backstop always applies.
    ///
    /// The channel-send timeout (backpressure guard) also uses
    /// `DEFAULT_CALL_TIMEOUT` when this is `None`.
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
            self.set_log_enabled_callback,
        )?;

        Ok(Box::new(wrapper))
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

#[cfg(test)]
mod tests {
    // Tests intentionally use unwrap/expect so any failure points directly at
    // the failed precondition (tempfile creation, registry construction)
    // rather than a propagated `?` from deep inside the test body.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use streamkit_core::types::PacketType;

    #[test]
    fn namespaced_kind_prepends_plugin_namespace() {
        let out = namespaced_kind("gain").expect("simple name accepted");
        assert_eq!(out, "plugin::native::gain");
    }

    #[test]
    fn namespaced_kind_returns_already_prefixed_unchanged() {
        let out = namespaced_kind("plugin::native::whisper").expect("idempotent");
        assert_eq!(out, "plugin::native::whisper");
    }

    #[test]
    fn namespaced_kind_rejects_unprefixed_namespace_separator() {
        let err = namespaced_kind("vendor::gain").expect_err("contains '::'");
        let msg = err.to_string();
        assert!(msg.contains("'::'"), "error mentions reserved separator: {msg}");
    }

    #[test]
    fn namespaced_kind_rejects_reserved_core_prefix() {
        let err = namespaced_kind("core::gain").expect_err("core:: is reserved");
        let msg = err.to_string();
        assert!(msg.contains("core::"), "error mentions reserved prefix: {msg}");
    }

    #[test]
    fn namespaced_kind_empty_input_is_prefixed_or_rejected() {
        // Empty kind is intentionally left ambiguous: today it is prefixed
        // ("plugin::native::") and rejected downstream by the registry, but
        // moving rejection upstream into namespaced_kind() would be an
        // equally valid implementation. Either outcome is acceptable here;
        // a panic is not.
        if let Ok(s) = namespaced_kind("") {
            assert_eq!(s, "plugin::native::");
        }
    }

    #[test]
    fn register_plugins_returns_zero_for_empty_input() {
        let mut registry = NodeRegistry::new();
        let count = register_plugins(&mut registry, Vec::new()).expect("registers nothing");
        assert_eq!(count, 0);
    }

    #[test]
    fn plugin_metadata_is_debug_and_clone() {
        let meta = PluginMetadata {
            kind: "gain".into(),
            description: Some("A gain node".into()),
            inputs: vec![streamkit_core::InputPin {
                name: "in".into(),
                accepts_types: vec![PacketType::Text],
                cardinality: PinCardinality::One,
            }],
            outputs: vec![streamkit_core::OutputPin {
                name: "out".into(),
                produces_type: PacketType::Text,
                cardinality: PinCardinality::Broadcast,
            }],
            param_schema: serde_json::json!({"type": "object"}),
            categories: vec!["audio".into()],
            is_source: false,
            tick_interval_us: 0,
            max_ticks: 0,
        };

        let cloned = meta.clone();
        assert_eq!(cloned.kind, "gain");
        assert_eq!(cloned.description.as_deref(), Some("A gain node"));
        assert_eq!(cloned.inputs.len(), 1);
        assert_eq!(cloned.outputs.len(), 1);
        assert_eq!(cloned.categories, vec!["audio".to_string()]);

        let dbg = format!("{meta:?}");
        assert!(dbg.contains("gain"));
        assert!(dbg.contains("PluginMetadata"));
    }

    /// Confirms that `load()` surfaces a wrapped libloading error (not a panic
    /// or silent fallback) when given a path that cannot be opened.  Keeps the
    /// expected error-message shape stable without requiring a real .so.
    #[test]
    fn load_returns_error_for_missing_library() {
        let result = LoadedNativePlugin::load("/this/path/definitely/does/not/exist.so");
        let Err(err) = result else { panic!("expected error for missing path") };
        let msg = err.to_string();
        assert!(msg.starts_with("Failed to load library"), "wrapped libloading error: {msg}");
        assert!(msg.contains("/this/path/definitely/does/not/exist.so"));
    }

    /// load() must error on a path that points at a non-library file rather than
    /// proceeding into symbol lookup with garbage memory.  Uses a tempfile so the
    /// failure source is "not a dynamic library" rather than "file not found".
    #[test]
    fn load_returns_error_for_non_library_file() {
        let mut tmp = tempfile::Builder::new().suffix(".so").tempfile().expect("create tempfile");
        std::io::Write::write_all(&mut tmp, b"not a real shared object").expect("write tempfile");
        let path = tmp.path().to_path_buf();

        let Err(err) = LoadedNativePlugin::load(&path) else {
            panic!("expected error for non-library file");
        };
        let msg = err.to_string();
        assert!(msg.starts_with("Failed to load library"), "wrapped libloading error: {msg}");
    }
}
