// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! SMPTE EIA 75% color bars video generator.
//!
//! Produces raw I420 frames with the standard 7-bar test pattern.
//! Configurable resolution, frame rate, and frame count.
//!
//! - `frame_count > 0`: batch mode — emits exactly N frames with synthetic timestamps (oneshot).
//! - `frame_count == 0`: real-time mode — emits indefinitely, paced by `tokio::time::interval` (dynamic).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{Packet, PacketMetadata, PacketType, PixelFormat, VideoFormat};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

const fn default_width() -> u32 {
    640
}

const fn default_height() -> u32 {
    480
}

const fn default_fps() -> u32 {
    30
}

const fn default_frame_count() -> u32 {
    0
}

/// Configuration for the SMPTE color bars generator.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ColorBarsConfig {
    /// Frame width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Frame height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Frames per second.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Total frames to generate. 0 = infinite (real-time pacing).
    #[serde(default = "default_frame_count")]
    pub frame_count: u32,
}

impl Default for ColorBarsConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            frame_count: default_frame_count(),
        }
    }
}

/// Source node that generates SMPTE EIA 75% color bar I420 frames.
///
/// No input pins. Outputs `PacketType::RawVideo(I420)` on `"out"`.
/// Follows the Ready → Start lifecycle (like `FileReadNode`).
pub struct ColorBarsNode {
    config: ColorBarsConfig,
}

#[async_trait]
impl ProcessorNode for ColorBarsNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::I420,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let width = self.config.width;
        let height = self.config.height;
        let fps = self.config.fps;
        let frame_count = self.config.frame_count;
        let duration_us = if fps > 0 { 1_000_000 / u64::from(fps) } else { 33_333 };

        tracing::info!(
            "ColorBarsNode: {}x{} @ {} fps, frame_count={}",
            width,
            height,
            fps,
            frame_count
        );

        // Pre-generate the color bar pattern into a template buffer.
        let layout = streamkit_core::types::VideoLayout::packed(width, height, PixelFormat::I420);
        let total_bytes = layout.total_bytes();
        let mut template = vec![0u8; total_bytes];
        generate_smpte_colorbars_i420(width, height, &mut template, &layout);

        // Source nodes emit Ready state and wait for Start signal.
        state_helpers::emit_ready(&context.state_tx, &node_name);
        tracing::info!("ColorBarsNode ready, waiting for start signal");

        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Start) => {
                    tracing::info!("ColorBarsNode received start signal");
                    break;
                },
                Some(streamkit_core::control::NodeControlMessage::UpdateParams(_)) => {},
                Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                    tracing::info!("ColorBarsNode received shutdown before start");
                    return Ok(());
                },
                None => {
                    tracing::warn!("Control channel closed before start signal received");
                    return Ok(());
                },
            }
        }

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Set up real-time pacing for dynamic (frame_count == 0) mode.
        let mut interval = if frame_count == 0 && fps > 0 {
            let period = std::time::Duration::from_micros(duration_us);
            Some(tokio::time::interval(period))
        } else {
            None
        };

        let mut seq: u64 = 0;

        loop {
            // Honour finite frame count.
            if frame_count > 0 && seq >= u64::from(frame_count) {
                tracing::info!("ColorBarsNode finished after {} frames", seq);
                break;
            }

            // Check cancellation.
            if let Some(token) = &context.cancellation_token {
                if token.is_cancelled() {
                    tracing::info!("ColorBarsNode cancelled after {} frames", seq);
                    break;
                }
            }

            // Pace in real-time mode.
            if let Some(ref mut iv) = interval {
                tokio::select! {
                    _ = iv.tick() => {},
                    Some(msg) = context.control_rx.recv() => {
                        match msg {
                            streamkit_core::control::NodeControlMessage::Shutdown => {
                                tracing::info!("ColorBarsNode received shutdown during generation");
                                break;
                            },
                            streamkit_core::control::NodeControlMessage::UpdateParams(_)
                            | streamkit_core::control::NodeControlMessage::Start => {},
                        }
                        continue;
                    }
                }
            }

            let timestamp_us = seq * duration_us;
            let metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp_us),
                duration_us: Some(duration_us),
                sequence: Some(seq),
                keyframe: Some(true),
            });

            // Allocate frame from pool if available, otherwise from vec.
            let frame = if let Some(pool) = &context.video_pool {
                let mut pooled = pool.get(total_bytes);
                pooled.as_mut_slice()[..total_bytes].copy_from_slice(&template);
                draw_sweep_bar_i420(pooled.as_mut_slice(), width, height, &layout, seq);
                streamkit_core::types::VideoFrame::from_pooled(
                    width,
                    height,
                    PixelFormat::I420,
                    pooled,
                    metadata,
                )
            } else {
                let mut data = template.clone();
                draw_sweep_bar_i420(&mut data, width, height, &layout, seq);
                streamkit_core::types::VideoFrame::with_metadata(
                    width,
                    height,
                    PixelFormat::I420,
                    data,
                    metadata,
                )
            };

            if context.output_sender.send("out", Packet::Video(frame)).await.is_err() {
                tracing::debug!("Output channel closed, stopping ColorBarsNode");
                break;
            }

            stats_tracker.sent();
            stats_tracker.maybe_send();
            seq += 1;
        }

        stats_tracker.force_send();
        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
        Ok(())
    }
}

// ── SMPTE color bar generation ──────────────────────────────────────────────

/// SMPTE EIA 75% color bars (ITU-R BT.601 Y'CbCr).
///
/// Seven equal-width vertical bars, left to right:
///   White, Yellow, Cyan, Green, Magenta, Red, Blue
///
/// 75% amplitude values (studio range):
///   | Bar     |   Y  |   U (Cb) |   V (Cr) |
///   |---------|------|----------|----------|
///   | White   | 180  |  128     |  128     |
///   | Yellow  | 162  |   44     |  142     |
///   | Cyan    | 131  |  156     |   44     |
///   | Green   | 112  |   72     |   58     |
///   | Magenta |  84  |  184     |  198     |
///   | Red     |  65  |  100     |  212     |
///   | Blue    |  35  |  212     |  114     |
const SMPTE_BARS_YUV: [(u8, u8, u8); 7] = [
    (180, 128, 128), // white
    (162, 44, 142),  // yellow
    (131, 156, 44),  // cyan
    (112, 72, 58),   // green
    (84, 184, 198),  // magenta
    (65, 100, 212),  // red
    (35, 212, 114),  // blue
];

/// Draws a bright vertical sweep bar that moves across the frame each tick.
///
/// The bar is 4 pixels wide, pure white (Y=235, U=V=128), and its horizontal
/// position advances by 4 pixels per frame, wrapping around the width.
fn draw_sweep_bar_i420(
    data: &mut [u8],
    width: u32,
    height: u32,
    layout: &streamkit_core::types::VideoLayout,
    seq: u64,
) {
    let planes = layout.planes();
    let y_plane = planes[0];
    let u_plane = planes[1];
    let v_plane = planes[2];

    let bar_width: usize = 4;
    let speed: usize = 4; // pixels per frame
    let w = width as usize;
    let h = height as usize;
    let bar_x = (seq as usize * speed) % w;

    // Y plane: set bar columns to peak white (235).
    for row in 0..h {
        for dx in 0..bar_width {
            let col = (bar_x + dx) % w;
            data[y_plane.offset + row * y_plane.stride + col] = 235;
        }
    }

    // U and V planes (half resolution): set to neutral (128) for pure white.
    let chroma_w = u_plane.width as usize;
    let chroma_h = u_plane.height as usize;
    let chroma_bar_x = bar_x / 2;
    let chroma_bar_w = (bar_width + 1) / 2; // round up
    for row in 0..chroma_h {
        for dx in 0..chroma_bar_w {
            let col = (chroma_bar_x + dx) % chroma_w;
            data[u_plane.offset + row * u_plane.stride + col] = 128;
            data[v_plane.offset + row * v_plane.stride + col] = 128;
        }
    }
}

/// Fills an I420 buffer with SMPTE 75% color bars.
fn generate_smpte_colorbars_i420(
    width: u32,
    height: u32,
    data: &mut [u8],
    layout: &streamkit_core::types::VideoLayout,
) {
    let planes = layout.planes();
    let y_plane = planes[0];
    let u_plane = planes[1];
    let v_plane = planes[2];

    let bar_count = SMPTE_BARS_YUV.len();

    // Fill Y plane.
    for row in 0..height as usize {
        for col in 0..width as usize {
            let bar_idx = col * bar_count / width as usize;
            let (y, _, _) = SMPTE_BARS_YUV[bar_idx];
            data[y_plane.offset + row * y_plane.stride + col] = y;
        }
    }

    // Fill U and V planes (half resolution for I420).
    let chroma_w = u_plane.width as usize;
    let chroma_h = u_plane.height as usize;
    for row in 0..chroma_h {
        for col in 0..chroma_w {
            let src_col = col * 2;
            let bar_idx = src_col * bar_count / width as usize;
            let (_, u, v) = SMPTE_BARS_YUV[bar_idx];
            data[u_plane.offset + row * u_plane.stride + col] = u;
            data[v_plane.offset + row * v_plane.stride + col] = v;
        }
    }
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used)]
pub fn register_colorbars_nodes(registry: &mut NodeRegistry) {
    let default_node = ColorBarsNode { config: ColorBarsConfig::default() };
    registry.register_static_with_description(
        "video::colorbars",
        |params| {
            let config: ColorBarsConfig = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(ColorBarsNode { config }))
        },
        serde_json::to_value(schema_for!(ColorBarsConfig))
            .expect("ColorBarsConfig schema should serialize to JSON"),
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        vec!["video".to_string(), "generators".to_string()],
        false,
        "Generates SMPTE EIA 75% color bar test frames in I420 format. \
         Use with a video encoder for pipeline testing and validation.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_smpte_colorbars_i420_dimensions() {
        let width = 640u32;
        let height = 480u32;
        let layout = streamkit_core::types::VideoLayout::packed(width, height, PixelFormat::I420);
        let total = layout.total_bytes();
        let mut data = vec![0u8; total];
        generate_smpte_colorbars_i420(width, height, &mut data, &layout);

        // Y plane: first pixel should be white (Y=180).
        assert_eq!(data[0], 180);
        // Last bar (rightmost column) should be blue (Y=35).
        let last_y_col = (width - 1) as usize;
        assert_eq!(data[last_y_col], 35);
    }

    #[test]
    fn test_colorbars_config_defaults() {
        let config = ColorBarsConfig::default();
        assert_eq!(config.width, 640);
        assert_eq!(config.height, 480);
        assert_eq!(config.fps, 30);
        assert_eq!(config.frame_count, 0);
    }
}
