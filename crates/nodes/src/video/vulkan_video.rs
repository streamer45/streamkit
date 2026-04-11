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
#[serde(default, deny_unknown_fields)]
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
                let decode_result =
                    decoder.decode(vk_video::EncodedInputChunk { data: &data, pts });
                decode_duration_histogram.record(decode_start.elapsed().as_secs_f64(), &[]);

                match decode_result {
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
#[serde(default, deny_unknown_fields)]
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
    /// supports hardware encoding.  Also rejects zero bitrate or
    /// framerate to avoid confusing hardware-level errors later.
    pub fn new(config: VulkanVideoH264EncoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VulkanVideoH264EncoderNode only supports hardware encoding; \
                 use an OpenH264 encoder for CPU-only mode"
                    .to_string(),
            ));
        }
        if config.bitrate == 0 {
            return Err(StreamKitError::Configuration(
                "VulkanVideoH264EncoderNode: bitrate must be > 0".to_string(),
            ));
        }
        if config.framerate == 0 {
            return Err(StreamKitError::Configuration(
                "VulkanVideoH264EncoderNode: framerate must be > 0".to_string(),
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

        // ── Pre-initialise Vulkan device ─────────────────────────────────
        // Eagerly create the Vulkan device so the blocking encode task can
        // start processing frames immediately.  Without this, device
        // creation (~500 ms on some GPUs) blocks the encode loop and
        // causes short/fast pipelines (e.g. colorbars with 30 frames) to
        // produce zero output — the input stream closes before the encoder
        // is ready.
        let pre_init_device = tokio::task::spawn_blocking(|| init_vulkan_encode_device(None))
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Vulkan device init task panicked: {e}")))?
            .map_err(StreamKitError::Runtime)?;

        // ── Blocking encode task ─────────────────────────────────────────
        let config = self.config.clone();
        let encode_task = tokio::task::spawn_blocking(move || {
            // The BytesEncoder is lazily created on the first frame (so we
            // know the actual resolution), but the Vulkan device is already
            // initialised above to avoid blocking frame reception.
            let mut encoder: Option<vk_video::BytesEncoder> = None;
            let mut device: Option<Arc<vk_video::VulkanDevice>> = Some(pre_init_device);
            let mut current_dimensions: Option<(u32, u32)> = None;
            let mut frames_encoded: u64 = 0;

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

                    let dev = match init_vulkan_encode_device(device.as_ref()) {
                        Ok(d) => d,
                        Err(err) => {
                            let _ = result_tx.blocking_send(Err(err));
                            return;
                        },
                    };

                    let max_bitrate = u64::from(
                        config.max_bitrate.unwrap_or_else(|| config.bitrate.saturating_mul(4)),
                    );

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

                let Some(enc) = encoder.as_mut() else {
                    let _ = result_tx.blocking_send(Err("encoder not initialised".to_string()));
                    return;
                };

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
                        frames_encoded += 1;
                        // Always propagate the keyframe flag, even when
                        // the input had no metadata.  Without this,
                        // downstream RTMP/MoQ transport cannot detect
                        // keyframes for stream initialisation.
                        let out_meta = match metadata {
                            Some(mut m) => {
                                m.keyframe = Some(encoded_chunk.is_keyframe);
                                Some(m)
                            },
                            None => Some(PacketMetadata {
                                timestamp_us: None,
                                duration_us: None,
                                sequence: None,
                                keyframe: Some(encoded_chunk.is_keyframe),
                            }),
                        };

                        let output = EncoderOutput {
                            data: Bytes::from(encoded_chunk.data),
                            metadata: out_meta,
                        };
                        if result_tx.blocking_send(Ok(output)).is_err() {
                            tracing::debug!("VulkanVideoH264EncoderNode result channel closed after {frames_encoded} frame(s)");
                            return;
                        }
                    },
                    Err(err) => {
                        let _ = result_tx
                            .blocking_send(Err(format!("Vulkan Video H.264 encode error: {err}")));
                    },
                }
            }

            // Note: vk-video 0.3.0's BytesEncoder has no flush() method
            // (unlike BytesDecoder which does).  The encoder operates
            // frame-at-a-time without B-frame reordering, so no frames
            // should be buffered internally.  If a future vk-video version
            // adds flush(), it should be called here — matching the
            // decoder's flush at line ~245 and the pattern in
            // encoder_trait::spawn_standard_encode_task.
            tracing::info!("VulkanVideoH264EncoderNode encode task finished after {frames_encoded} frame(s)");
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
    existing: Option<&Arc<vk_video::VulkanDevice>>,
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

    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let uv_row_bytes = chroma_w * 2;
    let y_size = w * h;
    let mut nv12 = vec![0u8; y_size + uv_row_bytes * chroma_h];

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
    for row in 0..chroma_h {
        let u_src_start = u_plane.offset + row * u_plane.stride;
        let v_src_start = v_plane.offset + row * v_plane.stride;
        let dst_start = y_size + row * uv_row_bytes;
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
         (NVIDIA, AMD, or Intel with recent Mesa drivers). Use video::openh264::decoder \
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
        create_test_video_frame,
    };
    use std::collections::HashMap;
    use streamkit_core::types::Packet;
    use tokio::sync::mpsc;

    // ── Vulkan Video availability helper ────────────────────────────────
    //
    // Integration tests that require a Vulkan Video capable GPU use this
    // helper.  On machines without the right hardware/drivers the tests
    // print a message and pass (skip) instead of failing.

    /// Try to create a Vulkan Video device.  Returns `true` if both encode
    /// and decode are available.
    fn vulkan_video_available() -> bool {
        let Ok(instance) = vk_video::VulkanInstance::new() else {
            return false;
        };
        let Ok(adapter) =
            instance.create_adapter(&vk_video::parameters::VulkanAdapterDescriptor::default())
        else {
            return false;
        };
        let Ok(device) =
            adapter.create_device(&vk_video::parameters::VulkanDeviceDescriptor::default())
        else {
            return false;
        };
        device.supports_decoding() && device.supports_encoding()
    }

    /// Like [`vulkan_video_available`] but only checks for decode support.
    fn vulkan_decode_available() -> bool {
        let Ok(instance) = vk_video::VulkanInstance::new() else {
            return false;
        };
        let Ok(adapter) =
            instance.create_adapter(&vk_video::parameters::VulkanAdapterDescriptor::default())
        else {
            return false;
        };
        let Ok(device) =
            adapter.create_device(&vk_video::parameters::VulkanDeviceDescriptor::default())
        else {
            return false;
        };
        device.supports_decoding()
    }

    /// Like [`vulkan_video_available`] but only checks for encode support.
    fn vulkan_encode_available() -> bool {
        let Ok(instance) = vk_video::VulkanInstance::new() else {
            return false;
        };
        let Ok(adapter) =
            instance.create_adapter(&vk_video::parameters::VulkanAdapterDescriptor::default())
        else {
            return false;
        };
        let Ok(device) =
            adapter.create_device(&vk_video::parameters::VulkanDeviceDescriptor::default())
        else {
            return false;
        };
        device.supports_encoding()
    }

    macro_rules! skip_without_vulkan_encode {
        () => {
            if !vulkan_encode_available() {
                eprintln!("SKIPPED: no Vulkan Video encode support on this machine");
                return;
            }
        };
    }

    macro_rules! skip_without_vulkan_decode {
        () => {
            if !vulkan_decode_available() {
                eprintln!("SKIPPED: no Vulkan Video decode support on this machine");
                return;
            }
        };
    }

    macro_rules! skip_without_vulkan_video {
        () => {
            if !vulkan_video_available() {
                eprintln!("SKIPPED: no Vulkan Video encode+decode support on this machine");
                return;
            }
        };
    }

    // ── Config validation tests (no GPU needed) ─────────────────────────

    #[test]
    fn test_decoder_rejects_force_cpu() {
        let result = VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig {
            hw_accel: HwAccelMode::ForceCpu,
        });
        assert!(result.is_err(), "ForceCpu should be rejected for HW-only decoder");
    }

    #[test]
    fn test_decoder_accepts_auto() {
        let result = VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig {
            hw_accel: HwAccelMode::Auto,
        });
        assert!(result.is_ok(), "Auto should be accepted");
    }

    #[test]
    fn test_decoder_accepts_force_hw() {
        let result = VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig {
            hw_accel: HwAccelMode::ForceHw,
        });
        assert!(result.is_ok(), "ForceHw should be accepted");
    }

    #[test]
    fn test_encoder_rejects_force_cpu() {
        let result = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig {
            hw_accel: HwAccelMode::ForceCpu,
            ..Default::default()
        });
        assert!(result.is_err(), "ForceCpu should be rejected for HW-only encoder");
    }

    #[test]
    fn test_encoder_rejects_zero_bitrate() {
        let result = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig {
            bitrate: 0,
            ..Default::default()
        });
        assert!(result.is_err(), "bitrate=0 should be rejected");
    }

    #[test]
    fn test_encoder_rejects_zero_framerate() {
        let result = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig {
            framerate: 0,
            ..Default::default()
        });
        assert!(result.is_err(), "framerate=0 should be rejected");
    }

    #[test]
    fn test_encoder_accepts_valid_config() {
        let result = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig {
            hw_accel: HwAccelMode::Auto,
            bitrate: 2_000_000,
            max_bitrate: None,
            framerate: 30,
        });
        assert!(result.is_ok(), "valid config should be accepted");
    }

    #[test]
    fn test_encoder_accepts_custom_max_bitrate() {
        let result = VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig {
            hw_accel: HwAccelMode::Auto,
            bitrate: 2_000_000,
            max_bitrate: Some(8_000_000),
            framerate: 60,
        });
        assert!(result.is_ok(), "custom max_bitrate config should be accepted");
    }

    // ── deny_unknown_fields tests ─────────────────────────────────────

    #[test]
    fn test_deny_unknown_fields_decoder() {
        let json = r#"{"hw_accel":"auto","bogus_field":42}"#;
        let result: Result<VulkanVideoH264DecoderConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Unknown fields should be rejected");
    }

    #[test]
    fn test_deny_unknown_fields_encoder() {
        let json = r#"{"bitrate":1000000,"unknown_key":"oops"}"#;
        let result: Result<VulkanVideoH264EncoderConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Unknown fields should be rejected");
    }

    // ── Pin configuration tests ─────────────────────────────────────────

    #[test]
    fn test_decoder_pin_config() {
        let node =
            VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig::default()).unwrap();

        let inputs = node.input_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert!(matches!(inputs[0].cardinality, PinCardinality::One));
        assert!(matches!(
            &inputs[0].accepts_types[0],
            PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::H264
        ));

        let outputs = node.output_pins();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
        assert!(matches!(outputs[0].cardinality, PinCardinality::Broadcast));
        assert!(matches!(
            &outputs[0].produces_type,
            PacketType::RawVideo(fmt) if fmt.pixel_format == PixelFormat::Nv12
        ));
    }

    #[test]
    fn test_encoder_pin_config() {
        let node =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();

        let inputs = node.input_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert_eq!(inputs[0].accepts_types.len(), 2, "should accept NV12 and I420");

        let outputs = node.output_pins();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
        assert!(matches!(
            &outputs[0].produces_type,
            PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::H264
        ));
    }

    #[test]
    fn test_encoder_content_type() {
        let node =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();
        assert_eq!(
            node.content_type().as_deref(),
            Some(H264_CONTENT_TYPE),
            "Encoder should report video/h264 content type"
        );
    }

    // ── Integration tests (require Vulkan Video GPU) ────────────────────

    #[tokio::test]
    async fn test_vulkan_video_encode_nv12() {
        skip_without_vulkan_encode!();

        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, sender, mut state_rx) = create_test_context(inputs, 10);
        let encoder =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();

        let handle = tokio::spawn(async move { Box::new(encoder).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        for i in 0_u64..5 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * i),
                duration_us: Some(33_333),
                sequence: Some(i),
                keyframe: Some(i == 0),
            });
            input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = sender.get_packets_for_pin("out").await;
        assert!(!packets.is_empty(), "Vulkan Video encoder should produce packets");

        for (i, packet) in packets.iter().enumerate() {
            match packet {
                Packet::Binary { data, content_type, metadata, .. } => {
                    assert!(!data.is_empty(), "Encoded packet {i} should have data");
                    assert_eq!(
                        content_type.as_deref(),
                        Some(H264_CONTENT_TYPE),
                        "Content type should be video/h264"
                    );
                    assert!(metadata.is_some(), "Encoded packet {i} should have metadata");
                    let meta = metadata.as_ref().unwrap();
                    assert!(
                        meta.keyframe.is_some(),
                        "Encoded packet {i} should have keyframe flag"
                    );
                },
                _ => panic!("Expected Binary packet from Vulkan Video encoder, got {packet:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_vulkan_video_encode_i420() {
        skip_without_vulkan_encode!();

        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, sender, mut state_rx) = create_test_context(inputs, 10);
        let encoder =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();

        let handle = tokio::spawn(async move { Box::new(encoder).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        for i in 0_u64..3 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * i),
                duration_us: Some(33_333),
                sequence: Some(i),
                keyframe: Some(true),
            });
            input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = sender.get_packets_for_pin("out").await;
        assert!(!packets.is_empty(), "Vulkan Video encoder should produce packets from I420 input");
    }

    #[tokio::test]
    async fn test_vulkan_video_encode_metadata_without_input_metadata() {
        skip_without_vulkan_encode!();

        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, sender, mut state_rx) = create_test_context(inputs, 10);
        let encoder =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();

        let handle = tokio::spawn(async move { Box::new(encoder).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send frames with NO metadata to verify keyframe flag is still propagated.
        for _ in 0..3 {
            let frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            // frame.metadata is None by default from create_test_video_frame
            input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = sender.get_packets_for_pin("out").await;
        assert!(!packets.is_empty(), "Encoder should produce packets even without input metadata");

        for (i, packet) in packets.iter().enumerate() {
            match packet {
                Packet::Binary { metadata, .. } => {
                    assert!(
                        metadata.is_some(),
                        "Packet {i} should have metadata even when input had None"
                    );
                    let meta = metadata.as_ref().unwrap();
                    assert!(
                        meta.keyframe.is_some(),
                        "Packet {i} should always have keyframe flag set"
                    );
                },
                _ => panic!("Expected Binary packet"),
            }
        }
    }

    #[tokio::test]
    async fn test_vulkan_video_roundtrip_encode_decode() {
        skip_without_vulkan_video!();

        // ── Step 1: Encode NV12 frames to H.264 ─────────────────────────
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        let frame_count = 5_u64;
        let width = 64_u32;
        let height = 64_u32;

        for i in 0..frame_count {
            let mut frame = create_test_video_frame(width, height, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * i),
                duration_us: Some(33_333),
                sequence: Some(i),
                keyframe: Some(i == 0),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "Encoder should produce packets");

        // ── Step 2: Decode the H.264 packets back to NV12 ───────────────
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder =
            VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig::default()).unwrap();

        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        // Feed encoded packets to the decoder.
        for packet in encoded_packets {
            dec_input_tx.send(packet).await.unwrap();
        }
        drop(dec_input_tx);

        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_sender.get_packets_for_pin("out").await;
        assert!(!decoded_packets.is_empty(), "Decoder should produce frames from roundtrip data");

        // Verify decoded frames are NV12 with the right dimensions.
        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(
                        frame.pixel_format,
                        PixelFormat::Nv12,
                        "Decoded frame {i} should be NV12"
                    );
                    assert_eq!(frame.width, width, "Decoded frame {i} width mismatch");
                    assert_eq!(frame.height, height, "Decoded frame {i} height mismatch");
                    assert!(
                        !frame.data.as_slice().is_empty(),
                        "Decoded frame {i} should have data"
                    );
                },
                _ => panic!("Expected Video packet from decoder, got {packet:?}"),
            }
        }
    }

    // ── I420→NV12 conversion unit test ──────────────────────────────────

    #[test]
    fn test_i420_to_nv12_conversion() {
        let width = 4_u32;
        let height = 4_u32;
        let frame = create_test_video_frame(width, height, PixelFormat::I420, 0);

        // Manually fill planes with known values for verification.
        let layout = frame.layout();
        let planes = layout.planes();

        // Build a frame with identifiable plane content.
        let mut data = vec![0u8; layout.total_bytes()];
        // Y plane: fill with 100
        for row in 0..height as usize {
            for col in 0..width as usize {
                data[planes[0].offset + row * planes[0].stride + col] = 100;
            }
        }
        // U plane: fill with 50
        let chroma_w = width as usize / 2;
        let chroma_h = height as usize / 2;
        for row in 0..chroma_h {
            for col in 0..chroma_w {
                data[planes[1].offset + row * planes[1].stride + col] = 50;
            }
        }
        // V plane: fill with 200
        for row in 0..chroma_h {
            for col in 0..chroma_w {
                data[planes[2].offset + row * planes[2].stride + col] = 200;
            }
        }

        let test_frame = VideoFrame::new(width, height, PixelFormat::I420, data)
            .expect("test frame should be valid");

        let nv12 = i420_to_nv12(&test_frame);

        let y_size = (width * height) as usize;
        let uv_size = width as usize * (height as usize / 2);
        assert_eq!(nv12.len(), y_size + uv_size, "NV12 buffer size mismatch");

        // Verify Y plane was copied correctly.
        for (i, &byte) in nv12.iter().enumerate().take(y_size) {
            assert_eq!(byte, 100, "Y plane byte {i} mismatch");
        }

        // Verify UV plane has interleaved U and V values.
        for row in 0..chroma_h {
            for col in 0..chroma_w {
                let uv_offset = y_size + row * width as usize + col * 2;
                assert_eq!(nv12[uv_offset], 50, "U value at row={row} col={col} mismatch");
                assert_eq!(nv12[uv_offset + 1], 200, "V value at row={row} col={col} mismatch");
            }
        }
    }

    // ── Standalone decode test (requires encode+decode to produce input) ─

    #[tokio::test]
    async fn test_vulkan_video_decode_produces_frames() {
        // We need both encode (to generate H.264 data) and decode capabilities.
        // Use skip_without_vulkan_decode for the decode-specific skip message,
        // but we also need encode to produce test data.
        skip_without_vulkan_decode!();
        skip_without_vulkan_encode!();

        // First encode a few frames to get valid H.264 data.
        let (enc_tx, enc_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_rx);

        let (enc_ctx, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder =
            VulkanVideoH264EncoderNode::new(VulkanVideoH264EncoderConfig::default()).unwrap();
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_ctx).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for i in 0_u64..5 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * i),
                duration_us: Some(33_333),
                sequence: Some(i),
                keyframe: Some(i == 0),
            });
            enc_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "Need encoded data to test decoder");

        // Now decode.
        let (dec_tx, dec_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_rx);

        let (dec_ctx, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder =
            VulkanVideoH264DecoderNode::new(VulkanVideoH264DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_ctx).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            dec_tx.send(packet).await.unwrap();
        }
        drop(dec_tx);

        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_sender.get_packets_for_pin("out").await;
        assert!(!decoded_packets.is_empty(), "Decoder should produce NV12 frames");

        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(
                        frame.pixel_format,
                        PixelFormat::Nv12,
                        "Decoded frame {i} should be NV12"
                    );
                    assert_eq!(frame.width, 64, "Decoded frame {i} width mismatch");
                    assert_eq!(frame.height, 64, "Decoded frame {i} height mismatch");
                },
                _ => panic!("Expected Video packet from decoder"),
            }
        }
    }

    // ── Registration test ───────────────────────────────────────────────

    #[test]
    fn test_node_registration() {
        let mut registry = NodeRegistry::new();
        register_vulkan_video_nodes(&mut registry);

        // Verify both nodes are registered by trying to create them with
        // default config.
        assert!(
            registry.create_node("video::vulkan_video::h264_decoder", None).is_ok(),
            "decoder should be registered"
        );
        assert!(
            registry.create_node("video::vulkan_video::h264_encoder", None).is_ok(),
            "encoder should be registered"
        );
    }
}
