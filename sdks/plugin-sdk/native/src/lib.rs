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
pub mod ffi_guard;
pub mod logger;
pub mod metadata_storage;
pub mod resource_cache;
pub mod types;

use std::ffi::CString;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{InputPin, OutputPin, PinCardinality};

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
    pub use crate::resource_cache::{CacheError, ResourceCache};
    pub use crate::types::{CLogCallback, CLogLevel};
    pub use crate::{
        native_plugin_entry, native_source_plugin_entry, plugin_debug, plugin_error, plugin_info,
        plugin_log, plugin_trace, plugin_warn, NativeProcessorNode, NativeSourceNode, NodeMetadata,
        OutputSender, PooledAudioBuffer, PooledVideoBuffer, SourceConfig,
    };
    pub use streamkit_core::types::{AudioFrame, Packet, PacketType};
    pub use streamkit_core::{InputPin, OutputPin, PinCardinality, UpstreamHint};
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

    /// Check if a callback field at the given byte offset is within the
    /// host-provided `CNodeCallbacks` struct.  Returns `false` when the
    /// host is older and doesn't include this field.
    #[allow(clippy::missing_const_for_fn)] // Not const because self.cb() dereferences a raw pointer.
    fn callback_available(&self, field_end_offset: usize) -> bool {
        self.cb().struct_size >= field_end_offset
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
        let end = std::mem::offset_of!(types::CNodeCallbacks, alloc_video)
            + std::mem::size_of::<Option<types::CAllocVideoFn>>();
        if !self.callback_available(end) {
            return None;
        }
        let cb = self.cb();
        let alloc_fn = cb.alloc_video?;
        let res = alloc_fn(min_bytes, cb.alloc_user_data);
        let free_fn = res.free_fn?;
        if res.data.is_null() {
            return None;
        }
        Some(PooledVideoBuffer {
            data: res.data,
            len: res.len,
            handle: res.handle,
            free_fn,
            consumed: false,
        })
    }

    /// Allocate an audio buffer from the host's frame pool.
    ///
    /// Returns `None` if the host has no audio pool or allocation fails.
    pub fn alloc_audio(&self, min_samples: usize) -> Option<PooledAudioBuffer> {
        let end = std::mem::offset_of!(types::CNodeCallbacks, alloc_audio)
            + std::mem::size_of::<Option<types::CAllocAudioFn>>();
        if !self.callback_available(end) {
            return None;
        }
        let cb = self.cb();
        let alloc_fn = cb.alloc_audio?;
        let res = alloc_fn(min_samples, cb.alloc_user_data);
        let free_fn = res.free_fn?;
        if res.data.is_null() {
            return None;
        }
        Some(PooledAudioBuffer {
            data: res.data,
            sample_count: res.sample_count,
            handle: res.handle,
            free_fn,
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

    /// Clean up resources (optional).
    ///
    /// # Panics
    ///
    /// This method **must not panic**.  It runs inside a `catch_unwind`
    /// guard, but the plugin value is dropped immediately afterwards.
    /// If both `cleanup()` and the type's `Drop` impl panic, the process
    /// aborts (Rust double-panic rule).
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

    /// Return a mutable reference to the plugin's [`Logger`] (v9).
    ///
    /// Override this to enable the host's log-enabled callback, allowing
    /// `plugin_trace!` / `plugin_debug!` / etc. to short-circuit before
    /// formatting when the level is disabled by the tracing subscriber.
    ///
    /// The host calls `set_log_enabled_callback` immediately after instance
    /// creation; the SDK trampoline uses this method to inject the callback.
    ///
    /// **Clone caveat:** If the plugin clones the [`Logger`] before the
    /// host injects the callback (i.e. during `create`), those clones
    /// will **not** see the enabled callback.  Recommended patterns for
    /// multi-threaded plugins:
    ///
    /// - **Single owner + shared ref:** Store the `Logger` in the plugin
    ///   struct and pass `&Logger` to spawned tasks (requires scoped
    ///   threads or an `Arc<Mutex<Logger>>`).
    /// - **Re-clone after create:** Clone the logger only after `create`
    ///   returns, at which point the callback is already injected.
    /// - **`Arc<Mutex<Logger>>`:** Wrap in a mutex so all threads see
    ///   the injected callback.  The lock is uncontended in practice
    ///   (only `logger_mut` needs `&mut`).
    ///
    /// Avoid `Arc<Logger>` (without interior mutability) — `logger_mut`
    /// cannot reach through an `Arc` to inject the callback.
    ///
    /// Default: `None` (no short-circuit — all levels always "enabled").
    fn logger_mut(&mut self) -> Option<&mut Logger> {
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
    ///
    /// # Panics
    ///
    /// This method **must not panic**.  It runs inside a `catch_unwind`
    /// guard, but the plugin value is dropped immediately afterwards.
    /// If both `cleanup()` and the type's `Drop` impl panic, the process
    /// aborts (Rust double-panic rule).
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

    /// Return a mutable reference to the plugin's [`Logger`] (v9).
    ///
    /// Override this to enable the host's log-enabled callback, allowing
    /// `plugin_trace!` / `plugin_debug!` / etc. to short-circuit before
    /// formatting when the level is disabled by the tracing subscriber.
    ///
    /// **Clone caveat:** If the plugin clones the [`Logger`] before the
    /// host injects the callback (i.e. during `create`), those clones
    /// will **not** see the enabled callback.  See
    /// [`NativeProcessorNode::logger_mut`] for recommended multi-thread
    /// patterns (`Arc<Mutex<Logger>>`, re-clone after create, etc.).
    ///
    /// Default: `None` (no short-circuit — all levels always "enabled").
    fn logger_mut(&mut self) -> Option<&mut Logger> {
        None
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
            $crate::ffi_guard::guard_schema(|| {
                if handle.is_null() {
                    return $crate::types::CSchemaResult::none();
                }

                let instance = unsafe { &*(handle as *const $plugin_type) };
                match instance.runtime_param_schema() {
                    None => $crate::types::CSchemaResult::none(),
                    Some(schema) => match serde_json::to_string(&schema) {
                        Ok(json) => {
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
            })
        }

        extern "C" fn __plugin_destroy_instance(handle: $crate::types::CPluginHandle) {
            $crate::ffi_guard::guard_unit("destroy_instance", || {
                if !handle.is_null() {
                    let mut instance = unsafe { Box::from_raw(handle as *mut $plugin_type) };
                    // Run cleanup() in a nested catch_unwind so that a
                    // panic here does not cause a double-panic abort if
                    // Drop also panics.
                    if let Err(payload) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            instance.cleanup()
                        }))
                    {
                        let msg = $crate::ffi_guard::panic_message(&*payload);
                        tracing::error!("plugin cleanup() panicked: {msg}");
                    }
                    // instance (Box) is dropped here — if Drop panics,
                    // the outer guard_unit catches it.
                }
            })
        }

        extern "C" fn __plugin_set_log_enabled_callback(
            handle: $crate::types::CPluginHandle,
            callback: $crate::types::CLogEnabledCallback,
            user_data: *mut std::os::raw::c_void,
        ) {
            $crate::ffi_guard::guard_unit("set_log_enabled_callback", || {
                if handle.is_null() {
                    return;
                }
                let instance = unsafe { &mut *(handle as *mut $plugin_type) };
                if let Some(logger) = instance.logger_mut() {
                    logger.set_enabled_callback(callback, user_data);
                }
            })
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
        static METADATA: std::sync::OnceLock<$crate::metadata_storage::PluginMetadataStorage> =
            std::sync::OnceLock::new();

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

        #[no_mangle]
        pub extern "C" fn streamkit_native_plugin_set_log_enabled_callback(
            handle: $crate::types::CPluginHandle,
            callback: $crate::types::CLogEnabledCallback,
            user_data: *mut std::os::raw::c_void,
        ) {
            __plugin_set_log_enabled_callback(handle, callback, user_data);
        }

        extern "C" fn __plugin_get_metadata() -> *const $crate::types::CNodeMetadata {
            $crate::ffi_guard::guard_ptr("get_metadata", || {
                let storage = METADATA.get_or_init(|| {
                    let meta = <$plugin_type as $crate::NativeProcessorNode>::metadata();
                    $crate::metadata_storage::PluginMetadataStorage::from_node_metadata(&meta)
                });
                &storage.c_metadata
            })
        }

        extern "C" fn __plugin_create_instance(
            params: *const std::os::raw::c_char,
            log_callback: $crate::types::CLogCallback,
            log_user_data: *mut std::os::raw::c_void,
        ) -> $crate::types::CPluginHandle {
            $crate::ffi_guard::guard_handle(|| {
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

                // Create logger using the plugin's kind as target (e.g. "whisper")
                // instead of module_path! which is opaque to the user.
                let kind = <$plugin_type as $crate::NativeProcessorNode>::metadata().kind;
                let logger = $crate::logger::Logger::new(log_callback, log_user_data, &kind);

                // Clone the logger so we can still report on `Err` after
                // ownership has been moved into `new()`.  Without this,
                // plugin-side validation errors (e.g. "url must not be
                // empty") were silently swallowed and the host only saw
                // a generic "Plugin failed to create instance" message.
                let err_logger = logger.clone();
                match <$plugin_type as $crate::NativeProcessorNode>::new(params_json, logger) {
                    Ok(instance) => Box::into_raw(Box::new(instance)) as $crate::types::CPluginHandle,
                    Err(e) => {
                        err_logger.error(&format!("Plugin instance creation failed: {e}"));
                        std::ptr::null_mut()
                    }
                }
            })
        }

        extern "C" fn __plugin_process_packet(
            handle: $crate::types::CPluginHandle,
            input_pin: *const std::os::raw::c_char,
            packet: *const $crate::types::CPacket,
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
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
            })
        }

        extern "C" fn __plugin_update_params(
            handle: $crate::types::CPluginHandle,
            params: *const std::os::raw::c_char,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
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
            })
        }

        extern "C" fn __plugin_flush(
            handle: $crate::types::CPluginHandle,
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
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
            })
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
        static METADATA: std::sync::OnceLock<$crate::metadata_storage::PluginMetadataStorage> =
            std::sync::OnceLock::new();

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

        #[no_mangle]
        pub extern "C" fn streamkit_native_plugin_set_log_enabled_callback(
            handle: $crate::types::CPluginHandle,
            callback: $crate::types::CLogEnabledCallback,
            user_data: *mut std::os::raw::c_void,
        ) {
            __plugin_set_log_enabled_callback(handle, callback, user_data);
        }

        extern "C" fn __plugin_get_metadata() -> *const $crate::types::CNodeMetadata {
            $crate::ffi_guard::guard_ptr("get_metadata", || {
                let storage = METADATA.get_or_init(|| {
                    let meta = <$plugin_type as $crate::NativeSourceNode>::metadata();
                    $crate::metadata_storage::PluginMetadataStorage::from_node_metadata(&meta)
                });
                &storage.c_metadata
            })
        }

        // ── Instance lifecycle ──────────────────────────────────────────

        extern "C" fn __plugin_create_instance(
            params: *const std::os::raw::c_char,
            log_callback: $crate::types::CLogCallback,
            log_user_data: *mut std::os::raw::c_void,
        ) -> $crate::types::CPluginHandle {
            $crate::ffi_guard::guard_handle(|| {
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

                // Create logger using the plugin's kind as target (e.g. "my_source")
                // instead of module_path! which is opaque to the user.
                let kind = <$plugin_type as $crate::NativeSourceNode>::metadata().kind;
                let logger = $crate::logger::Logger::new(log_callback, log_user_data, &kind);

                // Clone the logger so we can still report on `Err` after
                // ownership has been moved into `new()`.  See the
                // processor variant above for context.
                let err_logger = logger.clone();
                match <$plugin_type as $crate::NativeSourceNode>::new(params_json, logger) {
                    Ok(instance) => {
                        Box::into_raw(Box::new(instance)) as $crate::types::CPluginHandle
                    },
                    Err(e) => {
                        err_logger.error(&format!("Plugin instance creation failed: {e}"));
                        std::ptr::null_mut()
                    }
                }
            })
        }

        // ── Source-specific entry points ─────────────────────────────────

        extern "C" fn __plugin_get_source_config(
            handle: $crate::types::CPluginHandle,
        ) -> $crate::types::CSourceConfig {
            $crate::ffi_guard::guard_source_config(|| {
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
            })
        }

        extern "C" fn __plugin_tick(
            handle: $crate::types::CPluginHandle,
            callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CTickResult {
            $crate::ffi_guard::guard_tick(|| {
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
            })
        }

        // ── No-op processor stubs (required by CNativePluginAPI) ────────

        extern "C" fn __plugin_process_packet_noop(
            _handle: $crate::types::CPluginHandle,
            _input_pin: *const std::os::raw::c_char,
            _packet: *const $crate::types::CPacket,
            _callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
                let err = $crate::conversions::error_to_c(
                    "process_packet called on source plugin (not supported)",
                );
                $crate::types::CResult::error(err)
            })
        }

        extern "C" fn __plugin_flush_noop(
            _handle: $crate::types::CPluginHandle,
            _callbacks: *const $crate::types::CNodeCallbacks,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| $crate::types::CResult::success())
        }

        // ── Upstream hint delivery (v5) ─────────────────────────────────

        extern "C" fn __plugin_on_upstream_hint(
            handle: $crate::types::CPluginHandle,
            hint_json: *const std::os::raw::c_char,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
                if handle.is_null() {
                    let err = $crate::conversions::error_to_c("Invalid handle (null)");
                    return $crate::types::CResult::error(err);
                }
                let hint_str = match unsafe { $crate::conversions::c_str_to_string(hint_json) } {
                    Ok(s) => s,
                    Err(e) => {
                        let err =
                            $crate::conversions::error_to_c(format!("Invalid hint JSON: {e}"));
                        return $crate::types::CResult::error(err);
                    },
                };
                let hint: $crate::streamkit_core::UpstreamHint =
                    match serde_json::from_str(&hint_str) {
                        Ok(h) => h,
                        Err(e) => {
                            let err = $crate::conversions::error_to_c(format!(
                                "Failed to parse hint: {e}"
                            ));
                            return $crate::types::CResult::error(err);
                        },
                    };
                let instance = unsafe { &mut *(handle as *mut $plugin_type) };
                instance.on_upstream_hint(hint);
                $crate::types::CResult::success()
            })
        }

        // ── Shared ──────────────────────────────────────────────────────

        extern "C" fn __plugin_update_params(
            handle: $crate::types::CPluginHandle,
            params: *const std::os::raw::c_char,
        ) -> $crate::types::CResult {
            $crate::ffi_guard::guard_result(|| {
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
                            let err_msg = $crate::conversions::error_to_c(format!(
                                "Invalid params string: {e}"
                            ));
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
            })
        }

        $crate::__plugin_shared_ffi!($plugin_type);
    };
}
