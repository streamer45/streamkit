// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! C ABI types for native plugins
//!
//! These types define the stable binary interface between the host and native plugins.
//! The layout of these structs must remain stable across versions.

use std::os::raw::{c_char, c_void};

/// API version number. Plugins and host check compatibility via this field.
///
/// v1: Initial release — processor nodes (get_metadata, create_instance,
///     process_packet, update_params, flush, destroy_instance).
/// v2: Added telemetry callback parameters to process_packet and flush.
/// v3: Added video packet types (`RawVideo`, `EncodedVideo`), `CRawVideoFormat`,
///     `CPixelFormat`, and source node support (`get_source_config`, `tick`).
/// v4: Added `get_runtime_param_schema` (returns [`CSchemaResult`]) for
///     dynamic runtime parameter discovery.
/// v5: Added `on_upstream_hint` for receiving advisory hints from
///     downstream consumers (e.g. preferred output resolution).
/// v6: Added frame pool allocation (`CNodeCallbacks`, `CAllocVideoResult`,
///     `CAllocAudioResult`).  Consolidated per-call callback parameters
///     into a single `CNodeCallbacks` struct.  Extended `CVideoFrame` and
///     `CAudioFrame` with `buffer_handle` and `metadata` fields.
/// v7: Added `BinaryWithMeta` packet type (`CBinaryPacket`) that preserves
///     optional `content_type` and per-packet `metadata` across the plugin
///     ↔ host boundary.  Plain `Binary` remains for backward compatibility.
/// v8: Added `EncodedAudio` packet type discriminant, allowing plugins to
///     declare encoded audio output types (e.g. AAC) that are compatible
///     with MoQ transport nodes.  The codec name is carried as a
///     null-terminated string via the `custom_type_id` pointer in
///     [`CPacketTypeInfo`].
/// v9: Zero-copy binary packets (`CBinaryPacket::buffer_handle` + `free_fn`)
///     and logger overhaul (`CLogEnabledCallback`, `set_log_enabled_callback`,
///     logger target set to plugin kind instead of `module_path!()`).
///     **Wire change:** For v9 hosts, plain `Binary` packets are upgraded to
///     `BinaryWithMeta` (with null `content_type` / `metadata`) on the wire
///     to attach the zero-copy buffer handle.  C plugins that `switch` on
///     `packet_type` must handle `BinaryWithMeta` even when those fields
///     are null.
///     **ABI note:** `CBinaryPacket` grew by 16 bytes (buffer_handle +
///     free_fn).  v8 plugins compiled against the old 40-byte layout that
///     validate `len == sizeof(CBinaryPacket)` will reject BinaryWithMeta
///     packets from a v9 host.  Plugins using `len >= sizeof(…)` or bare
///     pointer casts are unaffected.
///     **Logger target change:** The logger target moved from
///     `module_path!()` (e.g. `whisper_plugin::inner`) to
///     `metadata().kind` (e.g. `whisper`).  Existing `RUST_LOG` directives
///     that filter on the old module path will need updating.
pub const NATIVE_PLUGIN_API_VERSION: u32 = 9;

/// Opaque handle to a plugin instance
pub type CPluginHandle = *mut c_void;

/// Log level for plugin logging
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CLogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

/// Callback function type for plugin logging
/// Parameters: (level, target, message, user_data)
/// - level: The log level
/// - target: Module path (e.g., "kokoro_plugin_native::kokoro_node")
/// - message: The log message
/// - user_data: Opaque pointer passed by host
pub type CLogCallback = extern "C" fn(CLogLevel, *const c_char, *const c_char, *mut c_void);

/// Callback to check whether a given log level is enabled for a target.
///
/// Parameters: (level, target, user_data) -> bool
///
/// The host implements this by consulting the tracing subscriber.  If
/// this returns `false`, the plugin can skip formatting the log message
/// entirely.
pub type CLogEnabledCallback = extern "C" fn(CLogLevel, *const c_char, *mut c_void) -> bool;

/// Function exported by v9 plugins to install the host's log-enabled callback.
pub type CSetLogEnabledCallback = extern "C" fn(CPluginHandle, CLogEnabledCallback, *mut c_void);

/// Result type for C ABI functions
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CResult {
    pub success: bool,
    /// Optional null-terminated error message.
    ///
    /// # Ownership
    ///
    /// This pointer is **borrowed** and must not be freed by the caller.
    /// Callers should copy it immediately if they need to keep it.
    pub error_message: *const c_char,
}

impl CResult {
    pub const fn success() -> Self {
        Self { success: true, error_message: std::ptr::null() }
    }

    pub const fn error(msg: *const c_char) -> Self {
        Self { success: false, error_message: msg }
    }
}

/// Result type for `get_runtime_param_schema`.
///
/// Unlike [`CResult`], this type has a dedicated `json_schema` field for the
/// success payload so that plugin authors don't have to read a JSON string
/// out of `error_message`.
///
/// - `success=true`, `json_schema=NULL` → plugin has no runtime schema.
/// - `success=true`, `json_schema=<ptr>` → JSON Schema string.
/// - `success=false`, `error_message=<ptr>` → error description.
///
/// # Ownership
///
/// Both pointers are **borrowed** and must not be freed by the caller.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CSchemaResult {
    pub success: bool,
    /// Null-terminated error message on failure, NULL on success.
    pub error_message: *const c_char,
    /// Null-terminated JSON Schema string on success, NULL otherwise.
    pub json_schema: *const c_char,
}

impl CSchemaResult {
    /// No runtime schema (success, both pointers NULL).
    pub const fn none() -> Self {
        Self { success: true, error_message: std::ptr::null(), json_schema: std::ptr::null() }
    }

    /// Runtime schema available (success, json_schema carries the payload).
    pub const fn schema(json: *const c_char) -> Self {
        Self { success: true, error_message: std::ptr::null(), json_schema: json }
    }

    /// Error during schema retrieval.
    pub const fn error(msg: *const c_char) -> Self {
        Self { success: false, error_message: msg, json_schema: std::ptr::null() }
    }
}

/// Audio sample format
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CSampleFormat {
    F32 = 0,
    S16Le = 1,
}

/// Audio format specification
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CAudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: CSampleFormat,
}

/// Packet type discriminant
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CPacketType {
    RawAudio = 0,
    OpusAudio = 1,
    Text = 2,
    Transcription = 3,
    Custom = 4,
    Binary = 5,
    Any = 6,
    Passthrough = 7,
    RawVideo = 8,
    /// Encoded video with codec metadata.  Uses `custom_type_id` in
    /// [`CPacketTypeInfo`] to carry the codec name (e.g. `"vp9"`, `"h264"`).
    /// Null `custom_type_id` falls back to `Binary` for backward compat.
    EncodedVideo = 9,
    /// Binary packet that preserves optional `content_type` and `metadata`
    /// across the plugin ↔ host boundary.  Points to a [`CBinaryPacket`].
    BinaryWithMeta = 10,
    /// Encoded audio with codec metadata.  Uses `custom_type_id` in
    /// [`CPacketTypeInfo`] to carry the codec name (e.g. `"opus"`, `"aac"`).
    /// Null `custom_type_id` defaults to Opus for backward compat.
    EncodedAudio = 11,
}

/// Pixel format discriminant for raw video frames.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CPixelFormat {
    Rgba8 = 0,
    I420 = 1,
    Nv12 = 2,
}

/// Raw video format metadata for the C ABI.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CRawVideoFormat {
    /// Frame width in pixels (0 = unspecified).
    pub width: u32,
    /// Frame height in pixels (0 = unspecified).
    pub height: u32,
    /// Pixel format.
    pub pixel_format: CPixelFormat,
}

/// Video frame data passed across the C ABI boundary.
///
/// `data` points to raw pixel bytes; layout depends on `pixel_format`.
///
/// When `buffer_handle` is non-null, the buffer was allocated from the
/// host's frame pool via [`CAllocVideoFn`].  The host reclaims the
/// underlying [`PooledVideoData`] directly — no copy is needed.
#[repr(C)]
pub struct CVideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: CPixelFormat,
    pub data: *const u8,
    pub data_len: usize,
    /// Opaque handle returned by [`CAllocVideoFn`].  NULL for legacy
    /// (non-pooled) frames.
    pub buffer_handle: *mut c_void,
    /// Optional metadata (may be null).
    pub metadata: *const CPacketMetadata,
}

/// Encoding for Custom packets.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CCustomEncoding {
    Json = 0,
}

/// Optional timing and sequencing metadata for packets.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct CPacketMetadata {
    pub timestamp_us: u64,
    pub has_timestamp_us: bool,
    pub duration_us: u64,
    pub has_duration_us: bool,
    pub sequence: u64,
    pub has_sequence: bool,
}

/// Custom packet payload passed across the C ABI boundary.
///
/// `data_json` points to UTF-8 encoded JSON (not null-terminated).
#[repr(C)]
pub struct CCustomPacket {
    pub type_id: *const c_char,
    pub encoding: CCustomEncoding,
    pub data_json: *const u8,
    pub data_len: usize,
    /// Optional metadata pointer (may be null).
    pub metadata: *const CPacketMetadata,
}

/// Full packet type with optional format information.
///
/// Exactly one of the optional pointers is non-null depending on
/// `type_discriminant`:
/// - `RawAudio`     → `audio_format`
/// - `Custom`       → `custom_type_id` (null-terminated type id string)
/// - `RawVideo`     → `raw_video_format`
/// - `EncodedAudio` → `custom_type_id` (null-terminated codec name, e.g. `"aac"`)
/// - `EncodedVideo` → `custom_type_id` (null-terminated codec name, e.g. `"h264"`)
///
/// ## Adding a new codec
///
/// Codec names are the canonical lowercase strings defined by
/// [`AudioCodec::as_c_name()`] and [`VideoCodec::as_c_name()`] in
/// `streamkit-core`.  To add a new codec:
///
/// 1. Add the variant to `AudioCodec` or `VideoCodec`.
/// 2. Add its name in `as_c_name()` and `from_c_name()` — these are the
///    **only** places codec-name strings need to live.
/// 3. The SDK macros and conversion functions use these methods, so no
///    further changes are needed in the plugin SDK.
///
/// # ABI stability
///
/// This struct is embedded (by value) in [`COutputPin`] and [`CInputPin`],
/// which live in arrays returned from `get_metadata`.  Adding fields would
/// change the struct size and break array indexing for older plugins.
/// New discriminants must reuse existing pointer fields.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CPacketTypeInfo {
    pub type_discriminant: CPacketType,
    /// For RawAudio: pointer to CAudioFormat, otherwise null.
    pub audio_format: *const CAudioFormat,
    /// For Custom: pointer to a null-terminated type id string.
    /// For EncodedAudio: pointer to a null-terminated codec name
    /// (e.g. `"opus"`, `"aac"`).
    /// For EncodedVideo: pointer to a null-terminated codec name
    /// (e.g. `"vp9"`, `"h264"`, `"av1"`).
    /// Otherwise null.
    pub custom_type_id: *const c_char,
    /// For RawVideo: pointer to CRawVideoFormat, otherwise null.
    pub raw_video_format: *const CRawVideoFormat,
}

/// Audio frame data (for RawAudio packets)
///
/// When `buffer_handle` is non-null, the buffer was allocated from the
/// host's audio frame pool via [`CAllocAudioFn`].  The host reclaims
/// the underlying [`PooledSamples`] directly — no copy is needed.
#[repr(C)]
pub struct CAudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: *const f32,
    pub sample_count: usize,
    /// Opaque handle returned by [`CAllocAudioFn`].  NULL for legacy
    /// (non-pooled) frames.
    pub buffer_handle: *mut c_void,
    /// Optional metadata (may be null).
    pub metadata: *const CPacketMetadata,
}

/// Binary packet with optional content-type and per-packet metadata.
///
/// Used as the `data` payload of a [`CPacket`] when
/// `packet_type == BinaryWithMeta`.  Unlike the plain `Binary` variant this
/// preserves MIME content-type (e.g. `"audio/aac"`) and timing metadata
/// across the plugin ↔ host boundary.
///
/// When `buffer_handle` is non-null (v9 host), the plugin can reclaim the
/// original `bytes::Bytes` via `Box::from_raw` for zero-copy transfer.
/// When null (v8 host or legacy), the plugin falls back to
/// `Bytes::copy_from_slice`.
#[repr(C)]
pub struct CBinaryPacket {
    pub data: *const u8,
    pub data_len: usize,
    /// Nullable.  Null-terminated MIME content-type string.
    pub content_type: *const c_char,
    /// Nullable.  Per-packet timing metadata.
    pub metadata: *const CPacketMetadata,
    /// Opaque handle to a `Box<bytes::Bytes>` for zero-copy transfer.
    /// NULL for v8 hosts or when the packet was not allocated from a
    /// `Bytes` buffer.  The plugin reclaims the `Bytes` via
    /// `Box::from_raw(handle.cast::<bytes::Bytes>())`.
    pub buffer_handle: *mut c_void,
    /// Releases the `buffer_handle` without using the buffer (e.g. on
    /// error paths where `packet_from_c` is never called).  NULL when
    /// `buffer_handle` is NULL.
    pub free_fn: Option<extern "C" fn(*mut c_void)>,
}

/// Generic packet container
/// The data field interpretation depends on packet_type
#[repr(C)]
pub struct CPacket {
    pub packet_type: CPacketType,
    pub data: *const c_void,
    pub len: usize,
}

/// Input pin definition
#[repr(C)]
pub struct CInputPin {
    pub name: *const c_char,
    /// Array of accepted packet types with format info
    pub accepts_types: *const CPacketTypeInfo,
    pub accepts_types_count: usize,
}

/// Output pin definition
#[repr(C)]
pub struct COutputPin {
    pub name: *const c_char,
    pub produces_type: CPacketTypeInfo,
}

/// Node metadata returned by plugin
#[repr(C)]
pub struct CNodeMetadata {
    pub kind: *const c_char,
    /// Optional description of the node (null-terminated string, can be null)
    pub description: *const c_char,
    pub inputs: *const CInputPin,
    pub inputs_count: usize,
    pub outputs: *const COutputPin,
    pub outputs_count: usize,
    /// JSON schema for parameters (null-terminated string)
    pub param_schema: *const c_char,
    /// Array of category strings
    pub categories: *const *const c_char,
    pub categories_count: usize,
}

/// Callback function type for sending output packets
/// Parameters: (pin_name, packet, user_data) -> CResult
pub type COutputCallback = extern "C" fn(*const c_char, *const CPacket, *mut c_void) -> CResult;

/// Callback function type for emitting telemetry events to the host.
///
/// Parameters:
/// - `event_type`: null-terminated UTF-8 string (e.g., "vad.speech_start")
/// - `data_json`: UTF-8 JSON bytes (not null-terminated)
/// - `data_len`: length of `data_json`
/// - `metadata`: optional packet-style metadata (may be null)
/// - `user_data`: opaque pointer provided by the host
pub type CTelemetryCallback = Option<
    extern "C" fn(*const c_char, *const u8, usize, *const CPacketMetadata, *mut c_void) -> CResult,
>;

/// Source node configuration returned by the plugin.
///
/// Tells the host how to drive the tick loop for source nodes (nodes with no
/// inputs that generate data on their own schedule).
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CSourceConfig {
    /// If true, this plugin is a source node (no inputs, host drives tick loop).
    pub is_source: bool,
    /// Microseconds between ticks (e.g. 33333 for 30 fps).
    pub tick_interval_us: u64,
    /// If > 0, host stops after this many ticks. 0 = infinite.
    pub max_ticks: u64,
}

/// Result returned by the source `tick` function.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CTickResult {
    /// Standard success/error result.
    pub result: CResult,
    /// If true, the source is done producing output (finite mode).
    pub done: bool,
}

impl CTickResult {
    /// Convenience: continue ticking.
    pub const fn ok() -> Self {
        Self { result: CResult::success(), done: false }
    }

    /// Convenience: last tick — stop after this one.
    pub const fn done() -> Self {
        Self { result: CResult::success(), done: true }
    }

    /// Convenience: tick failed.
    pub const fn error(msg: *const c_char) -> Self {
        Self { result: CResult::error(msg), done: false }
    }
}

/// The main plugin API structure.
///
/// Plugins export a function that returns a pointer to this struct.
/// Fields added after the required v6 layout are `Option` for backward
/// compatibility; processor plugins set source-only functions to `None`.
///
/// v9 extensions that would grow this struct live behind separate exported
/// symbols so a v9 host can safely load v6–v8 plugins compiled with the
/// smaller layout.
#[repr(C)]
pub struct CNativePluginAPI {
    /// API version for compatibility checking.
    pub version: u32,

    /// Get metadata about the node type.
    /// Returns: Pointer to CNodeMetadata (must remain valid for plugin lifetime).
    pub get_metadata: extern "C" fn() -> *const CNodeMetadata,

    /// Create a new plugin instance.
    /// params: JSON string with initialization parameters (nullable).
    /// log_callback: Callback for plugin to send log messages to host.
    /// log_user_data: Opaque pointer to pass to log callback.
    /// Returns: Opaque handle to the instance, or null on error.
    pub create_instance: extern "C" fn(*const c_char, CLogCallback, *mut c_void) -> CPluginHandle,

    /// Process an incoming packet (processor plugins).
    /// handle: Plugin instance handle.
    /// input_pin: Name of the input pin.
    /// packet: The packet to process.
    /// callbacks: Consolidated callback bundle (output + telemetry + alloc).
    pub process_packet: extern "C" fn(
        CPluginHandle,
        *const c_char,
        *const CPacket,
        *const CNodeCallbacks,
    ) -> CResult,

    /// Update runtime parameters.
    /// handle: Plugin instance handle.
    /// params: JSON string with new parameters (nullable).
    pub update_params: extern "C" fn(CPluginHandle, *const c_char) -> CResult,

    /// Flush any buffered data (called when input stream ends).
    /// handle: Plugin instance handle.
    /// callbacks: Consolidated callback bundle (output + telemetry + alloc).
    pub flush: extern "C" fn(CPluginHandle, *const CNodeCallbacks) -> CResult,

    /// Destroy a plugin instance.
    /// handle: Plugin instance handle.
    pub destroy_instance: extern "C" fn(CPluginHandle),

    // ── v3 additions ──────────────────────────────────────────────────────
    /// Query source configuration after instance creation.
    ///
    /// `None` for processor plugins. When `Some`, the returned
    /// `CSourceConfig.is_source` tells the host whether to use the tick
    /// loop instead of the input-driven processing loop.
    pub get_source_config: Option<extern "C" fn(CPluginHandle) -> CSourceConfig>,

    /// Produce one unit of output (source plugins).
    ///
    /// The host calls this at the interval specified by `get_source_config`.
    /// The plugin renders one frame/sample/etc. and sends it via
    /// `output_callback`.  Returns `CTickResult` to signal continuation or
    /// completion.
    ///
    /// `None` for processor plugins.
    pub tick: Option<extern "C" fn(CPluginHandle, *const CNodeCallbacks) -> CTickResult>,

    // ── v4 additions ──────────────────────────────────────────────────────
    /// Query runtime-discovered param schema after instance creation.
    ///
    /// Returns a [`CSchemaResult`] describing additional tunable parameters
    /// discovered at runtime (e.g. properties from a compiled `.slint`
    /// file).  The host deep-merges this with the static `param_schema`
    /// from metadata and delivers it to the UI.
    ///
    /// `None` when the plugin has no runtime-discovered parameters (the
    /// common case — most plugins declare everything statically).
    pub get_runtime_param_schema: Option<extern "C" fn(CPluginHandle) -> CSchemaResult>,

    // ── v5 additions ──────────────────────────────────────────────────────
    /// Deliver an upstream hint to a source plugin instance.
    ///
    /// `hint_json` is a null-terminated JSON string representing the
    /// serialized [`UpstreamHint`](streamkit_core::UpstreamHint).  The
    /// plugin deserializes it and adapts its output accordingly (e.g.
    /// resizing to a preferred resolution).
    ///
    /// `None` for processor plugins or source plugins that don't handle
    /// hints.
    pub on_upstream_hint: Option<extern "C" fn(CPluginHandle, *const c_char) -> CResult>,
}

// ── v6 additions: frame pool allocation ────────────────────────────────

/// Result of a video buffer allocation from the host's frame pool.
///
/// If `data` is non-null the allocation succeeded and the plugin owns the
/// buffer until it either passes it back via `CVideoFrame::buffer_handle`
/// or calls `free_fn(handle)` to release it without sending.
#[repr(C)]
pub struct CAllocVideoResult {
    /// Pointer to the writable buffer, or null on failure.
    pub data: *mut u8,
    /// Usable byte count (≥ requested `min_bytes`).
    pub len: usize,
    /// Opaque handle the plugin must store in `CVideoFrame::buffer_handle`
    /// (or pass to `free_fn` if the buffer is discarded).
    pub handle: *mut c_void,
    /// Releases the buffer without sending.  The plugin **must** call this
    /// if it decides not to send the frame (e.g. on error paths).
    pub free_fn: Option<extern "C" fn(*mut c_void)>,
}

impl CAllocVideoResult {
    /// Null / failed allocation sentinel.
    pub const fn null() -> Self {
        Self { data: std::ptr::null_mut(), len: 0, handle: std::ptr::null_mut(), free_fn: None }
    }
}

/// Result of an audio buffer allocation from the host's frame pool.
#[repr(C)]
pub struct CAllocAudioResult {
    /// Pointer to the writable sample buffer, or null on failure.
    pub data: *mut f32,
    /// Number of usable samples (≥ requested `min_samples`).
    pub sample_count: usize,
    /// Opaque handle the plugin must store in `CAudioFrame::buffer_handle`
    /// (or pass to `free_fn` if the buffer is discarded).
    pub handle: *mut c_void,
    /// Releases the buffer without sending.
    pub free_fn: Option<extern "C" fn(*mut c_void)>,
}

impl CAllocAudioResult {
    /// Null / failed allocation sentinel.
    pub const fn null() -> Self {
        Self {
            data: std::ptr::null_mut(),
            sample_count: 0,
            handle: std::ptr::null_mut(),
            free_fn: None,
        }
    }
}

/// Callback: allocate a video buffer from the host's frame pool.
///
/// `min_bytes` — minimum buffer size in bytes.
/// `user_data` — opaque pointer provided by the host.
pub type CAllocVideoFn = extern "C" fn(usize, *mut c_void) -> CAllocVideoResult;

/// Callback: allocate an audio buffer from the host's frame pool.
///
/// `min_samples` — minimum buffer size in samples.
/// `user_data` — opaque pointer provided by the host.
pub type CAllocAudioFn = extern "C" fn(usize, *mut c_void) -> CAllocAudioResult;

/// Consolidated callback bundle passed to `process_packet`, `flush`, and
/// `tick` starting in API v6.
///
/// Replaces the previous positional callback + user-data pairs with a
/// single struct pointer, making the ABI easier to extend in the future.
///
/// `struct_size` is set by the host so that a v7 plugin running on a v6
/// host can detect which fields are present.
#[repr(C)]
pub struct CNodeCallbacks {
    /// Size of this struct in bytes (set by the host).
    pub struct_size: usize,

    // ── output ──────────────────────────────────────────────────────────
    pub output_callback: COutputCallback,
    pub output_user_data: *mut c_void,

    // ── telemetry ───────────────────────────────────────────────────────
    pub telemetry_callback: CTelemetryCallback,
    pub telemetry_user_data: *mut c_void,

    // ── frame pool allocation (v6) ─────────────────────────────────────
    /// May be `None` if the host has no video pool for this pipeline.
    pub alloc_video: Option<CAllocVideoFn>,
    /// May be `None` if the host has no audio pool for this pipeline.
    pub alloc_audio: Option<CAllocAudioFn>,
    /// Opaque pointer passed as the last argument to `alloc_video` /
    /// `alloc_audio`.
    pub alloc_user_data: *mut c_void,
}

/// Symbol name that plugins must export
pub const PLUGIN_API_SYMBOL: &[u8] = b"streamkit_native_plugin_api\0";

/// Optional v9 symbol for installing the log-enabled callback.
pub const PLUGIN_SET_LOG_ENABLED_SYMBOL: &[u8] =
    b"streamkit_native_plugin_set_log_enabled_callback\0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_plugin_api_layout_stays_v8_sized_for_old_plugins() {
        let pointer_size = std::mem::size_of::<*const ()>();
        let version_with_padding = pointer_size;
        let function_fields = 10;
        assert_eq!(
            std::mem::size_of::<CNativePluginAPI>(),
            version_with_padding + function_fields * pointer_size
        );
    }
}
