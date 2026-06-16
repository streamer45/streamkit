// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pixel-format conversion node.
//!
//! Converts raw video frames between RGBA8, NV12, and I420 pixel formats.
//! Runs the CPU-heavy conversion on a persistent `spawn_blocking` thread
//! (same pattern as the VP9 encoder) and uses a **single-entry** cache:
//! when the input `Arc<PooledVideoData>` pointer hasn't changed, the most
//! recent conversion result is re-sent (zero-cost passthrough for static
//! scenes).
//!
//! ## Memory bound
//!
//! The cache holds at most **one** converted frame (the most recent).
//! New conversions overwrite the previous entry, so memory usage is O(1)
//! regardless of how many distinct format pairs are converted over time.
//! The `PixelFormat` enum currently has 3 variants giving 4 supported
//! conversion pairs — all well within a single-entry cache strategy.
//!
//! Supported conversions:
//! - RGBA8 → NV12
//! - RGBA8 → I420
//! - NV12  → RGBA8
//! - I420  → RGBA8
//!
//! Unsupported pairs (e.g. NV12 ↔ I420) return an error rather than
//! silently chaining two conversions.

use async_trait::async_trait;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{Packet, PacketType, PixelFormat, RawVideoFormat, VideoFrame};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, PooledVideoData, ProcessorNode,
    StreamKitError,
};
use tokio::sync::mpsc;

use super::parse_pixel_format;
use crate::video::pixel_ops::{
    i420_to_rgba8_buf, nv12_to_rgba8_buf, rgba8_to_i420_buf, rgba8_to_nv12_buf,
};

/// Configuration for the pixel format converter node.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PixelConvertConfig {
    /// Target pixel format: `"nv12"` (default), `"i420"`, or `"rgba8"`.
    pub output_format: String,
}

impl Default for PixelConvertConfig {
    fn default() -> Self {
        Self { output_format: "nv12".to_string() }
    }
}

/// Converts raw video frames between pixel formats (RGBA8, NV12, I420).
///
/// When the input format already matches the target, the frame is forwarded
/// unchanged (zero allocation).  When the input `Arc<PooledVideoData>`
/// pointer is identical to the previous frame, the single-entry cache
/// returns the previous conversion result (ref-count bump only, no
/// conversion work).  The cache is bounded to exactly one frame — see the
/// module-level documentation for the memory-bound rationale.
pub struct PixelConvertNode {
    target_format: PixelFormat,
}

impl PixelConvertNode {
    /// Create a new pixel convert node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `output_format` is not a recognised pixel format.
    pub fn new(config: &PixelConvertConfig) -> Result<Self, StreamKitError> {
        let target_format = parse_pixel_format(&config.output_format)?;
        Ok(Self { target_format })
    }
}

#[async_trait]
impl ProcessorNode for PixelConvertNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Rgba8,
                }),
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
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: self.target_format,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;

        tracing::info!("PixelConvertNode starting: target_format={:?}", self.target_format);

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("pixel_convert_packets_processed").build();

        let target_format = self.target_format;
        let otel_node_name = node_name.clone();
        let video_pool = context.video_pool.clone();

        let (convert_tx, mut convert_rx) =
            mpsc::channel::<VideoFrame>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let convert_task = tokio::task::spawn_blocking(move || {
            let meter = global::meter("skit_nodes");
            let frames_converted_counter = meter
                .u64_counter("pixel_convert.frames_converted")
                .with_description("Frames that required pixel format conversion")
                .build();
            let frames_passthrough_counter = meter
                .u64_counter("pixel_convert.frames_passthrough")
                .with_description("Frames forwarded unchanged (same format or same Arc pointer)")
                .build();
            let conversion_duration_histogram = meter
                .f64_histogram("pixel_convert.conversion_duration")
                .with_description("Seconds per pixel format conversion")
                .with_boundaries(
                    streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec(),
                )
                .build();
            let otel_attrs = [KeyValue::new("node", otel_node_name)];

            // Single-entry Arc-pointer cache: re-send the last result when
            // the input Arc pointer hasn't changed (zero-cost for static scenes).
            let mut last_input_ptr: usize = 0;
            let mut cached_output: Option<Arc<PooledVideoData>> = None;
            let mut cached_output_format: Option<PixelFormat> = None;
            let mut cached_width: u32 = 0;
            let mut cached_height: u32 = 0;

            while let Some(frame) = convert_rx.blocking_recv() {
                if frame.pixel_format == target_format {
                    frames_passthrough_counter.add(1, &otel_attrs);
                    if result_tx.blocking_send(Ok(frame)).is_err() {
                        break;
                    }
                    continue;
                }

                let current_ptr = Arc::as_ptr(&frame.data) as usize;
                if current_ptr == last_input_ptr
                    && last_input_ptr != 0
                    && cached_output.is_some()
                    && cached_output_format == Some(target_format)
                    && cached_width == frame.width
                    && cached_height == frame.height
                {
                    #[allow(clippy::unwrap_used)] // guarded by is_some() check above
                    let cached_data = cached_output.clone().unwrap();
                    let result = VideoFrame::from_arc(
                        frame.width,
                        frame.height,
                        target_format,
                        cached_data,
                        frame.metadata.clone(),
                    );
                    match result {
                        Ok(out_frame) => {
                            frames_passthrough_counter.add(1, &otel_attrs);
                            if result_tx.blocking_send(Ok(out_frame)).is_err() {
                                break;
                            }
                        },
                        Err(err) => {
                            let _ = result_tx.blocking_send(Err(err.to_string()));
                        },
                    }
                    continue;
                }

                let convert_start = Instant::now();
                let result = convert_frame(&frame, target_format, video_pool.as_deref());
                let duration = convert_start.elapsed();

                match result {
                    Ok(out_frame) => {
                        frames_converted_counter.add(1, &otel_attrs);
                        conversion_duration_histogram.record(duration.as_secs_f64(), &otel_attrs);

                        last_input_ptr = current_ptr;
                        cached_output = Some(Arc::clone(&out_frame.data));
                        cached_output_format = Some(target_format);
                        cached_width = out_frame.width;
                        cached_height = out_frame.height;

                        if result_tx.blocking_send(Ok(out_frame)).is_err() {
                            break;
                        }
                    },
                    Err(err) => {
                        let _ = result_tx.blocking_send(Err(err.to_string()));
                    },
                }
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

        let convert_tx_clone = convert_tx.clone();
        let mut input_task = tokio::spawn(async move {
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Video(frame) = packet {
                        if convert_tx_clone.send(frame).await.is_err() {
                            tracing::error!(
                                "PixelConvertNode convert task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("PixelConvertNode input stream closed");
        });

        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            convert_task,
            convert_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            Packet::Video,
            "PixelConvertNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("PixelConvertNode shutting down.");
        Ok(())
    }
}

/// Convert a `VideoFrame` to the given `target_format`.
///
/// Allocates the output buffer from `video_pool` when available.
///
/// # Errors
///
/// Returns an error for unsupported conversion pairs (e.g. NV12 ↔ I420).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn convert_frame(
    frame: &VideoFrame,
    target_format: PixelFormat,
    video_pool: Option<&streamkit_core::VideoFramePool>,
) -> Result<VideoFrame, StreamKitError> {
    let w = frame.width as usize;
    let h = frame.height as usize;

    let out_size = match target_format {
        PixelFormat::Rgba8 => w * h * 4,
        PixelFormat::Nv12 => {
            let chroma_w = w.div_ceil(2);
            let chroma_h = h.div_ceil(2);
            w * h + chroma_w * 2 * chroma_h
        },
        PixelFormat::I420 => {
            let chroma_w = w.div_ceil(2);
            let chroma_h = h.div_ceil(2);
            w * h + chroma_w * chroma_h * 2
        },
        other => {
            return Err(StreamKitError::Runtime(format!(
                "unsupported target pixel format: {other:?}"
            )));
        },
    };

    let mut out_data = video_pool
        .map_or_else(|| PooledVideoData::from_vec(vec![0u8; out_size]), |pool| pool.get(out_size));

    match (frame.pixel_format, target_format) {
        (PixelFormat::Rgba8, PixelFormat::Nv12) => {
            rgba8_to_nv12_buf(frame.data(), frame.width, frame.height, out_data.as_mut_slice());
        },
        (PixelFormat::Rgba8, PixelFormat::I420) => {
            rgba8_to_i420_buf(frame.data(), frame.width, frame.height, out_data.as_mut_slice());
        },
        (PixelFormat::Nv12, PixelFormat::Rgba8) => {
            nv12_to_rgba8_buf(frame.data(), frame.width, frame.height, out_data.as_mut_slice());
        },
        (PixelFormat::I420, PixelFormat::Rgba8) => {
            i420_to_rgba8_buf(frame.data(), frame.width, frame.height, out_data.as_mut_slice());
        },
        (src, dst) => {
            return Err(StreamKitError::Runtime(format!(
                "Unsupported pixel format conversion: {src:?} → {dst:?}. \
                 Only RGBA8 ↔ NV12 and RGBA8 ↔ I420 are supported."
            )));
        },
    }

    VideoFrame::from_pooled(
        frame.width,
        frame.height,
        target_format,
        out_data,
        frame.metadata.clone(),
    )
}

use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)] // Default config and schema serialization should never fail
pub fn register_pixel_convert_nodes(registry: &mut NodeRegistry) {
    let default_node = PixelConvertNode::new(&PixelConvertConfig::default())
        .expect("default PixelConvertConfig should be valid");
    register_static_node!(
        registry,
        "video::pixel_convert",
        |params| {
            let config: PixelConvertConfig = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(PixelConvertNode::new(&config)?))
        },
        PixelConvertConfig,
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        ["video", "convert"],
        "Converts raw video frames between pixel formats (RGBA8, NV12, I420). \
         Insert upstream of nodes that require a specific format (e.g. VP9 encoder). \
         Passthrough when input format already matches the target.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
        create_test_video_frame,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_passthrough_same_format() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Target is NV12, send NV12 frames — should passthrough.
        let node = PixelConvertNode::new(&PixelConvertConfig { output_format: "nv12".to_string() })
            .unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 128);
        let original_data_ptr = Arc::as_ptr(&frame.data) as usize;
        input_tx.send(Packet::Video(frame)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1, "Expected 1 output packet");

        if let Packet::Video(out_frame) = &output_packets[0] {
            assert_eq!(out_frame.pixel_format, PixelFormat::Nv12);
            // Verify the Arc pointer is identical (zero-copy passthrough).
            let out_data_ptr = Arc::as_ptr(&out_frame.data) as usize;
            assert_eq!(
                original_data_ptr, out_data_ptr,
                "Passthrough should preserve the same Arc pointer"
            );
        } else {
            panic!("Expected Video packet");
        }
    }

    #[tokio::test]
    async fn test_identical_frame_caching() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PixelConvertNode::new(&PixelConvertConfig { output_format: "nv12".to_string() })
            .unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send the same RGBA8 frame twice (clone preserves the Arc).
        let frame = create_test_video_frame(64, 64, PixelFormat::Rgba8, 128);
        let frame_clone = frame.clone();
        assert_eq!(
            Arc::as_ptr(&frame.data) as usize,
            Arc::as_ptr(&frame_clone.data) as usize,
            "Cloned frames should share the same Arc"
        );

        input_tx.send(Packet::Video(frame)).await.unwrap();

        // Wait for the first output packet before sending the second so that
        // the caching logic has a converted result to compare Arc pointers
        // against.  This replaces a fragile `sleep(50ms)` that could fail
        // under CI load.
        let first_output = mock_sender
            .recv_timeout(std::time::Duration::from_secs(5))
            .await
            .expect("Timed out waiting for first output packet");

        input_tx.send(Packet::Video(frame_clone)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Collect the remaining output and prepend the first packet we
        // already consumed above.
        let mut output_packets = vec![first_output.2];
        output_packets.extend(mock_sender.get_packets_for_pin("out").await);
        assert_eq!(output_packets.len(), 2, "Expected 2 output packets");

        // Both outputs should be NV12.
        for pkt in &output_packets {
            if let Packet::Video(f) = pkt {
                assert_eq!(f.pixel_format, PixelFormat::Nv12);
            } else {
                panic!("Expected Video packet");
            }
        }

        // The second output should reuse the cached Arc (same pointer).
        if let (Packet::Video(f1), Packet::Video(f2)) = (&output_packets[0], &output_packets[1]) {
            let ptr1 = Arc::as_ptr(&f1.data) as usize;
            let ptr2 = Arc::as_ptr(&f2.data) as usize;
            assert_eq!(ptr1, ptr2, "Second frame should reuse cached Arc from first conversion");
        }
    }

    #[tokio::test]
    async fn test_rgba8_to_nv12_conversion() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PixelConvertNode::new(&PixelConvertConfig { output_format: "nv12".to_string() })
            .unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let frame = create_test_video_frame(64, 64, PixelFormat::Rgba8, 128);
        input_tx.send(Packet::Video(frame)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        if let Packet::Video(out_frame) = &output_packets[0] {
            assert_eq!(out_frame.pixel_format, PixelFormat::Nv12);
            assert_eq!(out_frame.width, 64);
            assert_eq!(out_frame.height, 64);
            assert!(!out_frame.data().is_empty());
        } else {
            panic!("Expected Video packet");
        }
    }

    #[tokio::test]
    async fn test_rgba8_to_nv12_roundtrip() {
        // Convert RGBA8 → NV12 via the raw buf functions and then
        // NV12 → RGBA8, verifying pixel values are within tolerance.
        let width: u32 = 64;
        let height: u32 = 64;
        let w = width as usize;
        let h = height as usize;

        // Create a test RGBA8 buffer with known values.
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                rgba[off] = 200; // R
                rgba[off + 1] = 100; // G
                rgba[off + 2] = 50; // B
                rgba[off + 3] = 255; // A
            }
        }

        // RGBA8 → NV12
        let chroma_w = w.div_ceil(2);
        let chroma_h = h.div_ceil(2);
        let nv12_size = w * h + chroma_w * 2 * chroma_h;
        let mut nv12 = vec![0u8; nv12_size];
        rgba8_to_nv12_buf(&rgba, width, height, &mut nv12);

        // NV12 → RGBA8
        let mut decoded = vec![0u8; w * h * 4];
        nv12_to_rgba8_buf(&nv12, width, height, &mut decoded);

        // Verify pixel values are within tolerance (YUV roundtrip has some loss).
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                let dr = (i32::from(decoded[off]) - 200).unsigned_abs();
                let dg = (i32::from(decoded[off + 1]) - 100).unsigned_abs();
                let db = (i32::from(decoded[off + 2]) - 50).unsigned_abs();
                assert!(dr <= 3, "R channel diff too large at ({x},{y}): {dr}");
                assert!(dg <= 3, "G channel diff too large at ({x},{y}): {dg}");
                assert!(db <= 3, "B channel diff too large at ({x},{y}): {db}");
            }
        }
    }

    #[tokio::test]
    async fn test_unsupported_conversion_pair() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Target is I420, but we'll send NV12 — unsupported pair.
        let node = PixelConvertNode::new(&PixelConvertConfig { output_format: "i420".to_string() })
            .unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 128);
        input_tx.send(Packet::Video(frame)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // The unsupported conversion should produce no output (error logged).
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 0, "Unsupported conversion should produce no output");
    }

    #[test]
    fn test_invalid_output_format() {
        let result =
            PixelConvertNode::new(&PixelConvertConfig { output_format: "yuv444".to_string() });
        assert!(result.is_err());
    }
}
