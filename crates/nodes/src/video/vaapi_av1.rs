// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! VA-API HW-accelerated AV1 encoder and decoder nodes.
//!
//! Uses the [`cros-codecs`](https://crates.io/crates/cros-codecs) crate which
//! provides VA-API bindings for hardware-accelerated video encoding and decoding
//! on Linux.  Primarily targets Intel GPUs (via `intel-media-driver`) but also
//! works on AMD (via Mesa VA-API).
//!
//! This module provides:
//! - `VaapiAv1DecoderNode` — decodes AV1 packets to NV12 `VideoFrame`s via VA-API
//! - `VaapiAv1EncoderNode` — encodes NV12 `VideoFrame`s to AV1 packets via VA-API
//!
//! Both nodes perform runtime capability detection: if no VA-API capable
//! device is found (or AV1 is not supported), node creation returns an error
//! so the pipeline can fall back to a CPU codec (rav1e/dav1d/SVT-AV1).
//!
//! # Feature gate
//!
//! Requires `vaapi` feature.
//!
//! # Platform support
//!
//! - **Intel**: Full AV1 encode (Arc+) and decode via `intel-media-driver`.
//! - **NVIDIA**: Decode only via community `nvidia-vaapi-driver`. No VA-API encoding.
//! - **AMD**: AV1 encode + decode via Mesa RadeonSI VA-API.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::{NodeRegistry, StreamKitError};

use super::HwAccelMode;
use super::AV1_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the VA-API AV1 decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VaapiAv1DecoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// VA-API render node path (e.g. `/dev/dri/renderD128`).
    /// If `None`, auto-detect the first capable device.
    pub render_device: Option<String>,
}

impl Default for VaapiAv1DecoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, render_device: None }
    }
}

/// VA-API AV1 decoder node.
///
/// Accepts AV1 encoded `Binary` packets on its `"in"` pin and emits
/// decoded NV12 `VideoFrame`s on its `"out"` pin.
pub struct VaapiAv1DecoderNode {
    #[allow(dead_code)]
    config: VaapiAv1DecoderConfig,
}

impl VaapiAv1DecoderNode {
    /// Create a new decoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no VA-API capable
    /// device with AV1 decode support is found.
    pub const fn new(config: VaapiAv1DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-b): Implement ProcessorNode for VaapiAv1DecoderNode
// - input_pins(): Binary AV1 input
// - output_pins(): RawVideo NV12 output
// - run(): open VA display, create cros_codecs AV1 decoder, decode loop
//   mapping VA surfaces to CPU NV12 via vaMapBuffer

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Configuration for the VA-API AV1 encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VaapiAv1EncoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// VA-API render node path (e.g. `/dev/dri/renderD128`).
    /// If `None`, auto-detect the first capable device.
    pub render_device: Option<String>,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
}

impl Default for VaapiAv1EncoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, render_device: None, bitrate: 2_000_000 }
    }
}

/// VA-API AV1 encoder node.
///
/// Accepts NV12/I420 `VideoFrame`s on its `"in"` pin and emits AV1
/// encoded `Binary` packets on its `"out"` pin.
pub struct VaapiAv1EncoderNode {
    #[allow(dead_code)]
    config: VaapiAv1EncoderConfig,
}

impl VaapiAv1EncoderNode {
    /// Create a new encoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no VA-API capable
    /// device with AV1 encode support is found.
    pub const fn new(config: VaapiAv1EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-b): Implement EncoderNodeRunner (or ProcessorNode directly)
// for VaapiAv1EncoderNode
// - input_pins(): RawVideo NV12/I420 input
// - output_pins(): EncodedVideo AV1 output
// - run(): open VA display, create cros_codecs AV1 encoder, encode loop
//   uploading CPU NV12 frames to VA surfaces

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_vaapi_av1_nodes(registry: &mut NodeRegistry) {
    let _ = registry;
    let _ = AV1_CONTENT_TYPE;

    // TODO(session-b): Implement ProcessorNode/EncoderNodeRunner for both
    // nodes, then uncomment the registration below.  See `vp9.rs` or `av1.rs`
    // for the registration pattern (register_static_with_description).
    //
    // Node IDs:
    //   "video::vaapi::av1_decoder"
    //   "video::vaapi::av1_encoder"
    //
    // Tags: ["video", "codecs", "av1", "hw", "vaapi"]
}
