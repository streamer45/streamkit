// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Native Plugin Runtime for StreamKit
//!
//! This crate provides the host-side runtime for loading and executing native plugins
//! that use the C ABI interface.

pub mod wrapper;

use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use std::path::Path;
use std::sync::Arc;
use streamkit_core::{NodeRegistry, PinCardinality};
use streamkit_plugin_sdk_native::types::{CNativePluginAPI, NATIVE_PLUGIN_API_VERSION};
use streamkit_plugin_sdk_native::{conversions, types::PLUGIN_API_SYMBOL};
use tracing::info;

/// A loaded native plugin
#[derive(Clone)]
pub struct LoadedNativePlugin {
    library: Arc<Library>,
    api: &'static CNativePluginAPI,
    metadata: PluginMetadata,
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

        // Check API version compatibility
        if api.version != NATIVE_PLUGIN_API_VERSION {
            let plugin_version = api.version;
            return Err(anyhow!(
                "Plugin API version mismatch: plugin has {plugin_version}, host expects {NATIVE_PLUGIN_API_VERSION}"
            ));
        }

        // Extract metadata
        let metadata = Self::extract_metadata(api)?;

        info!(kind = %metadata.kind, "Successfully loaded native plugin");

        Ok(Self { library: Arc::new(library), api, metadata })
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

        Ok(PluginMetadata { kind, description, inputs, outputs, param_schema, categories })
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
        let inputs = metadata.inputs.clone();
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
