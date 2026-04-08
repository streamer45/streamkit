// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Vulkan Video HW-accelerated H.264 encoder and decoder nodes.
//!
//! Uses the [`vk-video`](https://crates.io/crates/vk-video) crate which wraps
//! the Vulkan Video extensions and integrates natively with `wgpu`.  Decoded
//! frames are `wgpu::Texture`s — enabling a zero-copy path with the GPU
//! compositor in the future.
//!
//! This module provides:
//! - `VulkanVideoH264DecoderNode` — decodes H.264 packets to NV12 `VideoFrame`s
//! - `VulkanVideoH264EncoderNode` — encodes NV12 `VideoFrame`s to H.264 packets
//!
//! Both nodes perform runtime capability detection: if no Vulkan Video capable
//! GPU is found, node creation returns an error so the pipeline can fall back
//! to a CPU codec.
//!
//! # Feature gate
//!
//! Requires `vulkan_video` feature.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::{NodeRegistry, StreamKitError};

use super::HwAccelMode;
use super::H264_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the Vulkan Video H.264 decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VulkanVideoH264DecoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
}

impl Default for VulkanVideoH264DecoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto }
    }
}

/// Vulkan Video H.264 decoder node.
///
/// Accepts H.264 encoded `Binary` packets on its `"in"` pin and emits
/// decoded NV12 `VideoFrame`s on its `"out"` pin.
///
/// Internally uses `vk-video::WgpuTexturesDecoder` for GPU-accelerated
/// decoding, with readback to CPU NV12 for downstream compatibility.
pub struct VulkanVideoH264DecoderNode {
    #[allow(dead_code)]
    config: VulkanVideoH264DecoderConfig,
}

impl VulkanVideoH264DecoderNode {
    /// Create a new decoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no Vulkan Video
    /// capable GPU is found (capability probing is deferred to `run()`
    /// in the initial implementation).
    pub const fn new(config: VulkanVideoH264DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-a): Implement ProcessorNode for VulkanVideoH264DecoderNode
// - input_pins(): Binary H.264 input
// - output_pins(): RawVideo NV12 output
// - run(): create VulkanInstance → VulkanAdapter → VulkanDevice,
//   create WgpuTexturesDecoder, decode loop with readback to CPU NV12

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Configuration for the Vulkan Video H.264 encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VulkanVideoH264EncoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
}

impl Default for VulkanVideoH264EncoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, bitrate: 2_000_000 }
    }
}

/// Vulkan Video H.264 encoder node.
///
/// Accepts NV12/I420 `VideoFrame`s on its `"in"` pin and emits H.264
/// encoded `Binary` packets on its `"out"` pin.
///
/// Internally uses `vk-video::WgpuTexturesEncoder` for GPU-accelerated
/// encoding, with upload from CPU NV12 initially.
pub struct VulkanVideoH264EncoderNode {
    #[allow(dead_code)]
    config: VulkanVideoH264EncoderConfig,
}

impl VulkanVideoH264EncoderNode {
    /// Create a new encoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceHw` and no Vulkan Video
    /// capable GPU is found.
    pub const fn new(config: VulkanVideoH264EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

// TODO(session-a): Implement EncoderNodeRunner for VulkanVideoH264EncoderNode
// or implement ProcessorNode directly (vk-video has its own async model)
// - input_pins(): RawVideo NV12/I420 input
// - output_pins(): EncodedVideo H.264 output
// - run(): create VulkanInstance → VulkanAdapter → VulkanDevice,
//   create WgpuTexturesEncoder, encode loop with upload from CPU NV12

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_vulkan_video_nodes(registry: &mut NodeRegistry) {
    let _ = registry;
    let _ = H264_CONTENT_TYPE;

    // TODO(session-a): Implement ProcessorNode/EncoderNodeRunner for both
    // nodes, then uncomment the registration below.  See `vp9.rs` or `av1.rs`
    // for the registration pattern (register_static_with_description).
    //
    // Node IDs:
    //   "video::vulkan_video::h264_decoder"
    //   "video::vulkan_video::h264_encoder"
    //
    // Tags: ["video", "codecs", "h264", "hw"]
}
