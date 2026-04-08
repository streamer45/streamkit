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

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    EncodedVideoFormat, Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat,
    VideoCodec, VideoFrame, VideoLayout,
};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, PooledVideoData, ProcessorNode,
    StreamKitError, VideoFramePool,
};
use tokio::sync::mpsc;

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
/// Internally uses `vk-video::BytesDecoder` for GPU-accelerated decoding,
/// which returns raw NV12 pixel data directly — avoiding explicit GPU
/// texture readback while still leveraging the Vulkan Video decode engine.
pub struct VulkanVideoH264DecoderNode {
    config: VulkanVideoH264DecoderConfig,
}

impl VulkanVideoH264DecoderNode {
    /// Create a new decoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceCpu` — this node only
    /// supports hardware decoding.  Capability probing is deferred to
    /// `run()`.
    pub fn new(config: VulkanVideoH264DecoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VulkanVideoH264DecoderNode only supports hardware decoding; \
                 use an OpenH264 decoder for CPU-only mode"
                    .to_string(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VulkanVideoH264DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::H264,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            })],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Nv12,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("VulkanVideoH264DecoderNode starting (hw_accel={:?})", self.config.hw_accel);
        let mut input_rx = context.take_input("in")?;
        let video_pool = context.video_pool.clone();

        // ── Metrics ──────────────────────────────────────────────────────
        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("vulkan_video_h264_decoder_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("vulkan_video_h264_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        // ── Channels ─────────────────────────────────────────────────────
        let (decode_tx, mut decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        // ── Blocking decode task ─────────────────────────────────────────
        let decode_task = tokio::task::spawn_blocking(move || {
            let instance = match vk_video::VulkanInstance::new() {
                Ok(inst) => inst,
                Err(err) => {
                    let _ = result_tx
                        .blocking_send(Err(format!("failed to create VulkanInstance: {err}")));
                    return;
                },
            };

            let adapter = match instance
                .create_adapter(&vk_video::parameters::VulkanAdapterDescriptor::default())
            {
                Ok(a) => a,
                Err(err) => {
                    let _ = result_tx
                        .blocking_send(Err(format!("failed to create VulkanAdapter: {err}")));
                    return;
                },
            };

            let device = match adapter
                .create_device(&vk_video::parameters::VulkanDeviceDescriptor::default())
            {
                Ok(d) => d,
                Err(err) => {
                    let _ = result_tx
                        .blocking_send(Err(format!("failed to create VulkanDevice: {err}")));
                    return;
                },
            };

            if !device.supports_decoding() {
                let _ = result_tx.blocking_send(Err(
                    "Vulkan device does not support video decoding".to_string(),
                ));
                return;
            }

            let mut decoder = match device
                .create_bytes_decoder(vk_video::parameters::DecoderParameters::default())
            {
                Ok(dec) => dec,
                Err(err) => {
                    let _ = result_tx
                        .blocking_send(Err(format!("failed to create BytesDecoder: {err}")));
                    return;
                },
            };

            tracing::info!("Vulkan Video H.264 decoder initialised successfully");

            while let Some((data, metadata)) = decode_rx.blocking_recv() {
                if result_tx.is_closed() {
                    return;
                }

                let pts = metadata.as_ref().and_then(|m| m.timestamp_us);

                let decode_start = Instant::now();
                let decoded = decoder.decode(vk_video::EncodedInputChunk { data: &data, pts });
                decode_duration_histogram.record(decode_start.elapsed().as_secs_f64(), &[]);

                match decoded {
                    Ok(frames) => {
                        for output_frame in frames {
                            match raw_frame_to_video_frame(
                                &output_frame,
                                metadata.clone(),
                                video_pool.as_ref(),
                            ) {
                                Ok(vf) => {
                                    if result_tx.blocking_send(Ok(vf)).is_err() {
                                        return;
                                    }
                                },
                                Err(err) => {
                                    let _ = result_tx.blocking_send(Err(err));
                                },
                            }
                        }
                    },
                    Err(err) => {
                        let _ = result_tx
                            .blocking_send(Err(format!("Vulkan Video H.264 decode error: {err}")));
                    },
                }
            }

            // Flush remaining buffered frames.
            if result_tx.is_closed() {
                return;
            }
            match decoder.flush() {
                Ok(frames) => {
                    for output_frame in frames {
                        match raw_frame_to_video_frame(&output_frame, None, video_pool.as_ref()) {
                            Ok(vf) => {
                                if result_tx.blocking_send(Ok(vf)).is_err() {
                                    return;
                                }
                            },
                            Err(err) => {
                                let _ = result_tx.blocking_send(Err(err));
                            },
                        }
                    }
                },
                Err(err) => {
                    let _ = result_tx
                        .blocking_send(Err(format!("Vulkan Video H.264 flush error: {err}")));
                },
            }
        });

        // ── State transition ─────────────────────────────────────────────
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

        // ── Input task ───────────────────────────────────────────────────
        let decode_tx_clone = decode_tx.clone();
        let mut input_task = tokio::spawn(async move {
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Binary { data, metadata, .. } = packet {
                        if decode_tx_clone.send((data, metadata)).await.is_err() {
                            tracing::error!(
                                "VulkanVideoH264DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("VulkanVideoH264DecoderNode input stream closed");
        });

        // ── Forward loop ─────────────────────────────────────────────────
        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            decode_task,
            decode_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            Packet::Video,
            "VulkanVideoH264DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("VulkanVideoH264DecoderNode finished");
        Ok(())
    }
}

/// Convert a vk-video `OutputFrame<RawFrameData>` into a StreamKit `VideoFrame`.
fn raw_frame_to_video_frame(
    output_frame: &vk_video::OutputFrame<vk_video::RawFrameData>,
    metadata: Option<PacketMetadata>,
    video_pool: Option<&Arc<VideoFramePool>>,
) -> Result<VideoFrame, String> {
    let raw = &output_frame.data;
    let nv12_bytes = &raw.frame;
    let width = raw.width;
    let height = raw.height;

    let layout = VideoLayout::packed(width, height, PixelFormat::Nv12);
    let expected_bytes = layout.total_bytes();

    if nv12_bytes.len() < expected_bytes {
        return Err(format!(
            "Vulkan Video decoder returned {len} bytes but NV12 {width}×{height} needs {expected_bytes}",
            len = nv12_bytes.len(),
        ));
    }

    let mut data = video_pool.map_or_else(
        || PooledVideoData::from_vec(vec![0u8; expected_bytes]),
        |pool| pool.get(expected_bytes),
    );
    data.as_mut_slice()[..expected_bytes].copy_from_slice(&nv12_bytes[..expected_bytes]);

    let frame_metadata = metadata.map(|mut m| {
        // Propagate PTS from vk-video if the incoming metadata had none.
        if m.timestamp_us.is_none() {
            m.timestamp_us = output_frame.metadata.pts;
        }
        m
    });

    Ok(VideoFrame {
        data: Arc::new(data),
        pixel_format: PixelFormat::Nv12,
        width,
        height,
        layout,
        metadata: frame_metadata,
    })
}

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
    /// Maximum bitrate in bits per second (VBR mode).
    /// Defaults to 4× the target bitrate.
    pub max_bitrate: Option<u32>,
    /// Target framerate (frames per second).
    pub framerate: u32,
}

impl Default for VulkanVideoH264EncoderConfig {
    fn default() -> Self {
        Self { hw_accel: HwAccelMode::Auto, bitrate: 2_000_000, max_bitrate: None, framerate: 30 }
    }
}

/// Vulkan Video H.264 encoder node.
///
/// Accepts NV12/I420 `VideoFrame`s on its `"in"` pin and emits H.264
/// encoded `Binary` packets on its `"out"` pin.
///
/// Internally uses `vk-video::BytesEncoder` for GPU-accelerated encoding.
/// I420 input is converted to NV12 before encoding since Vulkan Video
/// operates on NV12.
pub struct VulkanVideoH264EncoderNode {
    config: VulkanVideoH264EncoderConfig,
}

impl VulkanVideoH264EncoderNode {
    /// Create a new encoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceCpu` — this node only
    /// supports hardware encoding.
    pub fn new(config: VulkanVideoH264EncoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VulkanVideoH264EncoderNode only supports hardware encoding; \
                 use an OpenH264 encoder for CPU-only mode"
                    .to_string(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VulkanVideoH264EncoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Nv12,
                }),
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::I420,
                }),
            ],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::H264,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some(H264_CONTENT_TYPE.to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "VulkanVideoH264EncoderNode starting (hw_accel={:?}, bitrate={})",
            self.config.hw_accel,
            self.config.bitrate,
        );
        let mut input_rx = context.take_input("in")?;

        // ── Metrics ──────────────────────────────────────────────────────
        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("vulkan_video_h264_encoder_packets_processed").build();
        let encode_duration_histogram = meter
            .f64_histogram("vulkan_video_h264_encode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        // ── Channels ─────────────────────────────────────────────────────
        let (encode_tx, mut encode_rx) =
            mpsc::channel::<(VideoFrame, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<EncoderOutput, String>>(get_codec_channel_capacity());

        // ── Blocking encode task ─────────────────────────────────────────
        let config = self.config.clone();
        let encode_task = tokio::task::spawn_blocking(move || {
            // Encoder and device are lazily initialised on the first frame
            // so we know the actual resolution.
            let mut encoder: Option<vk_video::BytesEncoder> = None;
            let mut device: Option<Arc<vk_video::VulkanDevice>> = None;
            let mut current_dimensions: Option<(u32, u32)> = None;

            while let Some((frame, metadata)) = encode_rx.blocking_recv() {
                if result_tx.is_closed() {
                    return;
                }

                let dims = (frame.width, frame.height);

                // (Re-)create encoder when dimensions change.
                if current_dimensions != Some(dims) {
                    tracing::info!(
                        "VulkanVideoH264EncoderNode: (re)creating encoder for {}×{}",
                        dims.0,
                        dims.1,
                    );

                    let dev = match init_vulkan_encode_device(&device) {
                        Ok(d) => d,
                        Err(err) => {
                            let _ = result_tx.blocking_send(Err(err));
                            return;
                        },
                    };

                    let max_bitrate =
                        u64::from(config.max_bitrate.unwrap_or(config.bitrate.saturating_mul(4)));

                    let output_params = match dev.encoder_output_parameters_high_quality(
                        vk_video::parameters::RateControl::VariableBitrate {
                            average_bitrate: u64::from(config.bitrate),
                            max_bitrate,
                            virtual_buffer_size: Duration::from_secs(2),
                        },
                    ) {
                        Ok(p) => p,
                        Err(err) => {
                            let _ = result_tx.blocking_send(Err(format!(
                                "failed to get encoder output parameters: {err}"
                            )));
                            return;
                        },
                    };

                    let width = NonZeroU32::new(dims.0).unwrap_or(NonZeroU32::MIN);
                    let height = NonZeroU32::new(dims.1).unwrap_or(NonZeroU32::MIN);

                    let enc =
                        match dev.create_bytes_encoder(vk_video::parameters::EncoderParameters {
                            input_parameters: vk_video::parameters::VideoParameters {
                                width,
                                height,
                                target_framerate: config.framerate.into(),
                            },
                            output_parameters: output_params,
                        }) {
                            Ok(e) => e,
                            Err(err) => {
                                let _ = result_tx.blocking_send(Err(format!(
                                    "failed to create BytesEncoder: {err}"
                                )));
                                return;
                            },
                        };

                    device = Some(dev);
                    encoder = Some(enc);
                    current_dimensions = Some(dims);
                }

                let enc = encoder.as_mut().expect("encoder should be initialised");

                // Convert I420 → NV12 if necessary.
                let nv12_data = match frame.pixel_format {
                    PixelFormat::Nv12 => frame.data.as_slice().to_vec(),
                    PixelFormat::I420 => i420_to_nv12(&frame),
                    other => {
                        let _ = result_tx.blocking_send(Err(format!(
                            "VulkanVideoH264EncoderNode: unsupported pixel format {other:?}, \
                             expected NV12 or I420"
                        )));
                        continue;
                    },
                };

                let force_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);

                let input_frame = vk_video::InputFrame {
                    data: vk_video::RawFrameData {
                        frame: nv12_data,
                        width: frame.width,
                        height: frame.height,
                    },
                    pts: metadata.as_ref().and_then(|m| m.timestamp_us),
                };

                let encode_start = Instant::now();
                let result = enc.encode(&input_frame, force_keyframe);
                encode_duration_histogram.record(encode_start.elapsed().as_secs_f64(), &[]);

                match result {
                    Ok(encoded_chunk) => {
                        let mut out_meta = metadata;
                        // Propagate keyframe flag from the encoder.
                        if let Some(ref mut m) = out_meta {
                            m.keyframe = Some(encoded_chunk.is_keyframe);
                        }

                        let output = EncoderOutput {
                            data: Bytes::from(encoded_chunk.data),
                            metadata: out_meta,
                        };
                        if result_tx.blocking_send(Ok(output)).is_err() {
                            return;
                        }
                    },
                    Err(err) => {
                        let _ = result_tx
                            .blocking_send(Err(format!("Vulkan Video H.264 encode error: {err}")));
                    },
                }
            }
        });

        // ── State transition ─────────────────────────────────────────────
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

        // ── Input task ───────────────────────────────────────────────────
        let encode_tx_clone = encode_tx.clone();
        let node_label = "VulkanVideoH264EncoderNode";
        let mut input_task = tokio::spawn(async move {
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Video(mut frame) = packet {
                        let metadata = frame.metadata.take();
                        if encode_tx_clone.send((frame, metadata)).await.is_err() {
                            tracing::error!("{node_label} encode task has shut down unexpectedly");
                            return;
                        }
                    }
                }
            }
            tracing::info!("{node_label} input stream closed");
        });

        // ── Forward loop ─────────────────────────────────────────────────
        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            encode_task,
            encode_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            |encoded: EncoderOutput| Packet::Binary {
                data: encoded.data,
                content_type: Some(Cow::Borrowed(H264_CONTENT_TYPE)),
                metadata: encoded.metadata,
            },
            node_label,
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("VulkanVideoH264EncoderNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoder helpers
// ---------------------------------------------------------------------------

/// Internal encoded output type for the encoder channel.
struct EncoderOutput {
    data: Bytes,
    metadata: Option<PacketMetadata>,
}

/// Initialise (or reuse) the Vulkan device for encoding.
fn init_vulkan_encode_device(
    existing: &Option<Arc<vk_video::VulkanDevice>>,
) -> Result<Arc<vk_video::VulkanDevice>, String> {
    if let Some(dev) = existing {
        return Ok(Arc::clone(dev));
    }

    let instance = vk_video::VulkanInstance::new()
        .map_err(|e| format!("failed to create VulkanInstance: {e}"))?;

    let adapter = instance
        .create_adapter(&vk_video::parameters::VulkanAdapterDescriptor::default())
        .map_err(|e| format!("failed to create VulkanAdapter: {e}"))?;

    let device = adapter
        .create_device(&vk_video::parameters::VulkanDeviceDescriptor::default())
        .map_err(|e| format!("failed to create VulkanDevice: {e}"))?;

    if !device.supports_encoding() {
        return Err("Vulkan device does not support video encoding".to_string());
    }

    tracing::info!("Vulkan Video encode device initialised successfully");
    Ok(device)
}

/// Convert an I420 `VideoFrame` to NV12 byte layout.
///
/// NV12 layout: Y plane (width × height) followed by interleaved UV plane
/// (width × height/2).
fn i420_to_nv12(frame: &VideoFrame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let layout = frame.layout();

    let y_size = w * h;
    let uv_size = w * (h / 2);
    let mut nv12 = vec![0u8; y_size + uv_size];

    let src = frame.data.as_slice();
    let planes = layout.planes();

    let y_plane = &planes[0];
    let u_plane = &planes[1];
    let v_plane = &planes[2];

    // Copy Y plane.
    for row in 0..h {
        let src_start = y_plane.offset + row * y_plane.stride;
        let dst_start = row * w;
        nv12[dst_start..dst_start + w].copy_from_slice(&src[src_start..src_start + w]);
    }

    // Interleave U and V into NV12 UV plane.
    let chroma_h = h / 2;
    let chroma_w = w / 2;
    for row in 0..chroma_h {
        let u_src_start = u_plane.offset + row * u_plane.stride;
        let v_src_start = v_plane.offset + row * v_plane.stride;
        let dst_start = y_size + row * w;
        for col in 0..chroma_w {
            nv12[dst_start + col * 2] = src[u_src_start + col];
            nv12[dst_start + col * 2 + 1] = src[v_src_start + col];
        }
    }

    nv12
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_vulkan_video_nodes(registry: &mut NodeRegistry) {
    let default_decoder = VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig::default())
        .expect("default VulkanVideoH264 decoder config should be valid");
    registry.register_static_with_description(
        "video::vulkan_video::h264_decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VulkanVideoH264DecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VulkanVideoH264DecoderConfig))
            .expect("VulkanVideoH264DecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "h264".to_string(), "hw".to_string()],
        false,
        "Decodes H.264 Annex B packets into raw NV12 video frames using Vulkan Video \
         hardware acceleration. Requires a GPU with Vulkan Video decode support \
         (NVIDIA, AMD, or Intel with recent Mesa drivers). Use video::openh264::encoder \
         for CPU-only fallback.",
    );

    let default_encoder = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default())
        .expect("default VulkanVideoH264 encoder config should be valid");
    registry.register_static_with_description(
        "video::vulkan_video::h264_encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VulkanVideoH264EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VulkanVideoH264EncoderConfig))
            .expect("VulkanVideoH264EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "h264".to_string(), "hw".to_string()],
        false,
        "Encodes raw video frames (NV12 or I420) into H.264 Annex B packets using \
         Vulkan Video hardware acceleration. Supports VBR rate control with configurable \
         bitrate. Requires a GPU with Vulkan Video encode support. Use \
         video::openh264::encoder for CPU-only fallback.",
    );
}
