// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! StreamKit Native Plugin SDK
//!
//! This SDK provides an ergonomic Rust interface for writing native plugins that use
//! a stable C ABI. While the interface feels like pure Rust, under the hood it generates
//! C-compatible exports for maximum binary compatibility.
//!
//! # Example
//!
//! ```no_run
//! use streamkit_plugin_sdk_native::prelude::*;
//!
//! pub struct MyPlugin {
//!     // plugin state
//! }
//!
//! impl NativeProcessorNode for MyPlugin {
//!     fn metadata() -> NodeMetadata {
//!         NodeMetadata::builder("my_plugin")
//!             .input("in", &[PacketType::Any])
//!             .output("out", PacketType::Any)
//!             .build()
//!     }
//!
//!     fn new(_params: Option<serde_json::Value>, _logger: Logger) -> Result<Self, String> {
//!         Ok(Self {})
//!     }
//!
//!     fn process(
//!         &mut self,
//!         _pin: &str,
//!         packet: Packet,
//!         output: &OutputSender,
//!     ) -> Result<(), String> {
//!         output.send("out", &packet)?;
//!         Ok(())
//!     }
//! }
//!
//! native_plugin_entry!(MyPlugin);
//! ```

pub mod conversions;
pub mod logger;
pub mod types;

use std::ffi::CString;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{InputPin, OutputPin, PinCardinality, Resource};

use logger::Logger;

pub use streamkit_core;
pub use types::*;

/// Re-export commonly used types
pub mod prelude {
    pub use crate::logger::Logger;
    pub use crate::types::{CLogCallback, CLogLevel};
    pub use crate::{
        native_plugin_entry, plugin_debug, plugin_error, plugin_info, plugin_log, plugin_trace,
        plugin_warn, NativeProcessorNode, NodeMetadata, OutputSender, ResourceSupport,
    };
    pub use streamkit_core::types::{AudioFrame, Packet, PacketType};
    pub use streamkit_core::{InputPin, OutputPin, PinCardinality, Resource};
}

/// Metadata about a node type
pub struct NodeMetadata {
    pub kind: String,
    pub description: Option<String>,
    pub inputs: Vec<InputPin>,
    pub outputs: Vec<OutputPin>,
    pub param_schema: serde_json::Value,
    pub categories: Vec<String>,
}

impl NodeMetadata {
    /// Create a builder for node metadata
    pub fn builder(kind: &str) -> NodeMetadataBuilder {
        NodeMetadataBuilder {
            kind: kind.to_string(),
            description: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            param_schema: serde_json::json!({}),
            categories: Vec::new(),
        }
    }
}

/// Builder for NodeMetadata
pub struct NodeMetadataBuilder {
    kind: String,
    description: Option<String>,
    inputs: Vec<InputPin>,
    outputs: Vec<OutputPin>,
    param_schema: serde_json::Value,
    categories: Vec<String>,
}

impl NodeMetadataBuilder {
    /// Set the node description
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add an input pin
    #[must_use]
    pub fn input(mut self, name: &str, accepts_types: &[PacketType]) -> Self {
        self.inputs.push(InputPin {
            name: name.to_string(),
            accepts_types: accepts_types.to_vec(),
            cardinality: PinCardinality::One,
        });
        self
    }

    /// Add an output pin
    #[must_use]
    pub fn output(mut self, name: &str, produces_type: PacketType) -> Self {
        self.outputs.push(OutputPin {
            name: name.to_string(),
            produces_type,
            cardinality: PinCardinality::Broadcast,
        });
        self
    }

    /// Set parameter schema
    #[must_use]
    pub fn param_schema(mut self, schema: serde_json::Value) -> Self {
        self.param_schema = schema;
        self
    }

    /// Add a category
    #[must_use]
    pub fn category(mut self, category: &str) -> Self {
        self.categories.push(category.to_string());
        self
    }

    /// Build the metadata
    pub fn build(self) -> NodeMetadata {
        NodeMetadata {
            kind: self.kind,
            description: self.description,
            inputs: self.inputs,
            outputs: self.outputs,
            param_schema: self.param_schema,
            categories: self.categories,
        }
    }
}

/// Output sender for sending packets to output pins
pub struct OutputSender {
    output_callback: COutputCallback,
    output_user_data: *mut std::os::raw::c_void,
    telemetry_callback: types::CTelemetryCallback,
    telemetry_user_data: *mut std::os::raw::c_void,
}

impl OutputSender {
    /// Create a new output sender from C callback
    pub fn from_callback(callback: COutputCallback, user_data: *mut std::os::raw::c_void) -> Self {
        Self {
            output_callback: callback,
            output_user_data: user_data,
            telemetry_callback: None,
            telemetry_user_data: std::ptr::null_mut(),
        }
    }

    /// Create a new output sender from C callbacks.
    ///
    /// `telemetry_callback` may be null if the host doesn't provide telemetry support.
    pub fn from_callbacks(
        output_callback: COutputCallback,
        output_user_data: *mut std::os::raw::c_void,
        telemetry_callback: types::CTelemetryCallback,
        telemetry_user_data: *mut std::os::raw::c_void,
    ) -> Self {
        Self { output_callback, output_user_data, telemetry_callback, telemetry_user_data }
    }

    /// Send a packet to an output pin
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The pin name contains null bytes
    /// - The C callback returns an error
    pub fn send(&self, pin: &str, packet: &Packet) -> Result<(), String> {
        let pin_c = CString::new(pin).map_err(|e| format!("Invalid pin name: {e}"))?;

        let packet_repr = conversions::packet_to_c(packet);
        let result = (self.output_callback)(
            pin_c.as_ptr(),
            &raw const packet_repr.packet,
            self.output_user_data,
        );

        if result.success {
            Ok(())
        } else {
            let error_msg = if result.error_message.is_null() {
                "Unknown error".to_string()
            } else {
                unsafe {
                    conversions::c_str_to_string(result.error_message)
                        .unwrap_or_else(|_| "Unknown error".to_string())
                }
            };
            Err(error_msg)
        }
    }

    /// Emit a telemetry event to the host telemetry bus (best-effort).
    ///
    /// `data` is encoded as JSON and forwarded out-of-band; it does not flow through graph pins.
    ///
    /// If the host doesn't provide a telemetry callback, this is a no-op.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `event_type` contains an interior NUL byte (invalid C string),
    /// - `data` cannot be serialized to JSON,
    /// - the host telemetry callback reports an error.
    pub fn emit_telemetry(
        &self,
        event_type: &str,
        data: &serde_json::Value,
        timestamp_us: Option<u64>,
    ) -> Result<(), String> {
        let Some(cb) = self.telemetry_callback else {
            return Ok(());
        };

        let event_type_c =
            CString::new(event_type).map_err(|e| format!("Invalid event_type: {e}"))?;
        let data_json = serde_json::to_vec(data)
            .map_err(|e| format!("Failed to serialize telemetry JSON: {e}"))?;

        let meta = timestamp_us.map(|ts| types::CPacketMetadata {
            timestamp_us: ts,
            has_timestamp_us: true,
            duration_us: 0,
            has_duration_us: false,
            sequence: 0,
            has_sequence: false,
        });
        let meta_ptr = meta.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);

        let result = cb(
            event_type_c.as_ptr(),
            data_json.as_ptr(),
            data_json.len(),
            meta_ptr,
            self.telemetry_user_data,
        );

        if result.success {
            Ok(())
        } else {
            let error_msg = if result.error_message.is_null() {
                "Unknown error".to_string()
            } else {
                unsafe {
                    conversions::c_str_to_string(result.error_message)
                        .unwrap_or_else(|_| "Unknown error".to_string())
                }
            };
            Err(error_msg)
        }
    }
}

/// Trait that plugin authors implement
/// This provides an ergonomic Rust interface that gets wrapped with C ABI exports
pub trait NativeProcessorNode: Sized + Send + 'static {
    /// Return metadata about this node type
    fn metadata() -> NodeMetadata;

    /// Create a new instance of the node
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (e.g., invalid parameters)
    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String>;

    /// Process an incoming packet
    ///
    /// # Errors
    ///
    /// Returns an error if packet processing fails
    fn process(&mut self, pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String>;

    /// Update runtime parameters (optional)
    ///
    /// # Errors
    ///
    /// Returns an error if parameter update fails (e.g., invalid values)
    fn update_params(&mut self, _params: Option<serde_json::Value>) -> Result<(), String> {
        Ok(())
    }

    /// Flush any buffered data when input stream ends (optional)
    ///
    /// Called when the input stream closes, allowing plugins to process any
    /// remaining buffered data before cleanup. This is useful for nodes that
    /// buffer input (e.g., sentence splitting in TTS, frame buffering in codecs).
    ///
    /// # Errors
    ///
    /// Returns an error if flushing fails
    fn flush(&mut self, _output: &OutputSender) -> Result<(), String> {
        Ok(())
    }

    /// Clean up resources (optional)
    fn cleanup(&mut self) {}
}

/// Optional trait for plugins that need shared resource management (e.g., ML models).
///
/// Plugins that implement this trait can have their resources (models) automatically
/// cached and shared across multiple node instances. This avoids loading the same
/// model multiple times in memory.
///
/// # Example
///
/// ```ignore
/// use streamkit_plugin_sdk_native::prelude::*;
/// use std::sync::Arc;
///
/// pub struct MyModelResource {
///     model_data: Vec<f32>,
/// }
///
/// impl Resource for MyModelResource {
///     fn size_bytes(&self) -> usize {
///         self.model_data.len() * std::mem::size_of::<f32>()
///     }
///     fn resource_type(&self) -> &str { "ml_model" }
/// }
///
/// pub struct MyPlugin {
///     resource: Arc<MyModelResource>,
/// }
///
/// // Note: MyPlugin must also implement NativeProcessorNode for this to compile
/// impl ResourceSupport for MyPlugin {
///     type Resource = MyModelResource;
///
///     fn compute_resource_key(params: Option<&serde_json::Value>) -> String {
///         // Hash only the params that affect resource creation
///         format!("{:?}", params)
///     }
///
///     fn init_resource(params: Option<serde_json::Value>) -> Result<Self::Resource, String> {
///         // Load model (can be expensive, but only happens once per unique params)
///         Ok(MyModelResource { model_data: vec![0.0; 1000] })
///     }
/// }
/// ```
pub trait ResourceSupport: NativeProcessorNode {
    /// The type of resource this plugin uses
    type Resource: Resource + 'static;

    /// Compute a cache key from parameters.
    ///
    /// This should hash only the parameters that affect resource initialization
    /// (e.g., model path, GPU device ID). Different parameters that produce the
    /// same key will share the same cached resource.
    fn compute_resource_key(params: Option<&serde_json::Value>) -> String;

    /// Initialize/load the resource.
    ///
    /// This is called once per unique cache key. The result is cached and shared
    /// across all node instances with matching parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if resource initialization fails (e.g., model file not found,
    /// GPU initialization error).
    ///
    /// # Note
    ///
    /// This method may be called from a blocking thread pool to avoid blocking
    /// async execution during model loading.
    fn init_resource(params: Option<serde_json::Value>) -> Result<Self::Resource, String>;

    /// Optional cleanup when the resource is being unloaded.
    ///
    /// This is called when the last reference to the resource is dropped
    /// (typically during plugin unload or LRU eviction).
    fn deinit_resource(_resource: Self::Resource) {
        // Default: just drop it
    }
}

/// Macro to generate C ABI exports for a plugin
///
/// This macro should be called once per plugin with the type that implements
/// `NativeProcessorNode`.
///
/// # Example
/// ```no_run
/// # use streamkit_plugin_sdk_native::prelude::*;
/// # struct MyPlugin;
/// # impl NativeProcessorNode for MyPlugin {
/// #     fn metadata() -> NodeMetadata { unimplemented!() }
/// #     fn new(_: Option<serde_json::Value>, _: Logger) -> Result<Self, String> { unimplemented!() }
/// #     fn process(&mut self, _: &str, _: Packet, _: &OutputSender) -> Result<(), String> { unimplemented!() }
/// # }
/// native_plugin_entry!(MyPlugin);
/// ```
#[macro_export]
macro_rules! native_plugin_entry {
    ($plugin_type:ty) => {
        // Static metadata storage
        static mut METADATA: std::sync::OnceLock<(
            $crate::types::CNodeMetadata,
            Vec<$crate::types::CInputPin>,
            Vec<$crate::types::COutputPin>,
            Vec<std::ffi::CString>,
            Vec<Vec<$crate::types::CPacketTypeInfo>>,
            Vec<Vec<Option<$crate::types::CAudioFormat>>>,
            Vec<Vec<Option<std::ffi::CString>>>,
            Vec<std::ffi::CString>,
            Vec<Option<$crate::types::CAudioFormat>>,
            Vec<Option<std::ffi::CString>>,
            Vec<std::ffi::CString>,
            Vec<*const std::os::raw::c_char>,
            std::ffi::CString,
            Option<std::ffi::CString>,
            std::ffi::CString,
        )> = std::sync::OnceLock::new();

        #[no_mangle]
        pub extern "C" fn streamkit_native_plugin_api() -> *const $crate::types::CNativePluginAPI {
            static API: $crate::types::CNativePluginAPI = $crate::types::CNativePluginAPI {
                version: $crate::types::NATIVE_PLUGIN_API_VERSION,
                get_metadata: __plugin_get_metadata,
                create_instance: __plugin_create_instance,
                process_packet: __plugin_process_packet,
                update_params: __plugin_update_params,
                flush: __plugin_flush,
                destroy_instance: __plugin_destroy_instance,
            };
            &API
        }

        extern "C" fn __plugin_get_metadata() -> *const $crate::types::CNodeMetadata {
            unsafe {
                let metadata = METADATA.get_or_init(|| {
                    let meta = <$plugin_type as $crate::NativeProcessorNode>::metadata();

                    // Convert inputs
                    let mut c_inputs = Vec::new();
                    let mut input_names = Vec::new();
                    let mut input_types = Vec::new();
                    let mut input_audio_formats = Vec::new();
                    let mut input_custom_type_ids = Vec::new();

                    for input in &meta.inputs {
                        let name = std::ffi::CString::new(input.name.as_str())
                            .expect("Input pin name should not contain null bytes");
                        let mut types_info = Vec::new();
                        let mut formats = Vec::new();
                        let mut custom_type_ids = Vec::new();

                        // First, collect all the audio formats
                        for pt in &input.accepts_types {
                            let (_type_info, audio_format) =
                                $crate::conversions::packet_type_to_c(pt);
                            formats.push(audio_format);
                            let custom_type_id = match pt {
                                $crate::streamkit_core::types::PacketType::Custom { type_id } => {
                                    Some(std::ffi::CString::new(type_id.as_str()).expect(
                                        "Custom type_id should not contain null bytes",
                                    ))
                                }
                                _ => None,
                            };
                            custom_type_ids.push(custom_type_id);
                        }

                        // Now create CPacketTypeInfo with stable pointers to the stored formats
                        for (idx, pt) in input.accepts_types.iter().enumerate() {
                            let type_discriminant = match pt {
                                $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                    $crate::types::CPacketType::RawAudio
                                }
                                $crate::streamkit_core::types::PacketType::OpusAudio => {
                                    $crate::types::CPacketType::OpusAudio
                                }
                                $crate::streamkit_core::types::PacketType::Text => {
                                    $crate::types::CPacketType::Text
                                }
                                $crate::streamkit_core::types::PacketType::Transcription => {
                                    $crate::types::CPacketType::Transcription
                                }
                                $crate::streamkit_core::types::PacketType::Custom { .. } => {
                                    $crate::types::CPacketType::Custom
                                }
                                $crate::streamkit_core::types::PacketType::Binary => {
                                    $crate::types::CPacketType::Binary
                                }
                                $crate::streamkit_core::types::PacketType::Any => {
                                    $crate::types::CPacketType::Any
                                }
                                $crate::streamkit_core::types::PacketType::Passthrough => {
                                    $crate::types::CPacketType::Any
                                }
                            };

                            let audio_format_ptr = if let Some(ref fmt) = formats[idx] {
                                fmt as *const $crate::types::CAudioFormat
                            } else {
                                std::ptr::null()
                            };

                            let custom_type_id_ptr = if let Some(ref s) = custom_type_ids[idx] {
                                s.as_ptr()
                            } else {
                                std::ptr::null()
                            };

                            types_info.push($crate::types::CPacketTypeInfo {
                                type_discriminant,
                                audio_format: audio_format_ptr,
                                custom_type_id: custom_type_id_ptr,
                            });
                        }

                        c_inputs.push($crate::types::CInputPin {
                            name: name.as_ptr(),
                            accepts_types: types_info.as_ptr(),
                            accepts_types_count: types_info.len(),
                        });

                        input_names.push(name);
                        input_types.push(types_info);
                        input_audio_formats.push(formats);
                        input_custom_type_ids.push(custom_type_ids);
                    }

                    // Convert outputs
                    let mut c_outputs = Vec::new();
                    let mut output_names = Vec::new();
                    let mut output_audio_formats = Vec::new();
                    let mut output_custom_type_ids = Vec::new();

                    for output in &meta.outputs {
                        let name = std::ffi::CString::new(output.name.as_str())
                            .expect("Output pin name should not contain null bytes");

                        // First, store the audio format
                        let (_type_info, audio_format) =
                            $crate::conversions::packet_type_to_c(&output.produces_type);
                        output_audio_formats.push(audio_format);
                        let output_custom_type_id = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::Custom { type_id } => {
                                Some(std::ffi::CString::new(type_id.as_str()).expect(
                                    "Custom type_id should not contain null bytes",
                                ))
                            }
                            _ => None,
                        };
                        output_custom_type_ids.push(output_custom_type_id);

                        // Now create CPacketTypeInfo with stable pointer to the stored format
                        let type_discriminant = match output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                $crate::types::CPacketType::RawAudio
                            }
                            $crate::streamkit_core::types::PacketType::OpusAudio => {
                                $crate::types::CPacketType::OpusAudio
                            }
                            $crate::streamkit_core::types::PacketType::Text => {
                                $crate::types::CPacketType::Text
                            }
                            $crate::streamkit_core::types::PacketType::Transcription => {
                                $crate::types::CPacketType::Transcription
                            }
                            $crate::streamkit_core::types::PacketType::Custom { .. } => {
                                $crate::types::CPacketType::Custom
                            }
                            $crate::streamkit_core::types::PacketType::Binary => {
                                $crate::types::CPacketType::Binary
                            }
                            $crate::streamkit_core::types::PacketType::Any => {
                                $crate::types::CPacketType::Any
                            }
                            $crate::streamkit_core::types::PacketType::Passthrough => {
                                $crate::types::CPacketType::Any
                            }
                        };

                        // SAFETY: We just pushed an element, so last() is guaranteed to be Some
                        #[allow(clippy::unwrap_used)]
                        let audio_format_ptr =
                            if let Some(ref fmt) = output_audio_formats.last().unwrap() {
                                fmt as *const $crate::types::CAudioFormat
                            } else {
                                std::ptr::null()
                            };

                        // SAFETY: We just pushed an element, so last() is guaranteed to be Some
                        #[allow(clippy::unwrap_used)]
                        let custom_type_id_ptr =
                            if let Some(ref s) = output_custom_type_ids.last().unwrap() {
                                s.as_ptr()
                            } else {
                                std::ptr::null()
                            };

                        let type_info = $crate::types::CPacketTypeInfo {
                            type_discriminant,
                            audio_format: audio_format_ptr,
                            custom_type_id: custom_type_id_ptr,
                        };

                        c_outputs.push($crate::types::COutputPin {
                            name: name.as_ptr(),
                            produces_type: type_info,
                        });
                        output_names.push(name);
                    }

                    // Convert categories
                    let mut category_strings = Vec::new();
                    let mut category_ptrs = Vec::new();

                    for cat in &meta.categories {
                        let c_str = std::ffi::CString::new(cat.as_str())
                            .expect("Category name should not contain null bytes");
                        category_ptrs.push(c_str.as_ptr());
                        category_strings.push(c_str);
                    }

                    let kind = std::ffi::CString::new(meta.kind.as_str())
                        .expect("Node kind should not contain null bytes");
                    let description = meta.description.as_ref().map(|d| {
                        std::ffi::CString::new(d.as_str())
                            .expect("Description should not contain null bytes")
                    });
                    let param_schema = std::ffi::CString::new(meta.param_schema.to_string())
                        .expect("Param schema JSON should not contain null bytes");

                    let c_metadata = $crate::types::CNodeMetadata {
                        kind: kind.as_ptr(),
                        description: description.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
                        inputs: c_inputs.as_ptr(),
                        inputs_count: c_inputs.len(),
                        outputs: c_outputs.as_ptr(),
                        outputs_count: c_outputs.len(),
                        param_schema: param_schema.as_ptr(),
                        categories: category_ptrs.as_ptr(),
                        categories_count: category_ptrs.len(),
                    };

                    (
                        c_metadata,
                        c_inputs,
                        c_outputs,
                        input_names,
                        input_types,
                        input_audio_formats,
                        input_custom_type_ids,
                        output_names,
                        output_audio_formats,
                        output_custom_type_ids,
                        category_strings,
                        category_ptrs,
                        kind,
                        description,
                        param_schema,
                    )
                });

                &metadata.0
            }
        }

        extern "C" fn __plugin_create_instance(
            params: *const std::os::raw::c_char,
            log_callback: $crate::types::CLogCallback,
            log_user_data: *mut std::os::raw::c_void,
        ) -> $crate::types::CPluginHandle {
            let params_json = if params.is_null() {
                None
            } else {
                match unsafe { $crate::conversions::c_str_to_string(params) } {
                    Ok(s) if s.is_empty() => None,
                    Ok(s) => match serde_json::from_str(&s) {
                        Ok(v) => Some(v),
                        Err(_) => return std::ptr::null_mut(),
                    },
                    Err(_) => return std::ptr::null_mut(),
                }
            };

            // Create logger for this plugin instance
            let logger = $crate::logger::Logger::new(log_callback, log_user_data, module_path!());

            match <$plugin_type as $crate::NativeProcessorNode>::new(params_json, logger) {
                Ok(instance) => Box::into_raw(Box::new(instance)) as $crate::types::CPluginHandle,
                Err(_) => std::ptr::null_mut(),
            }
        }

        extern "C" fn __plugin_process_packet(
            handle: $crate::types::CPluginHandle,
            input_pin: *const std::os::raw::c_char,
            packet: *const $crate::types::CPacket,
            output_callback: $crate::types::COutputCallback,
            callback_data: *mut std::os::raw::c_void,
            telemetry_callback: $crate::types::CTelemetryCallback,
            telemetry_callback_data: *mut std::os::raw::c_void,
        ) -> $crate::types::CResult {
            if handle.is_null() || input_pin.is_null() || packet.is_null() {
                return $crate::types::CResult::error(std::ptr::null());
            }

            let instance = unsafe { &mut *(handle as *mut $plugin_type) };

            let pin_name = match unsafe { $crate::conversions::c_str_to_string(input_pin) } {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = $crate::conversions::error_to_c(format!("Invalid pin name: {}", e));
                    return $crate::types::CResult::error(err_msg);
                }
            };

            let rust_packet = match unsafe { $crate::conversions::packet_from_c(packet) } {
                Ok(p) => p,
                Err(e) => {
                    let err_msg = $crate::conversions::error_to_c(format!("Invalid packet: {}", e));
                    return $crate::types::CResult::error(err_msg);
                }
            };

            let output = $crate::OutputSender::from_callbacks(
                output_callback,
                callback_data,
                telemetry_callback,
                telemetry_callback_data,
            );

            match instance.process(&pin_name, rust_packet, &output) {
                Ok(()) => $crate::types::CResult::success(),
                Err(e) => {
                    let err_msg = $crate::conversions::error_to_c(e);
                    $crate::types::CResult::error(err_msg)
                }
            }
        }

        extern "C" fn __plugin_update_params(
            handle: $crate::types::CPluginHandle,
            params: *const std::os::raw::c_char,
        ) -> $crate::types::CResult {
            if handle.is_null() {
                let err_msg = $crate::conversions::error_to_c("Invalid handle (null)");
                return $crate::types::CResult::error(err_msg);
            }

            let instance = unsafe { &mut *(handle as *mut $plugin_type) };

            let params_json = if params.is_null() {
                None
            } else {
                match unsafe { $crate::conversions::c_str_to_string(params) } {
                    Ok(s) if s.is_empty() => None,
                    Ok(s) => match serde_json::from_str(&s) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            let err_msg =
                                $crate::conversions::error_to_c(format!("Invalid params JSON: {e}"));
                            return $crate::types::CResult::error(err_msg);
                        },
                    },
                    Err(e) => {
                        let err_msg =
                            $crate::conversions::error_to_c(format!("Invalid params string: {e}"));
                        return $crate::types::CResult::error(err_msg);
                    },
                }
            };

            match instance.update_params(params_json) {
                Ok(()) => $crate::types::CResult::success(),
                Err(e) => {
                    let err_msg = $crate::conversions::error_to_c(e);
                    $crate::types::CResult::error(err_msg)
                },
            }
        }

        extern "C" fn __plugin_flush(
            handle: $crate::types::CPluginHandle,
            callback: $crate::types::COutputCallback,
            callback_data: *mut std::os::raw::c_void,
            telemetry_callback: $crate::types::CTelemetryCallback,
            telemetry_callback_data: *mut std::os::raw::c_void,
        ) -> $crate::types::CResult {
            tracing::info!("__plugin_flush called");
            if handle.is_null() {
                tracing::error!("Handle is null");
                let err_msg = $crate::conversions::error_to_c("Invalid handle (null)");
                return $crate::types::CResult::error(err_msg);
            }

            let instance = unsafe { &mut *(handle as *mut $plugin_type) };
            tracing::info!("Got instance pointer");

            // Create OutputSender wrapper for the callback
            let output_sender = $crate::OutputSender::from_callbacks(
                callback,
                callback_data,
                telemetry_callback,
                telemetry_callback_data,
            );
            tracing::info!("Created OutputSender, calling instance.flush()");

            match instance.flush(&output_sender) {
                Ok(()) => {
                    tracing::info!("instance.flush() returned Ok");
                    $crate::types::CResult::success()
                },
                Err(e) => {
                    tracing::error!(error = %e, "instance.flush() returned Err");
                    let err_msg = $crate::conversions::error_to_c(e);
                    $crate::types::CResult::error(err_msg)
                },
            }
        }

        extern "C" fn __plugin_destroy_instance(handle: $crate::types::CPluginHandle) {
            if !handle.is_null() {
                let mut instance = unsafe { Box::from_raw(handle as *mut $plugin_type) };
                instance.cleanup();
            }
        }
    };
}
