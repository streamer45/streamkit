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

/// Convert a [`CResult`] from a host callback into a Rust `Result`.
///
/// # Safety
///
/// If `result.error_message` is non-null it must point to a valid,
/// NUL-terminated C string.
unsafe fn result_from_c(result: types::CResult) -> Result<(), String> {
    if result.success {
        return Ok(());
    }
    let error_msg = if result.error_message.is_null() {
        "Unknown error".to_string()
    } else {
        conversions::c_str_to_string(result.error_message)
            .unwrap_or_else(|_| "Unknown error".to_string())
    };
    Err(error_msg)
}

pub use streamkit_core;
pub use types::*;

/// Re-export commonly used types
pub mod prelude {
    pub use crate::logger::Logger;
    pub use crate::types::{CLogCallback, CLogLevel};
    pub use crate::{
        native_plugin_entry, native_source_plugin_entry, plugin_debug, plugin_error, plugin_info,
        plugin_log, plugin_trace, plugin_warn, NativeProcessorNode, NativeSourceNode, NodeMetadata,
        OutputSender, PooledAudioBuffer, PooledVideoBuffer, ResourceSupport, SourceConfig,
    };
    pub use streamkit_core::types::{AudioFrame, Packet, PacketType};
    pub use streamkit_core::{InputPin, OutputPin, PinCardinality, Resource, UpstreamHint};
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

/// A video buffer allocated from the host's frame pool.
///
/// Follows linear-type semantics: after allocation the plugin must either
/// pass the buffer to [`OutputSender::send_video`] (which consumes it) or
/// let it drop (which calls `free_fn` to return the buffer to the pool).
pub struct PooledVideoBuffer {
    data: *mut u8,
    len: usize,
    handle: *mut std::os::raw::c_void,
    free_fn: extern "C" fn(*mut std::os::raw::c_void),
    consumed: bool,
}

impl PooledVideoBuffer {
    /// Writable slice into the pooled buffer.
    #[allow(clippy::missing_const_for_fn)] // Dereferences a heap pointer; will never be called in const context.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `data` was returned by the host's `alloc_video` callback
        // and is valid for `len` bytes. Exclusive access is guaranteed by
        // `&mut self` — no other reference exists.
        unsafe { std::slice::from_raw_parts_mut(self.data, self.len) }
    }

    /// Number of usable bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Mark the buffer as consumed (ownership transferred to the host).
    #[allow(clippy::missing_const_for_fn)] // Mutates runtime state; const context is meaningless here.
    fn consume(&mut self) -> (*mut std::os::raw::c_void, *const u8) {
        self.consumed = true;
        (self.handle, self.data)
    }
}

impl Drop for PooledVideoBuffer {
    fn drop(&mut self) {
        if !self.consumed {
            (self.free_fn)(self.handle);
        }
    }
}

/// An audio buffer allocated from the host's frame pool.
///
/// Same linear-type semantics as [`PooledVideoBuffer`].
pub struct PooledAudioBuffer {
    data: *mut f32,
    sample_count: usize,
    handle: *mut std::os::raw::c_void,
    free_fn: extern "C" fn(*mut std::os::raw::c_void),
    consumed: bool,
}

impl PooledAudioBuffer {
    /// Writable slice into the pooled buffer.
    #[allow(clippy::missing_const_for_fn)] // Dereferences a heap pointer; will never be called in const context.
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        // SAFETY: same as PooledVideoBuffer.
        unsafe { std::slice::from_raw_parts_mut(self.data, self.sample_count) }
    }

    /// Number of usable samples.
    #[must_use]
    pub const fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// Returns `true` if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sample_count == 0
    }

    /// Mark the buffer as consumed (ownership transferred to the host).
    #[allow(clippy::missing_const_for_fn)] // Mutates runtime state; const context is meaningless here.
    fn consume(&mut self) -> (*mut std::os::raw::c_void, *const f32) {
        self.consumed = true;
        (self.handle, self.data)
    }
}

impl Drop for PooledAudioBuffer {
    fn drop(&mut self) {
        if !self.consumed {
            (self.free_fn)(self.handle);
        }
    }
}

/// Output sender for sending packets to output pins
pub struct OutputSender {
    callbacks: *const types::CNodeCallbacks,
}

impl OutputSender {
    /// Create an `OutputSender` from a `CNodeCallbacks` pointer.
    ///
    /// # Safety
    ///
    /// The pointer must remain valid for the lifetime of this `OutputSender`.
    pub const unsafe fn from_node_callbacks(callbacks: *const types::CNodeCallbacks) -> Self {
        Self { callbacks }
    }

    /// Access the underlying callbacks.
    const fn cb(&self) -> &types::CNodeCallbacks {
        // SAFETY: The pointer is valid for the lifetime of this `OutputSender`,
        // guaranteed by the caller of `from_node_callbacks`.
        unsafe { &*self.callbacks }
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
        let cb = self.cb();

        let packet_repr = conversions::packet_to_c(packet);
        let result = (cb.output_callback)(
            pin_c.as_ptr(),
            &raw const packet_repr.packet,
            cb.output_user_data,
        );

        // SAFETY: CResult from host callback; error_message is a valid C string if non-null.
        unsafe { result_from_c(result) }
    }

    // ── Frame pool allocation ─────────────────────────────────────────────

    /// Allocate a video buffer from the host's frame pool.
    ///
    /// Returns `None` if the host has no video pool or allocation fails.
    pub fn alloc_video(&self, min_bytes: usize) -> Option<PooledVideoBuffer> {
        let cb = self.cb();
        let alloc_fn = cb.alloc_video?;
        let res = alloc_fn(min_bytes, cb.alloc_user_data);
        if res.data.is_null() || res.free_fn.is_none() {
            return None;
        }
        Some(PooledVideoBuffer {
            data: res.data,
            len: res.len,
            handle: res.handle,
            // SAFETY: free_fn is guaranteed to be Some by the check above.
            free_fn: unsafe { res.free_fn.unwrap_unchecked() },
            consumed: false,
        })
    }

    /// Allocate an audio buffer from the host's frame pool.
    ///
    /// Returns `None` if the host has no audio pool or allocation fails.
    pub fn alloc_audio(&self, min_samples: usize) -> Option<PooledAudioBuffer> {
        let cb = self.cb();
        let alloc_fn = cb.alloc_audio?;
        let res = alloc_fn(min_samples, cb.alloc_user_data);
        if res.data.is_null() || res.free_fn.is_none() {
            return None;
        }
        Some(PooledAudioBuffer {
            data: res.data,
            sample_count: res.sample_count,
            handle: res.handle,
            // SAFETY: free_fn is guaranteed to be Some by the check above.
            free_fn: unsafe { res.free_fn.unwrap_unchecked() },
            consumed: false,
        })
    }

    /// Send a video frame using a pool-allocated buffer (zero-copy path).
    ///
    /// Consumes `buf` — ownership transfers to the host.
    ///
    /// # Errors
    ///
    /// Returns an error if the pin name is invalid or the host rejects the
    /// packet.
    pub fn send_video(
        &self,
        pin: &str,
        width: u32,
        height: u32,
        pixel_format: streamkit_core::types::PixelFormat,
        mut buf: PooledVideoBuffer,
        metadata: Option<&streamkit_core::types::PacketMetadata>,
    ) -> Result<(), String> {
        let pin_c = CString::new(pin).map_err(|e| format!("Invalid pin name: {e}"))?;
        let cb = self.cb();

        let (handle, data_ptr) = buf.consume();

        let c_meta = metadata.map(conversions::metadata_to_c);
        let c_meta_ptr = c_meta.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);

        let c_frame = types::CVideoFrame {
            width,
            height,
            pixel_format: conversions::pixel_format_to_c(pixel_format),
            data: data_ptr,
            data_len: buf.len(),
            buffer_handle: handle,
            metadata: c_meta_ptr,
        };

        let c_pkt = types::CPacket {
            packet_type: types::CPacketType::RawVideo,
            data: std::ptr::from_ref(&c_frame).cast(),
            len: std::mem::size_of::<types::CVideoFrame>(),
        };

        let result = (cb.output_callback)(pin_c.as_ptr(), &raw const c_pkt, cb.output_user_data);
        // SAFETY: CResult from host callback; error_message is a valid C string if non-null.
        unsafe { result_from_c(result) }
    }

    /// Send an audio frame using a pool-allocated buffer (zero-copy path).
    ///
    /// Consumes `buf` — ownership transfers to the host.
    ///
    /// # Errors
    ///
    /// Returns an error if the pin name is invalid or the host rejects the
    /// packet.
    pub fn send_audio(
        &self,
        pin: &str,
        sample_rate: u32,
        channels: u16,
        mut buf: PooledAudioBuffer,
        metadata: Option<&streamkit_core::types::PacketMetadata>,
    ) -> Result<(), String> {
        let pin_c = CString::new(pin).map_err(|e| format!("Invalid pin name: {e}"))?;
        let cb = self.cb();

        let (handle, data_ptr) = buf.consume();

        let c_meta = metadata.map(conversions::metadata_to_c);
        let c_meta_ptr = c_meta.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);

        let c_frame = types::CAudioFrame {
            sample_rate,
            channels,
            samples: data_ptr,
            sample_count: buf.sample_count(),
            buffer_handle: handle,
            metadata: c_meta_ptr,
        };

        let c_pkt = types::CPacket {
            packet_type: types::CPacketType::RawAudio,
            data: std::ptr::from_ref(&c_frame).cast(),
            len: std::mem::size_of::<types::CAudioFrame>(),
        };

        let result = (cb.output_callback)(pin_c.as_ptr(), &raw const c_pkt, cb.output_user_data);
        // SAFETY: CResult from host callback; error_message is a valid C string if non-null.
        unsafe { result_from_c(result) }
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
        let cb = self.cb();
        let Some(telemetry_cb) = cb.telemetry_callback else {
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

        let result = telemetry_cb(
            event_type_c.as_ptr(),
            data_json.as_ptr(),
            data_json.len(),
            meta_ptr,
            cb.telemetry_user_data,
        );

        // SAFETY: CResult from host callback; error_message is a valid C string if non-null.
        unsafe { result_from_c(result) }
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

    /// Return a runtime-discovered param schema after initialization (optional).
    ///
    /// Plugins whose tunable parameters depend on runtime configuration
    /// (e.g., properties discovered after compiling a `.slint` file) can
    /// override this to return a JSON Schema fragment.  The engine will
    /// deep-merge it with the static `param_schema` from metadata and
    /// deliver the enriched schema to the UI.
    ///
    /// Default: `None` (use static schema only).
    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        None
    }
}

/// Configuration for a source node's tick loop.
///
/// Returned by [`NativeSourceNode::source_config`] to tell the host how
/// frequently to call [`NativeSourceNode::tick`].
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// Microseconds between ticks (e.g. 33_333 for ~30 fps).
    pub tick_interval_us: u64,
    /// If > 0, host stops after this many ticks. 0 = infinite.
    pub max_ticks: u64,
}

impl SourceConfig {
    /// Create a config for a given frames-per-second rate (infinite ticks).
    pub fn from_fps(fps: u32) -> Self {
        Self { tick_interval_us: 1_000_000 / u64::from(fps.max(1)), max_ticks: 0 }
    }

    /// Create a config with an explicit interval in microseconds (infinite ticks).
    pub const fn from_interval_us(us: u64) -> Self {
        Self { tick_interval_us: us, max_ticks: 0 }
    }
}

/// Trait for source plugins — nodes with **no inputs** that produce output
/// on a host-driven tick schedule.
///
/// Instead of receiving packets via `process()`, source nodes implement
/// [`tick()`](NativeSourceNode::tick) which the host calls at the interval
/// specified by [`source_config()`](NativeSourceNode::source_config).
///
/// # Example
///
/// ```no_run
/// use streamkit_plugin_sdk_native::prelude::*;
///
/// pub struct MySource {
///     frame_count: u64,
/// }
///
/// impl NativeSourceNode for MySource {
///     fn metadata() -> NodeMetadata {
///         NodeMetadata::builder("my_source")
///             .output("video", PacketType::Binary)
///             .category("source")
///             .build()
///     }
///
///     fn source_config(&self) -> SourceConfig {
///         SourceConfig::from_fps(30)
///     }
///
///     fn new(params: Option<serde_json::Value>, _logger: Logger) -> Result<Self, String> {
///         Ok(Self { frame_count: 0 })
///     }
///
///     fn tick(&mut self, output: &OutputSender) -> Result<bool, String> {
///         self.frame_count += 1;
///         // produce output via output.send(...)
///         Ok(false) // false = keep going, true = done
///     }
/// }
///
/// native_source_plugin_entry!(MySource);
/// ```
pub trait NativeSourceNode: Sized + Send + 'static {
    /// Return metadata about this node type.
    ///
    /// Source nodes typically have **no inputs** and one or more outputs.
    fn metadata() -> NodeMetadata;

    /// Return the tick configuration for this source.
    fn source_config(&self) -> SourceConfig;

    /// Create a new instance of the source node.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (e.g., invalid parameters).
    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String>;

    /// Produce one unit of output.
    ///
    /// Called by the host at the interval specified by [`source_config`].
    /// Return `Ok(false)` to keep ticking, `Ok(true)` to signal completion.
    ///
    /// # Errors
    ///
    /// Returns an error if producing output fails.
    fn tick(&mut self, output: &OutputSender) -> Result<bool, String>;

    /// Update runtime parameters (optional).
    ///
    /// # Errors
    ///
    /// Returns an error if parameter update fails (e.g., invalid values).
    fn update_params(&mut self, _params: Option<serde_json::Value>) -> Result<(), String> {
        Ok(())
    }

    /// Clean up resources (optional).
    fn cleanup(&mut self) {}

    /// Return a runtime-discovered param schema after initialization (optional).
    ///
    /// Source plugins whose tunable parameters depend on runtime configuration
    /// (e.g., properties discovered after compiling a `.slint` file) can
    /// override this to return a JSON Schema fragment.  The engine will
    /// deep-merge it with the static `param_schema` from metadata and
    /// deliver the enriched schema to the UI.
    ///
    /// Default: `None` (use static schema only).
    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Called when a downstream consumer sends an advisory hint.
    ///
    /// The default implementation ignores all hints.  Source plugins
    /// that support resolution-independent rendering (e.g. Slint) can
    /// override this to resize their output.
    fn on_upstream_hint(&mut self, _hint: streamkit_core::UpstreamHint) {
        // default: ignore
    }
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

/// Internal helper macro: generates `__plugin_get_runtime_param_schema` and
/// `__plugin_destroy_instance` trampolines.  Shared by both
/// `native_plugin_entry!` and `native_source_plugin_entry!` to avoid
/// identical duplicated implementations.
#[macro_export]
#[doc(hidden)]
macro_rules! __plugin_shared_ffi {
    ($plugin_type:ty) => {
        extern "C" fn __plugin_get_runtime_param_schema(
            handle: $crate::types::CPluginHandle,
        ) -> $crate::types::CSchemaResult {
            if handle.is_null() {
                return $crate::types::CSchemaResult::none();
            }

            let instance = unsafe { &*(handle as *const $plugin_type) };
            match instance.runtime_param_schema() {
                None => $crate::types::CSchemaResult::none(),
                Some(schema) => match serde_json::to_string(&schema) {
                    Ok(json) => {
                        // NOTE: error_to_c is a misnomer here — it's a generic
                        // "String → thread-local CString" helper reused for the
                        // success payload.  A rename to e.g. `thread_local_c_str`
                        // would clarify intent but touches many call-sites.
                        let c_str = $crate::conversions::error_to_c(json);
                        $crate::types::CSchemaResult::schema(c_str)
                    },
                    Err(e) => {
                        let err_msg = $crate::conversions::error_to_c(format!(
                            "Failed to serialize runtime param schema: {e}"
                        ));
                        $crate::types::CSchemaResult::error(err_msg)
                    },
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
            Vec<Vec<Option<$crate::types::CRawVideoFormat>>>,
            Vec<std::ffi::CString>,
            Vec<Option<$crate::types::CAudioFormat>>,
            Vec<Option<std::ffi::CString>>,
            Vec<Option<$crate::types::CRawVideoFormat>>,
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
                get_source_config: None,
                tick: None,
                get_runtime_param_schema: Some(__plugin_get_runtime_param_schema),
                on_upstream_hint: None,
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
                    let mut input_video_formats = Vec::new();

                    for input in &meta.inputs {
                        let name = std::ffi::CString::new(input.name.as_str())
                            .expect("Input pin name should not contain null bytes");
                        let mut types_info = Vec::new();
                        let mut audio_formats = Vec::new();
                        let mut custom_type_ids = Vec::new();
                        let mut video_formats = Vec::new();

                        for pt in &input.accepts_types {
                            let audio_format = match pt {
                                $crate::streamkit_core::types::PacketType::RawAudio(af) => {
                                    Some($crate::conversions::audio_format_to_c(af))
                                }
                                _ => None,
                            };
                            audio_formats.push(audio_format);
                            let custom_type_id = match pt {
                                $crate::streamkit_core::types::PacketType::Custom { type_id } => {
                                    Some(std::ffi::CString::new(type_id.as_str()).expect(
                                        "Custom type_id should not contain null bytes",
                                    ))
                                }
                                $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                    Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                                }
                                $crate::streamkit_core::types::PacketType::EncodedVideo(format) => {
                                    Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                                }
                                _ => None,
                            };
                            custom_type_ids.push(custom_type_id);
                            let video_format = match pt {
                                $crate::streamkit_core::types::PacketType::RawVideo(vf) => {
                                    Some($crate::conversions::raw_video_format_to_c(vf))
                                }
                                _ => None,
                            };
                            video_formats.push(video_format);
                        }

                        // Now create CPacketTypeInfo with stable pointers to the stored formats
                        for (idx, pt) in input.accepts_types.iter().enumerate() {
                            let type_discriminant = match pt {
                                $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                    $crate::types::CPacketType::RawAudio
                                }
                                $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                    if format.codec
                                        == $crate::streamkit_core::types::AudioCodec::Opus
                                        && format.codec_private.is_none()
                                    {
                                        $crate::types::CPacketType::OpusAudio
                                    } else {
                                        $crate::types::CPacketType::EncodedAudio
                                    }
                                }
                                $crate::streamkit_core::types::PacketType::RawVideo(_) => {
                                    $crate::types::CPacketType::RawVideo
                                }
                                $crate::streamkit_core::types::PacketType::EncodedVideo(_) => {
                                    $crate::types::CPacketType::EncodedVideo
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

                            let audio_format_ptr = if let Some(ref fmt) = audio_formats[idx] {
                                fmt as *const $crate::types::CAudioFormat
                            } else {
                                std::ptr::null()
                            };

                            let custom_type_id_ptr = if let Some(ref s) = custom_type_ids[idx] {
                                s.as_ptr()
                            } else {
                                std::ptr::null()
                            };

                            let video_format_ptr = if let Some(ref vf) = video_formats[idx] {
                                vf as *const $crate::types::CRawVideoFormat
                            } else {
                                std::ptr::null()
                            };

                            types_info.push($crate::types::CPacketTypeInfo {
                                type_discriminant,
                                audio_format: audio_format_ptr,
                                custom_type_id: custom_type_id_ptr,
                                raw_video_format: video_format_ptr,
                            });
                        }

                        c_inputs.push($crate::types::CInputPin {
                            name: name.as_ptr(),
                            accepts_types: types_info.as_ptr(),
                            accepts_types_count: types_info.len(),
                        });

                        input_names.push(name);
                        input_types.push(types_info);
                        input_audio_formats.push(audio_formats);
                        input_custom_type_ids.push(custom_type_ids);
                        input_video_formats.push(video_formats);
                    }

                    // Convert outputs
                    let mut c_outputs = Vec::new();
                    let mut output_names = Vec::new();
                    let mut output_audio_formats = Vec::new();
                    let mut output_custom_type_ids = Vec::new();
                    let mut output_video_formats = Vec::new();

                    for output in &meta.outputs {
                        let name = std::ffi::CString::new(output.name.as_str())
                            .expect("Output pin name should not contain null bytes");

                        let audio_format = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawAudio(af) => {
                                Some($crate::conversions::audio_format_to_c(af))
                            }
                            _ => None,
                        };
                        output_audio_formats.push(audio_format);
                        let output_custom_type_id = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::Custom { type_id } => {
                                Some(std::ffi::CString::new(type_id.as_str()).expect(
                                    "Custom type_id should not contain null bytes",
                                ))
                            }
                            $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                            }
                            $crate::streamkit_core::types::PacketType::EncodedVideo(format) => {
                                Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                            }
                            _ => None,
                        };
                        output_custom_type_ids.push(output_custom_type_id);
                        let video_format = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawVideo(vf) => {
                                Some($crate::conversions::raw_video_format_to_c(vf))
                            }
                            _ => None,
                        };
                        output_video_formats.push(video_format);

                        // Now create CPacketTypeInfo with stable pointer to the stored format
                        let type_discriminant = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                $crate::types::CPacketType::RawAudio
                            }
                            $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                if format.codec
                                    == $crate::streamkit_core::types::AudioCodec::Opus
                                    && format.codec_private.is_none()
                                {
                                    $crate::types::CPacketType::OpusAudio
                                } else {
                                    $crate::types::CPacketType::EncodedAudio
                                }
                            }
                            $crate::streamkit_core::types::PacketType::RawVideo(_) => {
                                $crate::types::CPacketType::RawVideo
                            }
                            $crate::streamkit_core::types::PacketType::EncodedVideo(_) => {
                                $crate::types::CPacketType::EncodedVideo
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

                        // SAFETY: We just pushed an element, so last() is guaranteed to be Some
                        #[allow(clippy::unwrap_used)]
                        let video_format_ptr =
                            if let Some(ref vf) = output_video_formats.last().unwrap() {
                                vf as *const $crate::types::CRawVideoFormat
                            } else {
                                std::ptr::null()
                            };

                        let type_info = $crate::types::CPacketTypeInfo {
                            type_discriminant,
                            audio_format: audio_format_ptr,
                            custom_type_id: custom_type_id_ptr,
                            raw_video_format: video_format_ptr,
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
                        input_video_formats,
                        output_names,
                        output_audio_formats,
                        output_custom_type_ids,
                        output_video_formats,
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
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            if handle.is_null() || input_pin.is_null() || packet.is_null() || callbacks.is_null() {
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

            let output = unsafe { $crate::OutputSender::from_node_callbacks(callbacks) };

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
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            tracing::trace!("__plugin_flush called");
            if handle.is_null() || callbacks.is_null() {
                tracing::error!("Handle or callbacks is null");
                let err_msg = $crate::conversions::error_to_c("Invalid handle or callbacks (null)");
                return $crate::types::CResult::error(err_msg);
            }

            let instance = unsafe { &mut *(handle as *mut $plugin_type) };
            tracing::trace!("Got instance pointer");

            let output_sender = unsafe { $crate::OutputSender::from_node_callbacks(callbacks) };
            tracing::trace!("Created OutputSender, calling instance.flush()");

            match instance.flush(&output_sender) {
                Ok(()) => {
                    tracing::trace!("instance.flush() returned Ok");
                    $crate::types::CResult::success()
                },
                Err(e) => {
                    tracing::error!(error = %e, "instance.flush() returned Err");
                    let err_msg = $crate::conversions::error_to_c(e);
                    $crate::types::CResult::error(err_msg)
                },
            }
        }

        $crate::__plugin_shared_ffi!($plugin_type);
    };
}

/// Macro to generate C ABI exports for a **source** plugin.
///
/// Source plugins have no inputs and produce output via a host-driven tick
/// loop.  This macro should be called once per plugin with the type that
/// implements [`NativeSourceNode`].
///
/// The generated API struct sets `get_source_config` and `tick` to the
/// appropriate C-ABI trampolines, while `process_packet` and `flush` are
/// no-ops (source nodes don't receive input packets).
///
/// # Example
/// ```no_run
/// # use streamkit_plugin_sdk_native::prelude::*;
/// # struct MySource;
/// # impl NativeSourceNode for MySource {
/// #     fn metadata() -> NodeMetadata { unimplemented!() }
/// #     fn source_config(&self) -> SourceConfig { unimplemented!() }
/// #     fn new(_: Option<serde_json::Value>, _: Logger) -> Result<Self, String> { unimplemented!() }
/// #     fn tick(&mut self, _: &OutputSender) -> Result<bool, String> { unimplemented!() }
/// # }
/// native_source_plugin_entry!(MySource);
/// ```
#[macro_export]
macro_rules! native_source_plugin_entry {
    ($plugin_type:ty) => {
        // Static metadata storage (same layout as processor macro + video format vecs)
        static mut METADATA: std::sync::OnceLock<(
            $crate::types::CNodeMetadata,
            Vec<$crate::types::CInputPin>,
            Vec<$crate::types::COutputPin>,
            Vec<std::ffi::CString>,
            Vec<Vec<$crate::types::CPacketTypeInfo>>,
            Vec<Vec<Option<$crate::types::CAudioFormat>>>,
            Vec<Vec<Option<std::ffi::CString>>>,
            Vec<Vec<Option<$crate::types::CRawVideoFormat>>>,
            Vec<std::ffi::CString>,
            Vec<Option<$crate::types::CAudioFormat>>,
            Vec<Option<std::ffi::CString>>,
            Vec<Option<$crate::types::CRawVideoFormat>>,
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
                process_packet: __plugin_process_packet_noop,
                update_params: __plugin_update_params,
                flush: __plugin_flush_noop,
                destroy_instance: __plugin_destroy_instance,
                get_source_config: Some(__plugin_get_source_config),
                tick: Some(__plugin_tick),
                get_runtime_param_schema: Some(__plugin_get_runtime_param_schema),
                on_upstream_hint: Some(__plugin_on_upstream_hint),
            };
            &API
        }

        // ── Metadata ────────────────────────────────────────────────────
        // Reuse the same metadata-building logic as the processor macro.
        // Source nodes typically have zero inputs and one or more outputs.

        extern "C" fn __plugin_get_metadata() -> *const $crate::types::CNodeMetadata {
            unsafe {
                let metadata = METADATA.get_or_init(|| {
                    let meta = <$plugin_type as $crate::NativeSourceNode>::metadata();

                    // Convert inputs (usually empty for source nodes)
                    let mut c_inputs = Vec::new();
                    let mut input_names = Vec::new();
                    let mut input_types = Vec::new();
                    let mut input_audio_formats = Vec::new();
                    let mut input_custom_type_ids = Vec::new();
                    let mut input_video_formats = Vec::new();

                    for input in &meta.inputs {
                        let name = std::ffi::CString::new(input.name.as_str())
                            .expect("Input pin name should not contain null bytes");
                        let mut types_info = Vec::new();
                        let mut audio_formats = Vec::new();
                        let mut custom_type_ids = Vec::new();
                        let mut video_formats = Vec::new();

                        for pt in &input.accepts_types {
                            let audio_format = match pt {
                                $crate::streamkit_core::types::PacketType::RawAudio(af) => {
                                    Some($crate::conversions::audio_format_to_c(af))
                                },
                                _ => None,
                            };
                            audio_formats.push(audio_format);
                            let custom_type_id = match pt {
                                $crate::streamkit_core::types::PacketType::Custom { type_id } => {
                                    Some(
                                        std::ffi::CString::new(type_id.as_str())
                                            .expect("Custom type_id should not contain null bytes"),
                                    )
                                },
                                $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                    Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                                },
                                $crate::streamkit_core::types::PacketType::EncodedVideo(format) => {
                                    Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                                },
                                _ => None,
                            };
                            custom_type_ids.push(custom_type_id);
                            let video_format = match pt {
                                $crate::streamkit_core::types::PacketType::RawVideo(vf) => {
                                    Some($crate::conversions::raw_video_format_to_c(vf))
                                },
                                _ => None,
                            };
                            video_formats.push(video_format);
                        }

                        for (idx, pt) in input.accepts_types.iter().enumerate() {
                            let type_discriminant = match pt {
                                $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                    $crate::types::CPacketType::RawAudio
                                },
                                $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                    if format.codec
                                        == $crate::streamkit_core::types::AudioCodec::Opus
                                        && format.codec_private.is_none()
                                    {
                                        $crate::types::CPacketType::OpusAudio
                                    } else {
                                        $crate::types::CPacketType::EncodedAudio
                                    }
                                },
                                $crate::streamkit_core::types::PacketType::RawVideo(_) => {
                                    $crate::types::CPacketType::RawVideo
                                },
                                $crate::streamkit_core::types::PacketType::EncodedVideo(_) => {
                                    $crate::types::CPacketType::EncodedVideo
                                },
                                $crate::streamkit_core::types::PacketType::Text => {
                                    $crate::types::CPacketType::Text
                                },
                                $crate::streamkit_core::types::PacketType::Transcription => {
                                    $crate::types::CPacketType::Transcription
                                },
                                $crate::streamkit_core::types::PacketType::Custom { .. } => {
                                    $crate::types::CPacketType::Custom
                                },
                                $crate::streamkit_core::types::PacketType::Binary => {
                                    $crate::types::CPacketType::Binary
                                },
                                $crate::streamkit_core::types::PacketType::Any => {
                                    $crate::types::CPacketType::Any
                                },
                                $crate::streamkit_core::types::PacketType::Passthrough => {
                                    $crate::types::CPacketType::Any
                                },
                            };

                            let audio_format_ptr = if let Some(ref fmt) = audio_formats[idx] {
                                fmt as *const $crate::types::CAudioFormat
                            } else {
                                std::ptr::null()
                            };

                            let custom_type_id_ptr = if let Some(ref s) = custom_type_ids[idx] {
                                s.as_ptr()
                            } else {
                                std::ptr::null()
                            };

                            let video_format_ptr = if let Some(ref vf) = video_formats[idx] {
                                vf as *const $crate::types::CRawVideoFormat
                            } else {
                                std::ptr::null()
                            };

                            types_info.push($crate::types::CPacketTypeInfo {
                                type_discriminant,
                                audio_format: audio_format_ptr,
                                custom_type_id: custom_type_id_ptr,
                                raw_video_format: video_format_ptr,
                            });
                        }

                        c_inputs.push($crate::types::CInputPin {
                            name: name.as_ptr(),
                            accepts_types: types_info.as_ptr(),
                            accepts_types_count: types_info.len(),
                        });

                        input_names.push(name);
                        input_types.push(types_info);
                        input_audio_formats.push(audio_formats);
                        input_custom_type_ids.push(custom_type_ids);
                        input_video_formats.push(video_formats);
                    }

                    // Convert outputs
                    let mut c_outputs = Vec::new();
                    let mut output_names = Vec::new();
                    let mut output_audio_formats = Vec::new();
                    let mut output_custom_type_ids = Vec::new();
                    let mut output_video_formats = Vec::new();

                    for output in &meta.outputs {
                        let name = std::ffi::CString::new(output.name.as_str())
                            .expect("Output pin name should not contain null bytes");

                        let audio_format = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawAudio(af) => {
                                Some($crate::conversions::audio_format_to_c(af))
                            },
                            _ => None,
                        };
                        output_audio_formats.push(audio_format);
                        let output_custom_type_id = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::Custom { type_id } => Some(
                                std::ffi::CString::new(type_id.as_str())
                                    .expect("Custom type_id should not contain null bytes"),
                            ),
                            $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                            },
                            $crate::streamkit_core::types::PacketType::EncodedVideo(format) => {
                                Some($crate::conversions::codec_name_to_cstring(format.codec.as_c_name()))
                            },
                            _ => None,
                        };
                        output_custom_type_ids.push(output_custom_type_id);
                        let video_format = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawVideo(vf) => {
                                Some($crate::conversions::raw_video_format_to_c(vf))
                            },
                            _ => None,
                        };
                        output_video_formats.push(video_format);

                        let type_discriminant = match &output.produces_type {
                            $crate::streamkit_core::types::PacketType::RawAudio(_) => {
                                $crate::types::CPacketType::RawAudio
                            },
                            $crate::streamkit_core::types::PacketType::EncodedAudio(format) => {
                                if format.codec == $crate::streamkit_core::types::AudioCodec::Opus
                                    && format.codec_private.is_none()
                                {
                                    $crate::types::CPacketType::OpusAudio
                                } else {
                                    $crate::types::CPacketType::EncodedAudio
                                }
                            },
                            $crate::streamkit_core::types::PacketType::RawVideo(_) => {
                                $crate::types::CPacketType::RawVideo
                            },
                            $crate::streamkit_core::types::PacketType::EncodedVideo(_) => {
                                $crate::types::CPacketType::EncodedVideo
                            },
                            $crate::streamkit_core::types::PacketType::Text => {
                                $crate::types::CPacketType::Text
                            },
                            $crate::streamkit_core::types::PacketType::Transcription => {
                                $crate::types::CPacketType::Transcription
                            },
                            $crate::streamkit_core::types::PacketType::Custom { .. } => {
                                $crate::types::CPacketType::Custom
                            },
                            $crate::streamkit_core::types::PacketType::Binary => {
                                $crate::types::CPacketType::Binary
                            },
                            $crate::streamkit_core::types::PacketType::Any => {
                                $crate::types::CPacketType::Any
                            },
                            $crate::streamkit_core::types::PacketType::Passthrough => {
                                $crate::types::CPacketType::Any
                            },
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

                        // SAFETY: We just pushed an element, so last() is guaranteed to be Some
                        #[allow(clippy::unwrap_used)]
                        let video_format_ptr =
                            if let Some(ref vf) = output_video_formats.last().unwrap() {
                                vf as *const $crate::types::CRawVideoFormat
                            } else {
                                std::ptr::null()
                            };

                        let type_info = $crate::types::CPacketTypeInfo {
                            type_discriminant,
                            audio_format: audio_format_ptr,
                            custom_type_id: custom_type_id_ptr,
                            raw_video_format: video_format_ptr,
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
                        input_video_formats,
                        output_names,
                        output_audio_formats,
                        output_custom_type_ids,
                        output_video_formats,
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

        // ── Instance lifecycle ──────────────────────────────────────────

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

            let logger = $crate::logger::Logger::new(log_callback, log_user_data, module_path!());

            match <$plugin_type as $crate::NativeSourceNode>::new(params_json, logger) {
                Ok(instance) => Box::into_raw(Box::new(instance)) as $crate::types::CPluginHandle,
                Err(_) => std::ptr::null_mut(),
            }
        }

        // ── Source-specific entry points ─────────────────────────────────

        extern "C" fn __plugin_get_source_config(
            handle: $crate::types::CPluginHandle,
        ) -> $crate::types::CSourceConfig {
            if handle.is_null() {
                return $crate::types::CSourceConfig {
                    is_source: false,
                    tick_interval_us: 0,
                    max_ticks: 0,
                };
            }
            let instance = unsafe { &*(handle as *const $plugin_type) };
            let cfg = instance.source_config();
            $crate::types::CSourceConfig {
                is_source: true,
                tick_interval_us: cfg.tick_interval_us,
                max_ticks: cfg.max_ticks,
            }
        }

        extern "C" fn __plugin_tick(
            handle: $crate::types::CPluginHandle,
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CTickResult {
            if handle.is_null() || callbacks.is_null() {
                let err = $crate::conversions::error_to_c("Invalid handle or callbacks (null)");
                return $crate::types::CTickResult::error(err);
            }

            let instance = unsafe { &mut *(handle as *mut $plugin_type) };
            let output = unsafe { $crate::OutputSender::from_node_callbacks(callbacks) };

            match instance.tick(&output) {
                Ok(done) => {
                    if done {
                        $crate::types::CTickResult::done()
                    } else {
                        $crate::types::CTickResult::ok()
                    }
                },
                Err(e) => {
                    let err = $crate::conversions::error_to_c(e);
                    $crate::types::CTickResult::error(err)
                },
            }
        }

        // ── No-op processor stubs (required by CNativePluginAPI) ────────

        extern "C" fn __plugin_process_packet_noop(
            _handle: $crate::types::CPluginHandle,
            _input_pin: *const std::os::raw::c_char,
            _packet: *const $crate::types::CPacket,
            _callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            let err = $crate::conversions::error_to_c(
                "process_packet called on source plugin (not supported)",
            );
            $crate::types::CResult::error(err)
        }

        extern "C" fn __plugin_flush_noop(
            _handle: $crate::types::CPluginHandle,
            _callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            $crate::types::CResult::success()
        }

        // ── Upstream hint delivery (v5) ─────────────────────────────────

        extern "C" fn __plugin_on_upstream_hint(
            handle: $crate::types::CPluginHandle,
            hint_json: *const std::os::raw::c_char,
        ) -> $crate::types::CResult {
            if handle.is_null() {
                let err = $crate::conversions::error_to_c("Invalid handle (null)");
                return $crate::types::CResult::error(err);
            }
            let hint_str = match unsafe { $crate::conversions::c_str_to_string(hint_json) } {
                Ok(s) => s,
                Err(e) => {
                    let err = $crate::conversions::error_to_c(format!("Invalid hint JSON: {e}"));
                    return $crate::types::CResult::error(err);
                },
            };
            let hint: $crate::streamkit_core::UpstreamHint = match serde_json::from_str(&hint_str) {
                Ok(h) => h,
                Err(e) => {
                    let err = $crate::conversions::error_to_c(format!("Failed to parse hint: {e}"));
                    return $crate::types::CResult::error(err);
                },
            };
            let instance = unsafe { &mut *(handle as *mut $plugin_type) };
            instance.on_upstream_hint(hint);
            $crate::types::CResult::success()
        }

        // ── Shared ──────────────────────────────────────────────────────

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
                            let err_msg = $crate::conversions::error_to_c(format!(
                                "Invalid params JSON: {e}"
                            ));
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

        $crate::__plugin_shared_ffi!($plugin_type);
    };
}
