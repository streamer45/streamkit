// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! NVIDIA NVENC/NVDEC HW-accelerated AV1 encoder and decoder nodes.
//!
//! Uses the [`shiguredo_nvcodec`](https://crates.io/crates/shiguredo_nvcodec)
//! crate which provides Rust bindings for the NVIDIA Video Codec SDK.  CUDA
//! driver API is loaded dynamically at runtime (`dlopen`) — no build-time
//! CUDA Toolkit dependency.
//!
//! This module provides:
//! - `NvAv1DecoderNode` — decodes AV1 packets to NV12 `VideoFrame`s via NVDEC
//! - `NvAv1EncoderNode` — encodes NV12 `VideoFrame`s to AV1 packets via NVENC
//!
//! Both nodes perform runtime capability detection: if no NVIDIA GPU with
//! AV1 support is found, node creation returns an error so the pipeline can
//! fall back to a CPU codec (rav1e/dav1d/SVT-AV1).
//!
//! # Feature gate
//!
//! Requires `nvcodec` feature.
//!
//! # GPU requirements
//!
//! - **AV1 decode**: NVIDIA RTX 30xx (Ampere) or newer.
//! - **AV1 encode**: NVIDIA RTX 40xx (Ada Lovelace) or newer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::{NodeRegistry, StreamKitError};

use super::HwAccelMode;
use super::AV1_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the NVIDIA AV1 decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NvAv1DecoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// CUDA device index (0-based). If `None`, use device 0.
    pub cuda_device: Option<u32>,
}

impl Default for NvAv1DecoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, cuda_device: None }
    }
}

/// NVIDIA NVDEC AV1 decoder node.
///
/// Accepts AV1 encoded `Binary` packets on its `"in"` pin and emits
/// decoded NV12 `VideoFrame`s on its `"out"` pin.
pub struct NvAv1DecoderNode {
    #[allow(dead_code)]
    config: NvAv1DecoderConfig,
}

impl NvAv1DecoderNode {
    /// Create a new decoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no NVIDIA GPU
    /// with AV1 decode capability is found.
    pub const fn new(config: NvAv1DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-c): Implement ProcessorNode for NvAv1DecoderNode
// - input_pins(): Binary AV1 input
// - output_pins(): RawVideo NV12 output
// - run(): init CUDA context, create shiguredo_nvcodec Decoder,
//   decode loop copying CUDA device memory to CPU NV12 via cuMemcpy2D

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Configuration for the NVIDIA AV1 encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NvAv1EncoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// CUDA device index (0-based). If `None`, use device 0.
    pub cuda_device: Option<u32>,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
}

impl Default for NvAv1EncoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, cuda_device: None, bitrate: 2_000_000 }
    }
}

/// NVIDIA NVENC AV1 encoder node.
///
/// Accepts NV12/I420 `VideoFrame`s on its `"in"` pin and emits AV1
/// encoded `Binary` packets on its `"out"` pin.
pub struct NvAv1EncoderNode {
    #[allow(dead_code)]
    config: NvAv1EncoderConfig,
}

impl NvAv1EncoderNode {
    /// Create a new encoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no NVIDIA GPU
    /// with AV1 encode capability is found.
    pub const fn new(config: NvAv1EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-c): Implement EncoderNodeRunner (or ProcessorNode directly)
// for NvAv1EncoderNode
// - input_pins(): RawVideo NV12/I420 input
// - output_pins(): EncodedVideo AV1 output
// - run(): init CUDA context, create shiguredo_nvcodec Encoder with
//   AV1 codec config, encode loop uploading CPU NV12 to CUDA input buffers

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_nv_av1_nodes(registry: &mut NodeRegistry) {
    let _ = registry;
    let _ = AV1_CONTENT_TYPE;

    // TODO(session-c): Implement ProcessorNode/EncoderNodeRunner for both
    // nodes, then uncomment the registration below.  See `vp9.rs` or `av1.rs`
    // for the registration pattern (register_static_with_description).
    //
    // Node IDs:
    //   "video::nv::av1_decoder"
    //   "video::nv::av1_encoder"
    //
    // Tags: ["video", "codecs", "av1", "hw", "nvidia"]
}
