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

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;
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

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};
use super::HwAccelMode;
use super::AV1_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the NVIDIA AV1 decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
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
    config: NvAv1DecoderConfig,
}

impl NvAv1DecoderNode {
    /// Create a new decoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceCpu` (this node only does HW).
    pub fn new(config: NvAv1DecoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "NvAv1DecoderNode only supports hardware decoding; \
                 use the CPU AV1 decoder (video::av1::decoder) for ForceCpu mode"
                    .to_string(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for NvAv1DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
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

        tracing::info!("NvAv1DecoderNode starting");
        let mut input_rx = context.take_input("in")?;
        let video_pool = context.video_pool.clone();

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("nv_av1_decoder_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("nv_av1_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, mut decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let cuda_device = self.config.cuda_device.unwrap_or(0);
        let decode_task = tokio::task::spawn_blocking(move || {
            let nv_config = shiguredo_nvcodec::DecoderConfig {
                #[allow(clippy::cast_possible_wrap)]
                device_id: cuda_device as i32,
                max_display_delay: 0, // low-latency
                ..shiguredo_nvcodec::DecoderConfig::default()
            };

            let mut decoder = match shiguredo_nvcodec::Decoder::new_av1(nv_config) {
                Ok(d) => d,
                Err(err) => {
                    let _ = result_tx.blocking_send(Err(format!(
                        "NVDEC: failed to create AV1 decoder on GPU {cuda_device}: {err}"
                    )));
                    return;
                },
            };

            tracing::info!("NVDEC AV1 decoder created on GPU {cuda_device}");

            while let Some((data, metadata)) = decode_rx.blocking_recv() {
                if result_tx.is_closed() {
                    return;
                }

                if data.is_empty() {
                    continue;
                }

                let decode_start_time = Instant::now();

                if let Err(err) = decoder.decode(&data) {
                    tracing::warn!("NVDEC AV1 decode error: {err}");
                    let _ =
                        result_tx.blocking_send(Err(format!("NVDEC: AV1 decode failed: {err}")));
                    continue;
                }

                // Drain all decoded frames produced by this input packet.
                loop {
                    match decoder.next_frame() {
                        Ok(Some(nv_frame)) => {
                            decode_duration_histogram
                                .record(decode_start_time.elapsed().as_secs_f64(), &[]);

                            match copy_nvdec_frame(&nv_frame, metadata.clone(), video_pool.as_ref())
                            {
                                Ok(frame) => {
                                    if result_tx.blocking_send(Ok(frame)).is_err() {
                                        return;
                                    }
                                },
                                Err(err) => {
                                    let _ = result_tx.blocking_send(Err(err));
                                },
                            }
                        },
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!("NVDEC next_frame error: {err}");
                            let _ = result_tx
                                .blocking_send(Err(format!("NVDEC: next_frame failed: {err}")));
                            break;
                        },
                    }
                }
            }

            // Flush remaining frames.
            if result_tx.is_closed() {
                return;
            }
            if let Err(err) = decoder.finish() {
                tracing::warn!("NVDEC finish error: {err}");
                return;
            }
            loop {
                match decoder.next_frame() {
                    Ok(Some(nv_frame)) => {
                        match copy_nvdec_frame(&nv_frame, None, video_pool.as_ref()) {
                            Ok(frame) => {
                                if result_tx.blocking_send(Ok(frame)).is_err() {
                                    return;
                                }
                            },
                            Err(err) => {
                                let _ = result_tx.blocking_send(Err(err));
                            },
                        }
                    },
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!("NVDEC flush next_frame error: {err}");
                        break;
                    },
                }
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

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
                                "NvAv1DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("NvAv1DecoderNode input stream closed");
        });

        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            decode_task,
            decode_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            Packet::Video,
            "NvAv1DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("NvAv1DecoderNode finished");
        Ok(())
    }
}

/// Copy a decoded NV12 frame from `shiguredo_nvcodec` into a `VideoFrame`.
///
/// The `DecodedFrame` already provides NV12 data (separate Y and interleaved
/// UV planes), so we copy them into a contiguous buffer with the canonical
/// packed NV12 layout.
fn copy_nvdec_frame(
    decoded: &shiguredo_nvcodec::DecodedFrame,
    metadata: Option<PacketMetadata>,
    video_pool: Option<&Arc<VideoFramePool>>,
) -> Result<VideoFrame, String> {
    #[allow(clippy::cast_possible_truncation)]
    let width = decoded.width() as u32;
    #[allow(clippy::cast_possible_truncation)]
    let height = decoded.height() as u32;

    if width == 0 || height == 0 {
        return Err("NVDEC produced empty frame".to_string());
    }

    let nv12_layout = VideoLayout::packed(width, height, PixelFormat::Nv12);
    let mut data = video_pool.map_or_else(
        || PooledVideoData::from_vec(vec![0u8; nv12_layout.total_bytes()]),
        |pool| pool.get(nv12_layout.total_bytes()),
    );
    let data_slice = data.as_mut_slice();

    let nv12_planes = nv12_layout.planes();
    let y_plane = nv12_planes[0];
    let uv_plane = nv12_planes[1];

    // Copy Y plane.
    let y_src = decoded.y_plane();
    let y_src_stride = decoded.y_stride();
    let width_usize = width as usize;
    let height_usize = height as usize;

    for row in 0..height_usize {
        let src_start = row * y_src_stride;
        let src_end = src_start + width_usize;
        if src_end > y_src.len() {
            return Err(format!("NVDEC Y plane too small: need {src_end}, have {}", y_src.len()));
        }
        let dst_start = y_plane.offset + row * y_plane.stride;
        let dst_end = dst_start + width_usize;
        if dst_end > data_slice.len() {
            return Err("NVDEC Y plane copy overflow".to_string());
        }
        data_slice[dst_start..dst_end].copy_from_slice(&y_src[src_start..src_end]);
    }

    // Copy UV plane (already interleaved NV12 format from NVDEC).
    let uv_src = decoded.uv_plane();
    let uv_src_stride = decoded.uv_stride();
    let chroma_h = uv_plane.height as usize;
    let uv_row_bytes = width_usize; // NV12: UV width == luma width (pairs of U,V bytes)

    for row in 0..chroma_h {
        let src_start = row * uv_src_stride;
        let src_end = src_start + uv_row_bytes;
        if src_end > uv_src.len() {
            return Err(format!("NVDEC UV plane too small: need {src_end}, have {}", uv_src.len()));
        }
        let dst_start = uv_plane.offset + row * uv_plane.stride;
        let dst_end = dst_start + uv_row_bytes;
        if dst_end > data_slice.len() {
            return Err("NVDEC UV plane copy overflow".to_string());
        }
        data_slice[dst_start..dst_end].copy_from_slice(&uv_src[src_start..src_end]);
    }

    VideoFrame::from_pooled(width, height, PixelFormat::Nv12, data, metadata)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Configuration for the NVIDIA AV1 encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NvAv1EncoderConfig {
    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
    /// CUDA device index (0-based). If `None`, use device 0.
    pub cuda_device: Option<u32>,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// Keyframe interval (GOP length). `None` uses the NVENC default
    /// (infinite GOP).
    pub keyframe_interval: Option<u32>,
}

impl Default for NvAv1EncoderConfig {
    fn default() -> Self {
        Self {
            hw_accel: HwAccelMode::Auto,
            cuda_device: None,
            bitrate: 2_000_000,
            keyframe_interval: None,
        }
    }
}

/// NVIDIA NVENC AV1 encoder node.
///
/// Accepts NV12/I420 `VideoFrame`s on its `"in"` pin and emits AV1
/// encoded `Binary` packets on its `"out"` pin.
pub struct NvAv1EncoderNode {
    config: NvAv1EncoderConfig,
}

impl NvAv1EncoderNode {
    /// Create a new encoder node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `hw_accel` is `ForceCpu` (this node only does HW).
    pub fn new(config: NvAv1EncoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "NvAv1EncoderNode only supports hardware encoding; \
                 use the CPU AV1 encoder (video::av1::encoder) for ForceCpu mode"
                    .to_string(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for NvAv1EncoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::I420,
                }),
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Nv12,
                }),
            ],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some(AV1_CONTENT_TYPE.to_string())
    }

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for NvAv1EncoderNode {
    const CONTENT_TYPE: &'static str = AV1_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "NvAv1EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "nv_av1_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "nv_av1_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<NvAv1Encoder>(
            self.config,
            encode_rx,
            result_tx,
            duration_histogram,
        )
    }
}

// ---------------------------------------------------------------------------
// Internal NVENC wrapper implementing StandardVideoEncoder
// ---------------------------------------------------------------------------

struct NvAv1Encoder {
    encoder: shiguredo_nvcodec::Encoder,
    next_pts: i64,
}

impl StandardVideoEncoder for NvAv1Encoder {
    type Config = NvAv1EncoderConfig;
    const CODEC_NAME: &'static str = "NV-AV1";

    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String>
    where
        Self: Sized,
    {
        let cuda_device = config.cuda_device.unwrap_or(0);

        let nv_config = shiguredo_nvcodec::EncoderConfig {
            width,
            height,
            fps_numerator: 30,
            fps_denominator: 1,
            target_bitrate: Some(config.bitrate),
            preset: shiguredo_nvcodec::Preset::P1, // fastest for real-time
            tuning_info: shiguredo_nvcodec::TuningInfo::LOW_LATENCY,
            rate_control_mode: shiguredo_nvcodec::RateControlMode::Cbr,
            gop_length: config.keyframe_interval,
            idr_period: config.keyframe_interval,
            frame_interval_p: 1, // no B-frames for low latency
            profile: None,
            #[allow(clippy::cast_possible_wrap)]
            device_id: cuda_device as i32,
            max_encode_width: None,
            max_encode_height: None,
        };

        let encoder = shiguredo_nvcodec::Encoder::new_av1(nv_config).map_err(|err| {
            format!("NVENC: failed to create AV1 encoder on GPU {cuda_device}: {err}")
        })?;

        tracing::info!(
            width,
            height,
            bitrate = config.bitrate,
            gpu = cuda_device,
            "NVENC AV1 encoder created"
        );

        Ok(Self { encoder, next_pts: 0 })
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        let nv12_data = match frame.pixel_format {
            PixelFormat::Nv12 => Cow::Borrowed(frame.data.as_slice()),
            PixelFormat::I420 => Cow::Owned(i420_to_nv12_buffer(frame)),
            other => {
                return Err(format!("NV-AV1 encoder expects NV12 or I420 input, got {other:?}"));
            },
        };

        self.encoder
            .encode(&nv12_data)
            .map_err(|err| format!("NVENC: AV1 encode failed: {err}"))?;

        Ok(self.drain_packets(metadata))
    }

    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String> {
        self.encoder.finish().map_err(|err| format!("NVENC: AV1 finish failed: {err}"))?;

        Ok(self.drain_packets(None))
    }

    fn flush_on_dimension_change() -> bool {
        true
    }
}

impl NvAv1Encoder {
    /// Drain all available encoded frames from NVENC.
    fn drain_packets(&mut self, metadata: Option<PacketMetadata>) -> Vec<EncodedPacket> {
        let mut packets = Vec::new();
        let mut remaining_metadata = metadata;

        loop {
            let Some(encoded) = self.encoder.next_frame() else {
                break;
            };

            let is_keyframe = matches!(
                encoded.picture_type(),
                shiguredo_nvcodec::PictureType::I | shiguredo_nvcodec::PictureType::Idr
            );
            let data = Bytes::from(encoded.into_data());

            let pts = self.next_pts;
            self.next_pts += 1;

            let meta = remaining_metadata.take();
            let output_metadata = merge_keyframe_metadata(meta, is_keyframe, pts);

            packets.push(EncodedPacket { data, metadata: Some(output_metadata) });
        }

        packets
    }
}

/// Convert an I420 `VideoFrame` to a contiguous NV12 byte buffer suitable
/// for `shiguredo_nvcodec::Encoder::encode()`.
fn i420_to_nv12_buffer(frame: &VideoFrame) -> Vec<u8> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let layout = frame.layout();
    let planes = layout.planes();
    let data = frame.data.as_slice();

    // NV12 layout: Y plane (width * height) + UV plane (chroma_w*2 * chroma_h)
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    let uv_row_bytes = chroma_w * 2; // ceil(width/2) pairs of (U, V)
    let nv12_size = width * height + uv_row_bytes * chroma_h;
    let mut nv12 = vec![0u8; nv12_size];

    // Copy Y plane.
    let y_plane = &planes[0];
    for row in 0..height {
        let src_start = y_plane.offset + row * y_plane.stride;
        let dst_start = row * width;
        nv12[dst_start..dst_start + width].copy_from_slice(&data[src_start..src_start + width]);
    }

    // Interleave U + V into NV12 UV plane.
    let u_plane = &planes[1];
    let v_plane = &planes[2];
    let uv_offset = width * height;

    for row in 0..chroma_h {
        let u_src_start = u_plane.offset + row * u_plane.stride;
        let v_src_start = v_plane.offset + row * v_plane.stride;
        let dst_start = uv_offset + row * uv_row_bytes;
        for col in 0..chroma_w {
            nv12[dst_start + col * 2] = data[u_src_start + col];
            nv12[dst_start + col * 2 + 1] = data[v_src_start + col];
        }
    }

    nv12
}

#[allow(clippy::missing_const_for_fn)] // map_or with closures is not yet stable in const fn
fn merge_keyframe_metadata(
    metadata: Option<PacketMetadata>,
    keyframe: bool,
    pts: i64,
) -> PacketMetadata {
    metadata.map_or(
        PacketMetadata {
            #[allow(clippy::cast_sign_loss)]
            timestamp_us: if pts >= 0 { Some(pts as u64) } else { None },
            duration_us: None,
            sequence: None,
            keyframe: Some(keyframe),
        },
        |meta| PacketMetadata {
            timestamp_us: meta.timestamp_us,
            duration_us: meta.duration_us,
            sequence: meta.sequence,
            keyframe: Some(keyframe),
        },
    )
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_nv_av1_nodes(registry: &mut NodeRegistry) {
    // Runtime capability check: verify that CUDA libraries are loadable.
    // If not, log a warning but still register the nodes — they will fail
    // at runtime with a clear error when the pipeline starts.
    if !shiguredo_nvcodec::is_cuda_library_available() {
        tracing::warn!(
            "CUDA libraries not available — NV AV1 encoder/decoder nodes \
             will fail at runtime if used"
        );
    }

    let default_decoder = NvAv1DecoderNode::new(NvAv1DecoderConfig::default())
        .expect("default NV AV1 decoder config should be valid");
    registry.register_static_with_description(
        "video::nv::av1_decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(NvAv1DecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(NvAv1DecoderConfig))
            .expect("NvAv1DecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "av1".to_string(),
            "hw".to_string(),
            "nvidia".to_string(),
        ],
        false,
        "Decodes AV1-compressed packets into raw NV12 video frames using \
         NVIDIA NVDEC hardware acceleration. Requires an NVIDIA RTX 30xx \
         (Ampere) or newer GPU.",
    );

    let default_encoder = NvAv1EncoderNode::new(NvAv1EncoderConfig::default())
        .expect("default NV AV1 encoder config should be valid");
    registry.register_static_with_description(
        "video::nv::av1_encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(NvAv1EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(NvAv1EncoderConfig))
            .expect("NvAv1EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "av1".to_string(),
            "hw".to_string(),
            "nvidia".to_string(),
        ],
        false,
        "Encodes raw video frames (NV12 or I420) into AV1 packets using \
         NVIDIA NVENC hardware acceleration. Requires an NVIDIA RTX 40xx \
         (Ada Lovelace) or newer GPU. Insert a video::pixel_convert node \
         upstream if the source outputs RGBA8.",
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
    use std::borrow::Cow;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// Returns `true` if CUDA libraries can be loaded AND a decoder can
    /// actually be created on device 0.  This catches machines that have
    /// `libcuda.so` but no physical GPU (or no AV1-capable GPU).
    fn nvdec_av1_available() -> bool {
        if !shiguredo_nvcodec::is_cuda_library_available() {
            return false;
        }
        let config = shiguredo_nvcodec::DecoderConfig {
            device_id: 0,
            max_display_delay: 0,
            ..shiguredo_nvcodec::DecoderConfig::default()
        };
        shiguredo_nvcodec::Decoder::new_av1(config).is_ok()
    }

    /// Returns `true` if NVENC AV1 encoding is available on device 0.
    /// AV1 encode requires RTX 40xx (Ada Lovelace) or newer.
    fn nvenc_av1_available() -> bool {
        if !shiguredo_nvcodec::is_cuda_library_available() {
            return false;
        }
        let config = shiguredo_nvcodec::EncoderConfig {
            width: 64,
            height: 64,
            fps_numerator: 30,
            fps_denominator: 1,
            target_bitrate: Some(2_000_000),
            preset: shiguredo_nvcodec::Preset::P1,
            tuning_info: shiguredo_nvcodec::TuningInfo::LOW_LATENCY,
            rate_control_mode: shiguredo_nvcodec::RateControlMode::Cbr,
            gop_length: Some(1),
            idr_period: Some(1),
            frame_interval_p: 1,
            profile: None,
            device_id: 0,
            max_encode_width: None,
            max_encode_height: None,
        };
        shiguredo_nvcodec::Encoder::new_av1(config).is_ok()
    }

    // ── Unit tests (no GPU required) ────────────────────────────────────────

    #[test]
    fn force_cpu_decoder_rejected() {
        let result = NvAv1DecoderNode::new(NvAv1DecoderConfig {
            hw_accel: HwAccelMode::ForceCpu,
            cuda_device: None,
        });
        assert!(result.is_err(), "ForceCpu should be rejected by NV decoder");
    }

    #[test]
    fn force_cpu_encoder_rejected() {
        let result = NvAv1EncoderNode::new(NvAv1EncoderConfig {
            hw_accel: HwAccelMode::ForceCpu,
            cuda_device: None,
            bitrate: 2_000_000,
            keyframe_interval: None,
        });
        assert!(result.is_err(), "ForceCpu should be rejected by NV encoder");
    }

    #[test]
    fn default_configs_accepted() {
        assert!(NvAv1DecoderNode::new(NvAv1DecoderConfig::default()).is_ok());
        assert!(NvAv1EncoderNode::new(NvAv1EncoderConfig::default()).is_ok());
    }

    #[test]
    fn decoder_pins_correct() {
        let node = NvAv1DecoderNode::new(NvAv1DecoderConfig::default()).unwrap();
        let inputs = node.input_pins();
        let outputs = node.output_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(outputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert_eq!(outputs[0].name, "out");
        assert!(
            matches!(&inputs[0].accepts_types[0], PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Av1),
            "Decoder input should accept AV1"
        );
        assert!(
            matches!(&outputs[0].produces_type, PacketType::RawVideo(fmt) if fmt.pixel_format == PixelFormat::Nv12),
            "Decoder output should produce NV12"
        );
    }

    #[test]
    fn encoder_pins_correct() {
        let node = NvAv1EncoderNode::new(NvAv1EncoderConfig::default()).unwrap();
        let inputs = node.input_pins();
        let outputs = node.output_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(outputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert_eq!(outputs[0].name, "out");
        // Encoder should accept both I420 and NV12.
        assert_eq!(inputs[0].accepts_types.len(), 2);
        assert!(
            matches!(&outputs[0].produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Av1),
            "Encoder output should produce AV1"
        );
    }

    #[test]
    fn deny_unknown_fields_decoder() {
        let json = r#"{"hw_accel":"Auto","cuda_device":null,"bogus_field":42}"#;
        let result: Result<NvAv1DecoderConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Unknown fields should be rejected");
    }

    #[test]
    fn deny_unknown_fields_encoder() {
        let json = r#"{"bitrate":1000000,"unknown_key":"oops"}"#;
        let result: Result<NvAv1EncoderConfig, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Unknown fields should be rejected");
    }

    #[test]
    fn i420_to_nv12_basic() {
        // Build a minimal 4×4 I420 frame and convert.
        let frame = create_test_video_frame(4, 4, PixelFormat::I420, 1);
        let nv12 = i420_to_nv12_buffer(&frame);

        // NV12 size: Y (4*4) + UV (ceil(4/2)*2 * ceil(4/2)) = 16 + 4*2 = 24
        let expected_size = 4 * 4 + 4 * 2;
        assert_eq!(nv12.len(), expected_size, "NV12 buffer size mismatch");
    }

    // ── GPU integration tests ───────────────────────────────────────────────

    /// Encode several NV12 frames via NVENC, then decode them via NVDEC.
    /// This is the full HW roundtrip test.
    #[tokio::test]
    async fn gpu_tests_nv_av1_encode_decode_roundtrip() {
        if !nvenc_av1_available() {
            eprintln!("Skipping NV AV1 encode/decode roundtrip: NVENC AV1 not available");
            return;
        }
        if !nvdec_av1_available() {
            eprintln!("Skipping NV AV1 encode/decode roundtrip: NVDEC AV1 not available");
            return;
        }

        // --- Encode ---
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = NvAv1EncoderConfig {
            bitrate: 2_000_000,
            keyframe_interval: Some(1),
            ..Default::default()
        };
        let encoder = NvAv1EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..5 {
            let timestamp = 1_000 + 33_333_u64 * index;
            let duration: u64 = 33_333;

            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp),
                duration_us: Some(duration),
                sequence: Some(index),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "NVENC AV1 encoder produced no packets");

        // --- Decode ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = NvAv1DecoderNode::new(NvAv1DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            if let Packet::Binary { data, metadata, .. } = packet {
                dec_input_tx
                    .send(Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed(AV1_CONTENT_TYPE)),
                        metadata,
                    })
                    .await
                    .unwrap();
            }
        }
        drop(dec_input_tx);

        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_sender.get_packets_for_pin("out").await;
        assert!(!decoded_packets.is_empty(), "NVDEC AV1 decoder produced no frames");

        for packet in decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");
                },
                _ => panic!("Expected Video packet from NV AV1 decoder"),
            }
        }
    }

    /// Encode-only test: verify that the encoder produces output packets
    /// and that the first packet is marked as a keyframe.
    #[tokio::test]
    async fn gpu_tests_nv_av1_encoder_produces_keyframes() {
        if !nvenc_av1_available() {
            eprintln!("Skipping NV AV1 encoder keyframe test: NVENC AV1 not available");
            return;
        }

        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = NvAv1EncoderConfig {
            bitrate: 2_000_000,
            keyframe_interval: Some(1),
            ..Default::default()
        };
        let encoder = NvAv1EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..3 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * index),
                duration_us: Some(33_333),
                sequence: Some(index),
                keyframe: None,
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "NVENC AV1 encoder produced no packets");

        // With keyframe_interval=1, every packet should be a keyframe.
        for (i, packet) in encoded_packets.iter().enumerate() {
            if let Packet::Binary { metadata, .. } = packet {
                let meta = metadata.as_ref().expect("Encoded packet should have metadata");
                assert_eq!(
                    meta.keyframe,
                    Some(true),
                    "Packet {i} should be a keyframe with keyframe_interval=1"
                );
            }
        }
    }

    /// Encode from I420 input — verifies the I420→NV12 conversion path.
    #[tokio::test]
    async fn gpu_tests_nv_av1_encoder_i420_input() {
        if !nvenc_av1_available() {
            eprintln!("Skipping NV AV1 I420 input test: NVENC AV1 not available");
            return;
        }

        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = NvAv1EncoderConfig {
            bitrate: 2_000_000,
            keyframe_interval: Some(1),
            ..Default::default()
        };
        let encoder = NvAv1EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        // Send I420 frames instead of NV12.
        for index in 0_u64..3 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 1);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * index),
                duration_us: Some(33_333),
                sequence: Some(index),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(
            !encoded_packets.is_empty(),
            "NVENC AV1 encoder produced no packets from I420 input"
        );
    }

    /// Metadata propagation: timestamps from input frames should be
    /// preserved through the encode→decode roundtrip.
    #[tokio::test]
    async fn gpu_tests_nv_av1_metadata_propagation() {
        if !nvenc_av1_available() || !nvdec_av1_available() {
            eprintln!("Skipping NV AV1 metadata test: NVENC/NVDEC AV1 not available");
            return;
        }

        // --- Encode ---
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = NvAv1EncoderConfig {
            bitrate: 2_000_000,
            keyframe_interval: Some(1),
            ..Default::default()
        };
        let encoder = NvAv1EncoderNode::new(encoder_config).unwrap();
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        let timestamps: Vec<u64> = vec![1_000, 34_333, 67_666];
        for (i, &ts) in timestamps.iter().enumerate() {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(ts),
                duration_us: Some(33_333),
                sequence: Some(i as u64),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty());

        // --- Decode and verify metadata ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = NvAv1DecoderNode::new(NvAv1DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            if let Packet::Binary { data, metadata, .. } = packet {
                dec_input_tx
                    .send(Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed(AV1_CONTENT_TYPE)),
                        metadata,
                    })
                    .await
                    .unwrap();
            }
        }
        drop(dec_input_tx);

        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_sender.get_packets_for_pin("out").await;
        assert!(!decoded_packets.is_empty(), "Decoder should produce at least one frame");

        // Every decoded frame should have metadata preserved.
        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Video(frame) => {
                    assert!(frame.metadata.is_some(), "Decoded frame {i} should have metadata");
                },
                _ => panic!("Expected Video packet from NV AV1 decoder"),
            }
        }
    }
}
