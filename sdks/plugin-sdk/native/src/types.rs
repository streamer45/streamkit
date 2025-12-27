// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! C ABI types for native plugins
//!
//! These types define the stable binary interface between the host and native plugins.
//! The layout of these structs must remain stable across versions.

use std::os::raw::{c_char, c_void};

/// API version number. Plugins and host check compatibility via this field.
pub const NATIVE_PLUGIN_API_VERSION: u32 = 2;

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

/// Full packet type with optional format information
/// For RawAudio, includes the audio format details
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CPacketTypeInfo {
    pub type_discriminant: CPacketType,
    /// For RawAudio: pointer to CAudioFormat, otherwise null
    pub audio_format: *const CAudioFormat,
    /// For Custom: pointer to a null-terminated type id string, otherwise null
    pub custom_type_id: *const c_char,
}

/// Audio frame data (for RawAudio packets)
#[repr(C)]
pub struct CAudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: *const f32,
    pub sample_count: usize,
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

/// The main plugin API structure
/// Plugins export a function that returns a pointer to this struct
#[repr(C)]
pub struct CNativePluginAPI {
    /// API version for compatibility checking
    pub version: u32,

    /// Get metadata about the node type
    /// Returns: Pointer to CNodeMetadata (must remain valid for plugin lifetime)
    pub get_metadata: extern "C" fn() -> *const CNodeMetadata,

    /// Create a new plugin instance
    /// params: JSON string with initialization parameters (nullable)
    /// log_callback: Callback for plugin to send log messages to host
    /// log_user_data: Opaque pointer to pass to log callback
    /// Returns: Opaque handle to the instance, or null on error
    pub create_instance: extern "C" fn(*const c_char, CLogCallback, *mut c_void) -> CPluginHandle,

    /// Process an incoming packet
    /// handle: Plugin instance handle
    /// input_pin: Name of the input pin
    /// packet: The packet to process
    /// output_callback: Callback to send output packets
    /// callback_data: User data to pass to output callback
    /// telemetry_callback: Callback to emit telemetry events
    /// telemetry_user_data: User data to pass to telemetry callback
    pub process_packet: extern "C" fn(
        CPluginHandle,
        *const c_char,
        *const CPacket,
        COutputCallback,
        *mut c_void,
        CTelemetryCallback,
        *mut c_void,
    ) -> CResult,

    /// Update runtime parameters
    /// handle: Plugin instance handle
    /// params: JSON string with new parameters (nullable)
    pub update_params: extern "C" fn(CPluginHandle, *const c_char) -> CResult,

    /// Flush any buffered data (called when input stream ends)
    /// handle: Plugin instance handle
    /// output_callback: Callback to send output packets
    /// callback_data: User data to pass to output callback
    /// telemetry_callback: Callback to emit telemetry events
    /// telemetry_user_data: User data to pass to telemetry callback
    pub flush: extern "C" fn(
        CPluginHandle,
        COutputCallback,
        *mut c_void,
        CTelemetryCallback,
        *mut c_void,
    ) -> CResult,

    /// Destroy a plugin instance
    /// handle: Plugin instance handle
    pub destroy_instance: extern "C" fn(CPluginHandle),
}

/// Symbol name that plugins must export
pub const PLUGIN_API_SYMBOL: &[u8] = b"streamkit_native_plugin_api\0";
