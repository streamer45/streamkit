// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! H.264 video encoder node (CPU).
//!
//! Uses [openh264](https://crates.io/crates/openh264) (Cisco's OpenH264,
//! BSD-2-Clause) for encoding.  Only Constrained Baseline profile is
//! supported — no B-frames, no CABAC — which is ideal for real-time and
//! WebRTC use cases.

use async_trait::async_trait;
use bytes::Bytes;
use openh264::encoder::{BitRate, EncoderConfig, FrameRate, FrameType, RateControlMode};
use openh264::formats::YUVSlices;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::types::{
    EncodedVideoFormat, PacketMetadata, PacketType, PixelFormat, RawVideoFormat, VideoCodec,
    VideoFrame,
};
use streamkit_core::{
    config_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};
use tokio::sync::mpsc;

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};
use super::H264_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Default encoder parameters
// ---------------------------------------------------------------------------

const H264_DEFAULT_BITRATE_KBPS: u32 = 2000;
const H264_DEFAULT_MAX_FRAME_RATE: f32 = 30.0;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the OpenH264 encoder node.
///
/// OpenH264 only supports Constrained Baseline profile (no B-frames, no
/// CABAC).  This is well-suited for real-time / low-latency use cases.
#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct OpenH264EncoderConfig {
    /// Target bitrate in kilobits per second.  Must be greater than zero.
    pub bitrate_kbps: u32,
    /// Maximum frame rate in Hz.  Must be greater than zero.
    pub max_frame_rate: f32,
}

impl Default for OpenH264EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_kbps: H264_DEFAULT_BITRATE_KBPS,
            max_frame_rate: H264_DEFAULT_MAX_FRAME_RATE,
        }
    }
}

// ---------------------------------------------------------------------------
// Encoder node
// ---------------------------------------------------------------------------

pub struct OpenH264EncoderNode {
    config: OpenH264EncoderConfig,
}

impl OpenH264EncoderNode {
    /// Maximum sane bitrate (500 Mbps).  Values above this are almost
    /// certainly typos and would overflow when converted to bps.
    const MAX_BITRATE_KBPS: u32 = 500_000;

    #[allow(clippy::missing_errors_doc)]
    pub fn new(config: OpenH264EncoderConfig) -> Result<Self, StreamKitError> {
        if config.bitrate_kbps == 0 {
            return Err(StreamKitError::Configuration(
                "OpenH264 encoder: bitrate_kbps must be greater than zero".into(),
            ));
        }
        if config.bitrate_kbps > Self::MAX_BITRATE_KBPS {
            return Err(StreamKitError::Configuration(format!(
                "OpenH264 encoder: bitrate_kbps {} exceeds maximum of {} (500 Mbps)",
                config.bitrate_kbps,
                Self::MAX_BITRATE_KBPS,
            )));
        }
        if config.max_frame_rate <= 0.0 || !config.max_frame_rate.is_finite() {
            return Err(StreamKitError::Configuration(
                "OpenH264 encoder: max_frame_rate must be a positive finite value".into(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for OpenH264EncoderNode {
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

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for OpenH264EncoderNode {
    const CONTENT_TYPE: &'static str = H264_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "OpenH264EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "openh264_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "openh264_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<OpenH264Encoder>(
            self.config,
            encode_rx,
            result_tx,
            duration_histogram,
        )
    }
}

impl StandardVideoEncoder for OpenH264Encoder {
    type Config = OpenH264EncoderConfig;
    const CODEC_NAME: &'static str = "H264";

    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String> {
        Self::new(width, height, config)
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        self.encode_frame(frame, metadata)
    }

    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String> {
        // OpenH264 (Constrained Baseline) does not buffer or reorder frames,
        // so there is nothing to flush.
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Internal codec wrapper
// ---------------------------------------------------------------------------

struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    /// Reusable I420 buffer to avoid per-frame heap allocation.
    yuv_buf: Vec<u8>,
}

impl OpenH264Encoder {
    fn new(_width: u32, _height: u32, config: &OpenH264EncoderConfig) -> Result<Self, String> {
        // The openh264 crate auto-detects resolution from the YUVSource on
        // each encode() call and reinits internally when it changes.  The
        // StandardVideoEncoder infrastructure recreates the encoder on
        // resolution changes anyway, so we don't set dimensions here.
        let enc_config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(config.bitrate_kbps.saturating_mul(1000)))
            .max_frame_rate(FrameRate::from_hz(config.max_frame_rate))
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(false);

        let encoder = openh264::encoder::Encoder::with_api_config(
            openh264::OpenH264API::from_source(),
            enc_config,
        )
        .map_err(|e| format!("OpenH264: failed to create encoder: {e}"))?;

        Ok(Self { encoder, yuv_buf: Vec::new() })
    }

    fn encode_frame(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        if !matches!(frame.pixel_format, PixelFormat::I420 | PixelFormat::Nv12) {
            return Err(format!(
                "OpenH264 encoder expects I420 or NV12 input, got {:?}",
                frame.pixel_format
            ));
        }

        let width = frame.width as usize;
        let height = frame.height as usize;

        // H.264 requires even dimensions (4:2:0 chroma subsampling).
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(format!("OpenH264 encoder requires even dimensions, got {width}x{height}"));
        }

        let layout = frame.layout();

        if frame.data_len() < layout.total_bytes() {
            return Err(format!(
                "OpenH264 encoder expected {} bytes, got {}",
                layout.total_bytes(),
                frame.data_len()
            ));
        }

        // Force IDR if metadata requests a keyframe.
        let force_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);
        if force_keyframe {
            self.encoder.force_intra_frame();
        }

        // Build I420 YUV data for the openh264 crate.
        self.fill_yuv_buffer(frame, width, height, &layout);

        let y_size = width * height;
        let uv_size = (width / 2) * (height / 2);
        let yuv = YUVSlices::new(
            (
                &self.yuv_buf[..y_size],
                &self.yuv_buf[y_size..y_size + uv_size],
                &self.yuv_buf[y_size + uv_size..],
            ),
            (width, height),
            (width, width / 2, width / 2),
        );

        let bitstream =
            self.encoder.encode(&yuv).map_err(|e| format!("OpenH264: encode failed: {e}"))?;

        let data = bitstream.to_vec();
        if data.is_empty() {
            return Ok(Vec::new());
        }

        let is_keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);

        let output_metadata = metadata.map_or(
            Some(PacketMetadata {
                timestamp_us: None,
                duration_us: None,
                sequence: None,
                keyframe: Some(is_keyframe),
            }),
            |mut meta| {
                meta.keyframe = Some(is_keyframe);
                Some(meta)
            },
        );

        Ok(vec![EncodedPacket { data: Bytes::from(data), metadata: output_metadata }])
    }

    /// Fill `self.yuv_buf` with packed I420 data from a [`VideoFrame`]
    /// (I420 or NV12).  The buffer is reused across calls to avoid
    /// per-frame heap allocation.
    fn fill_yuv_buffer(
        &mut self,
        frame: &VideoFrame,
        width: usize,
        height: usize,
        layout: &streamkit_core::types::VideoLayout,
    ) {
        let data = frame.data.as_slice();
        let planes = layout.planes();
        let chroma_w = width / 2;
        let chroma_h = height / 2;

        // Build a contiguous I420 buffer: Y (w*h) + U (w/2*h/2) + V (w/2*h/2).
        let y_size = width * height;
        let uv_size = chroma_w * chroma_h;
        let total = y_size + 2 * uv_size;

        // Reuse the allocation; only re-allocates if the frame grew.
        self.yuv_buf.resize(total, 0);

        // Copy Y plane (common for both I420 and NV12).
        let y_plane = &planes[0];
        for row in 0..height {
            let src_start = y_plane.offset + row * y_plane.stride;
            let dst_start = row * width;
            self.yuv_buf[dst_start..dst_start + width]
                .copy_from_slice(&data[src_start..src_start + width]);
        }

        match frame.pixel_format {
            PixelFormat::I420 => {
                // U plane
                let u_plane = &planes[1];
                for row in 0..chroma_h {
                    let src_start = u_plane.offset + row * u_plane.stride;
                    let dst_start = y_size + row * chroma_w;
                    self.yuv_buf[dst_start..dst_start + chroma_w]
                        .copy_from_slice(&data[src_start..src_start + chroma_w]);
                }
                // V plane
                let v_plane = &planes[2];
                for row in 0..chroma_h {
                    let src_start = v_plane.offset + row * v_plane.stride;
                    let dst_start = y_size + uv_size + row * chroma_w;
                    self.yuv_buf[dst_start..dst_start + chroma_w]
                        .copy_from_slice(&data[src_start..src_start + chroma_w]);
                }
            },
            PixelFormat::Nv12 => {
                // NV12 has interleaved UV — de-interleave into separate U and V.
                let uv_plane = &planes[1];
                for row in 0..chroma_h {
                    let src_start = uv_plane.offset + row * uv_plane.stride;
                    for col in 0..chroma_w {
                        self.yuv_buf[y_size + row * chroma_w + col] = data[src_start + col * 2];
                        self.yuv_buf[y_size + uv_size + row * chroma_w + col] =
                            data[src_start + col * 2 + 1];
                    }
                }
            },
            _ => unreachable!("already checked above"),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_openh264_nodes(registry: &mut NodeRegistry) {
    let default_encoder = OpenH264EncoderNode::new(OpenH264EncoderConfig::default())
        .expect("default OpenH264 encoder config should be valid");
    registry.register_static_with_description(
        "video::openh264::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(OpenH264EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(OpenH264EncoderConfig))
            .expect("OpenH264EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "h264".to_string()],
        false,
        "Encodes raw video frames (NV12 or I420) into H.264 Annex B packets using OpenH264 \
         (Constrained Baseline profile). Insert a video::pixel_convert node upstream if the \
         source outputs RGBA8.",
    );
}

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

    #[tokio::test]
    async fn test_openh264_encode_produces_packets() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = OpenH264EncoderConfig { bitrate_kbps: 2000, max_frame_rate: 30.0 };
        let encoder = OpenH264EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..5 {
            let timestamp = 1_000 + 33_333_u64 * index;
            let duration: u64 = 33_333;

            let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp),
                duration_us: Some(duration),
                sequence: Some(index),
                keyframe: Some(index == 0),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "OpenH264 encoder produced no packets");

        for packet in &encoded_packets {
            match packet {
                Packet::Binary { data, content_type, metadata, .. } => {
                    assert!(!data.is_empty(), "Encoded packet should have data");
                    assert_eq!(
                        content_type.as_deref(),
                        Some(H264_CONTENT_TYPE),
                        "Content type should be video/h264"
                    );
                    assert!(metadata.is_some(), "Encoded packet should have metadata");
                },
                _ => panic!("Expected Binary packet from OpenH264 encoder"),
            }
        }
    }

    #[tokio::test]
    async fn test_openh264_encode_nv12_input() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder = OpenH264EncoderNode::new(OpenH264EncoderConfig::default()).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..3 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
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
        assert!(!encoded_packets.is_empty(), "OpenH264 encoder produced no packets from NV12");
    }

    #[tokio::test]
    async fn test_openh264_metadata_propagation() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder = OpenH264EncoderNode::new(OpenH264EncoderConfig::default()).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        let timestamps: Vec<u64> = vec![1_000, 34_333, 67_666];
        for (i, &ts) in timestamps.iter().enumerate() {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
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
        assert!(!encoded_packets.is_empty(), "Encoder should produce at least one packet");

        for (i, packet) in encoded_packets.iter().enumerate() {
            match packet {
                Packet::Binary { metadata, .. } => {
                    assert!(metadata.is_some(), "Encoded packet {i} should have metadata");
                    let meta = metadata.as_ref().unwrap();
                    assert!(
                        meta.keyframe.is_some(),
                        "Encoded packet {i} should have keyframe flag set"
                    );
                },
                _ => panic!("Expected Binary packet from OpenH264 encoder"),
            }
        }
    }

    #[test]
    fn test_config_validation_zero_bitrate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 0,
            max_frame_rate: 30.0,
        });
        assert!(result.is_err(), "bitrate_kbps=0 should be rejected");
    }

    #[test]
    fn test_config_validation_negative_frame_rate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 2000,
            max_frame_rate: -1.0,
        });
        assert!(result.is_err(), "negative max_frame_rate should be rejected");
    }

    #[test]
    fn test_config_validation_zero_frame_rate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 2000,
            max_frame_rate: 0.0,
        });
        assert!(result.is_err(), "zero max_frame_rate should be rejected");
    }

    #[test]
    fn test_config_validation_nan_frame_rate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 2000,
            max_frame_rate: f32::NAN,
        });
        assert!(result.is_err(), "NaN max_frame_rate should be rejected");
    }

    #[test]
    fn test_config_validation_infinity_frame_rate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 2000,
            max_frame_rate: f32::INFINITY,
        });
        assert!(result.is_err(), "INFINITY max_frame_rate should be rejected");

        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 2000,
            max_frame_rate: f32::NEG_INFINITY,
        });
        assert!(result.is_err(), "NEG_INFINITY max_frame_rate should be rejected");
    }

    #[test]
    fn test_config_validation_excessive_bitrate() {
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 500_001,
            max_frame_rate: 30.0,
        });
        assert!(result.is_err(), "bitrate_kbps above 500_000 should be rejected");

        // Boundary: 500_000 should be accepted.
        let result = OpenH264EncoderNode::new(OpenH264EncoderConfig {
            bitrate_kbps: 500_000,
            max_frame_rate: 30.0,
        });
        assert!(result.is_ok(), "bitrate_kbps=500_000 should be accepted");
    }

    #[test]
    fn test_odd_dimensions_rejected() {
        let config = OpenH264EncoderConfig::default();
        let mut encoder = OpenH264Encoder::new(64, 64, &config).unwrap();

        let frame = create_test_video_frame(63, 64, PixelFormat::I420, 16);
        match encoder.encode_frame(&frame, None) {
            Err(msg) => assert!(
                msg.contains("even dimensions"),
                "error should mention even dimensions, got: {msg}"
            ),
            Ok(_) => panic!("odd width should be rejected"),
        }

        let frame = create_test_video_frame(64, 63, PixelFormat::I420, 16);
        match encoder.encode_frame(&frame, None) {
            Err(msg) => assert!(
                msg.contains("even dimensions"),
                "error should mention even dimensions, got: {msg}"
            ),
            Ok(_) => panic!("odd height should be rejected"),
        }
    }
}
