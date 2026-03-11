// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! SMPTE EIA 75% color bars video generator.
//!
//! Produces raw video frames with the standard 7-bar test pattern.
//! Supports NV12 (default), I420, and RGBA8 pixel formats.
//! Configurable resolution, frame rate, and frame count.
//!
//! - `frame_count > 0`: batch mode — emits exactly N frames with synthetic timestamps (oneshot).
//! - `frame_count == 0`: real-time mode — emits indefinitely, paced by `tokio::time::interval` (dynamic).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat};
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
    /// Output pixel format. Supported: "nv12" (default), "i420", and "rgba8".
    #[serde(default = "default_pixel_format")]
    pub pixel_format: String,
    /// When `true`, draws the current wall-clock time (`HH:MM:SS.mmm`)
    /// onto each generated frame using a monospace font.
    #[serde(default)]
    pub draw_time: bool,
    /// Optional filesystem path to a custom TTF/OTF font used for the
    /// `draw_time` overlay.  When omitted the bundled DejaVu Sans Mono
    /// font (embedded in the binary) is used.
    #[serde(default)]
    pub draw_time_font_path: Option<String>,
    /// When `true`, horizontally scrolls the color bars each frame so that
    /// every frame differs substantially from the previous one.  Useful for
    /// encoding benchmarks where static content would compress to nearly
    /// nothing.
    #[serde(default)]
    pub animate: bool,
}

fn default_pixel_format() -> String {
    "nv12".to_string()
}

// Re-export the shared parse_pixel_format from the parent module.
use super::parse_pixel_format;

impl Default for ColorBarsConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            frame_count: default_frame_count(),
            pixel_format: default_pixel_format(),
            draw_time: false,
            draw_time_font_path: None,
            animate: false,
        }
    }
}

/// Source node that generates SMPTE EIA 75% color bar frames.
///
/// No input pins. Outputs `PacketType::RawVideo` on `"out"` in the
/// configured pixel format (I420 or RGBA8).
/// Follows the Ready → Start lifecycle (like `FileReadNode`).
pub struct ColorBarsNode {
    config: ColorBarsConfig,
    /// Resolved pixel format from the config string.
    pixel_format: PixelFormat,
}

#[async_trait]
impl ProcessorNode for ColorBarsNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: self.pixel_format,
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

        let pixel_format = self.pixel_format;

        // Pre-load the monospace font for draw_time (once, if enabled).
        let draw_time_font = if self.config.draw_time {
            // If the user specified a custom font path, try that first;
            // otherwise use the compile-time embedded DejaVu Sans Mono.
            let font_bytes = self.config.draw_time_font_path.as_ref().map_or_else(
                || crate::video::fonts::DEFAULT_MONO_FONT_DATA.to_vec(),
                |path| match std::fs::read(path) {
                    Ok(bytes) => {
                        tracing::info!("draw_time: loaded custom font from {path}");
                        bytes
                    },
                    Err(e) => {
                        tracing::warn!(
                            "draw_time: failed to read custom font '{path}': {e}, \
                             falling back to bundled DejaVu Sans Mono"
                        );
                        crate::video::fonts::DEFAULT_MONO_FONT_DATA.to_vec()
                    },
                },
            );

            match fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default()) {
                Ok(f) => {
                    tracing::info!("draw_time enabled: font ready");
                    Some(f)
                },
                Err(e) => {
                    tracing::warn!("draw_time: failed to parse font: {e}");
                    None
                },
            }
        } else {
            None
        };

        // Pre-generate the color bar pattern into a template buffer.
        let layout = streamkit_core::types::VideoLayout::packed(width, height, pixel_format);
        let total_bytes = layout.total_bytes();
        let mut template = vec![0u8; total_bytes];
        match pixel_format {
            PixelFormat::I420 => {
                generate_smpte_colorbars_i420(width, height, &mut template, &layout);
            },
            PixelFormat::Nv12 => {
                generate_smpte_colorbars_nv12(width, height, &mut template, &layout);
            },
            PixelFormat::Rgba8 => generate_smpte_colorbars_rgba8(width, height, &mut template),
        }

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
            let animate = self.config.animate;
            let frame = if let Some(pool) = &context.video_pool {
                let mut pooled = pool.get(total_bytes);
                #[allow(clippy::cast_possible_truncation)]
                if animate {
                    let offset_px = seq as usize * ANIMATE_SCROLL_PX;
                    scroll_frame(
                        &template,
                        pooled.as_mut_slice(),
                        pixel_format,
                        &layout,
                        offset_px,
                    );
                } else {
                    pooled.as_mut_slice()[..total_bytes].copy_from_slice(&template);
                }
                if let Some(ref font) = draw_time_font {
                    stamp_time(pooled.as_mut_slice(), width, height, pixel_format, &layout, font);
                }
                streamkit_core::types::VideoFrame::from_pooled(
                    width,
                    height,
                    pixel_format,
                    pooled,
                    metadata,
                )?
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let mut data = if animate {
                    let offset_px = seq as usize * ANIMATE_SCROLL_PX;
                    let mut buf = vec![0u8; total_bytes];
                    scroll_frame(&template, &mut buf, pixel_format, &layout, offset_px);
                    buf
                } else {
                    template.clone()
                };
                if let Some(ref font) = draw_time_font {
                    stamp_time(&mut data, width, height, pixel_format, &layout, font);
                }
                streamkit_core::types::VideoFrame::with_metadata(
                    width,
                    height,
                    pixel_format,
                    data,
                    metadata,
                )?
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

/// SMPTE EIA 75% color bars in RGBA8 format.
///
/// Same bar order and approximate 75% amplitude as the YUV table,
/// converted to full-range RGB.
const SMPTE_BARS_RGBA: [(u8, u8, u8, u8); 7] = [
    (191, 191, 191, 255), // white  (75%)
    (191, 191, 0, 255),   // yellow
    (0, 191, 191, 255),   // cyan
    (0, 191, 0, 255),     // green
    (191, 0, 191, 255),   // magenta
    (191, 0, 0, 255),     // red
    (0, 0, 191, 255),     // blue
];

/// Fills an RGBA8 buffer with SMPTE 75% color bars.
fn generate_smpte_colorbars_rgba8(width: u32, height: u32, data: &mut [u8]) {
    let bar_count = SMPTE_BARS_RGBA.len();
    let stride = width as usize * 4;
    for row in 0..height as usize {
        for col in 0..width as usize {
            let bar_idx = col * bar_count / width as usize;
            let (r, g, b, a) = SMPTE_BARS_RGBA[bar_idx];
            let offset = row * stride + col * 4;
            data[offset] = r;
            data[offset + 1] = g;
            data[offset + 2] = b;
            data[offset + 3] = a;
        }
    }
}

/// Fills an NV12 buffer with SMPTE 75% color bars.
///
/// Same YUV values as I420 but U and V are interleaved in a single chroma plane.
fn generate_smpte_colorbars_nv12(
    width: u32,
    height: u32,
    data: &mut [u8],
    layout: &streamkit_core::types::VideoLayout,
) {
    let planes = layout.planes();
    let y_plane = planes[0];
    let uv_plane = planes[1];

    let bar_count = SMPTE_BARS_YUV.len();

    // Fill Y plane (identical to I420).
    for row in 0..height as usize {
        for col in 0..width as usize {
            let bar_idx = col * bar_count / width as usize;
            let (y, _, _) = SMPTE_BARS_YUV[bar_idx];
            data[y_plane.offset + row * y_plane.stride + col] = y;
        }
    }

    // Fill interleaved UV plane (half resolution).
    let chroma_w = (width + 1) as usize / 2;
    let chroma_h = uv_plane.height as usize;
    for row in 0..chroma_h {
        for col in 0..chroma_w {
            let src_col = col * 2;
            let bar_idx = src_col * bar_count / width as usize;
            let (_, u, v) = SMPTE_BARS_YUV[bar_idx];
            let offset = uv_plane.offset + row * uv_plane.stride + col * 2;
            data[offset] = u;
            data[offset + 1] = v;
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

// ── Animation (horizontal scroll) ───────────────────────────────────────────

/// Pixels scrolled per frame when `animate` is enabled.
const ANIMATE_SCROLL_PX: usize = 4;

/// Horizontally rotate a single plane by `offset_bytes`, writing into `dst`.
#[allow(clippy::cast_possible_truncation)]
fn rotate_plane_rows(
    src: &[u8],
    dst: &mut [u8],
    plane_offset: usize,
    stride: usize,
    data_width: usize,
    height: usize,
    offset_bytes: usize,
) {
    let off = offset_bytes % data_width;
    if off == 0 {
        let len = stride * height;
        dst[plane_offset..plane_offset + len]
            .copy_from_slice(&src[plane_offset..plane_offset + len]);
        return;
    }
    for row in 0..height {
        let base = plane_offset + row * stride;
        dst[base..base + data_width - off].copy_from_slice(&src[base + off..base + data_width]);
        dst[base + data_width - off..base + data_width].copy_from_slice(&src[base..base + off]);
    }
}

/// Scroll the entire frame (all planes) by `offset_px` luma pixels.
#[allow(clippy::cast_possible_truncation)]
fn scroll_frame(
    template: &[u8],
    dst: &mut [u8],
    pixel_format: PixelFormat,
    layout: &streamkit_core::types::VideoLayout,
    offset_px: usize,
) {
    // Round down to even so chroma stays aligned with 4:2:0 subsampling.
    let offset_px = offset_px & !1;
    let planes = layout.planes();

    match pixel_format {
        PixelFormat::Rgba8 => {
            let p = planes[0];
            rotate_plane_rows(
                template,
                dst,
                p.offset,
                p.stride,
                p.stride,
                p.height as usize,
                offset_px * 4,
            );
        },
        PixelFormat::I420 => {
            let y = planes[0];
            rotate_plane_rows(
                template,
                dst,
                y.offset,
                y.stride,
                y.stride,
                y.height as usize,
                offset_px,
            );
            let chroma_off = offset_px / 2;
            let u = planes[1];
            rotate_plane_rows(
                template,
                dst,
                u.offset,
                u.stride,
                u.stride,
                u.height as usize,
                chroma_off,
            );
            let v = planes[2];
            rotate_plane_rows(
                template,
                dst,
                v.offset,
                v.stride,
                v.stride,
                v.height as usize,
                chroma_off,
            );
        },
        PixelFormat::Nv12 => {
            let y = planes[0];
            rotate_plane_rows(
                template,
                dst,
                y.offset,
                y.stride,
                y.stride,
                y.height as usize,
                offset_px,
            );
            // UV plane: each chroma position is 2 bytes (U+V interleaved).
            let uv = planes[1];
            let chroma_off_bytes = (offset_px / 2) * 2;
            rotate_plane_rows(
                template,
                dst,
                uv.offset,
                uv.stride,
                uv.stride,
                uv.height as usize,
                chroma_off_bytes,
            );
        },
    }
}

// ── draw_time stamping ──────────────────────────────────────────────────────

/// Font size (px) used for the wall-clock timestamp overlay.
const DRAW_TIME_FONT_SIZE: f32 = 24.0;

/// Stamp the current wall-clock time (`HH:MM:SS.mmm`) onto a frame buffer.
///
/// Works for both RGBA8 and I420 pixel formats.  For I420 the text is
/// rasterized into a tiny RGBA scratch area and then each lit pixel is
/// converted to YUV and poked into the Y/U/V planes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
fn stamp_time(
    data: &mut [u8],
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    layout: &streamkit_core::types::VideoLayout,
    font: &fontdue::Font,
) {
    use std::time::SystemTime;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let total_secs = now.as_secs();
    let millis = now.subsec_millis();
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hrs = (total_secs / 3600) % 24;
    let time_str = format!("{hrs:02}:{mins:02}:{secs:02}.{millis:03}");

    // Placement: bottom-left with a small margin.
    let margin_x: i32 = 8;
    let margin_y: i32 = 8;
    let origin_y = height as i32 - margin_y - DRAW_TIME_FONT_SIZE as i32;

    match pixel_format {
        PixelFormat::Rgba8 => {
            // White, fully opaque text.
            super::blit_text_rgba(
                data,
                width,
                height,
                font,
                DRAW_TIME_FONT_SIZE,
                &time_str,
                margin_x,
                origin_y,
                [255, 255, 255, 255],
            );
        },
        PixelFormat::I420 | PixelFormat::Nv12 => {
            // YUV formats need direct plane manipulation — no shared RGBA
            // utility applies here.
            let (ref_metrics, _) = font.rasterize('A', DRAW_TIME_FONT_SIZE);
            let baseline_y = ref_metrics.height as f32;
            let planes = layout.planes();

            let mut cursor_x: f32 = 0.0;

            for ch in time_str.chars() {
                let (metrics, bitmap) = font.rasterize(ch, DRAW_TIME_FONT_SIZE);

                let gx = margin_x + (cursor_x + metrics.xmin as f32) as i32;
                let gy =
                    origin_y + (baseline_y - metrics.ymin as f32) as i32 - metrics.height as i32;

                for row in 0..metrics.height {
                    let dst_y = gy + row as i32;
                    if dst_y < 0 || dst_y >= height as i32 {
                        continue;
                    }
                    for col in 0..metrics.width {
                        let dst_x = gx + col as i32;
                        if dst_x < 0 || dst_x >= width as i32 {
                            continue;
                        }
                        let coverage = bitmap[row * metrics.width + col];
                        if coverage == 0 {
                            continue;
                        }

                        let px = dst_x as usize;
                        let py = dst_y as usize;

                        let y_plane = planes[0];

                        // White in YUV = Y:235, U:128, V:128
                        let alpha = u16::from(coverage);
                        let inv = 255 - alpha;

                        let y_off = y_plane.offset + py * y_plane.stride + px;
                        let old_y = u16::from(data[y_off]);
                        data[y_off] = ((235 * alpha + old_y * inv + 128) / 255) as u8;

                        // Chroma planes are half-resolution; update only once
                        // per 2×2 block (when both coords are even).
                        if px.is_multiple_of(2) && py.is_multiple_of(2) {
                            let cx = px / 2;
                            let cy = py / 2;
                            match pixel_format {
                                PixelFormat::I420 => {
                                    let u_plane = planes[1];
                                    let v_plane = planes[2];
                                    let u_off = u_plane.offset + cy * u_plane.stride + cx;
                                    let v_off = v_plane.offset + cy * v_plane.stride + cx;
                                    let old_u = u16::from(data[u_off]);
                                    let old_v = u16::from(data[v_off]);
                                    data[u_off] = ((128 * alpha + old_u * inv + 128) / 255) as u8;
                                    data[v_off] = ((128 * alpha + old_v * inv + 128) / 255) as u8;
                                },
                                PixelFormat::Nv12 => {
                                    let uv_plane = planes[1];
                                    let uv_off = uv_plane.offset + cy * uv_plane.stride + cx * 2;
                                    let old_u = u16::from(data[uv_off]);
                                    let old_v = u16::from(data[uv_off + 1]);
                                    data[uv_off] = ((128 * alpha + old_u * inv + 128) / 255) as u8;
                                    data[uv_off + 1] =
                                        ((128 * alpha + old_v * inv + 128) / 255) as u8;
                                },
                                PixelFormat::Rgba8 => unreachable!(),
                            }
                        }
                    }
                }

                cursor_x += metrics.advance_width;
                if (margin_x as f32 + cursor_x) >= width as f32 {
                    break;
                }
            }
        },
    }
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_colorbars_nodes(registry: &mut NodeRegistry) {
    let default_node =
        ColorBarsNode { config: ColorBarsConfig::default(), pixel_format: PixelFormat::Nv12 };
    registry.register_static_with_description(
        "video::colorbars",
        |params| {
            let config: ColorBarsConfig = config_helpers::parse_config_optional(params)?;
            let pixel_format = parse_pixel_format(&config.pixel_format)?;
            Ok(Box::new(ColorBarsNode { config, pixel_format }))
        },
        serde_json::to_value(schema_for!(ColorBarsConfig))
            .expect("ColorBarsConfig schema should serialize to JSON"),
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        vec!["video".to_string(), "generators".to_string()],
        false,
        "Generates SMPTE EIA 75% color bar test frames. \
         Supports NV12 (default), I420, and RGBA8 pixel formats via the pixel_format config. \
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
