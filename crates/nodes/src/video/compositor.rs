// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video compositor node.
//!
//! Composites multiple raw video inputs onto a single RGBA8 output canvas with
//! optional image and text overlays. Supports dynamic pin creation for
//! attaching arbitrary inputs at runtime.
//!
//! - Inputs accept `RawVideo(RGBA8)` with wildcard dimensions.
//! - Output produces `RawVideo(RGBA8)` at the configured canvas size.
//! - Heavy compositing work runs on a persistent blocking thread (via
//!   `spawn_blocking`) to avoid blocking the async runtime and to keep CPU
//!   caches warm across frames.
//! - Row-level parallelism via `rayon` for blitting and pixel-format
//!   conversion.
//! - Image overlays are decoded once during initialization (PNG/JPEG via the
//!   `image` crate).
//! - Text overlays are rasterized via `tiny-skia` once per `UpdateParams`, not
//!   per frame.
//!
//! # Future work
//! - GPU-accelerated compositing via `wgpu`.
//! - Bilinear / Lanczos scaling (MVP uses nearest-neighbor).

use async_trait::async_trait;
use futures::future::select_all;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    Packet, PacketMetadata, PacketType, PixelFormat, VideoFormat, VideoFrame,
};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

// ── Configuration ───────────────────────────────────────────────────────────

const fn default_width() -> u32 {
    1280
}

const fn default_height() -> u32 {
    720
}

/// Pixel-space rectangle for positioning a layer on the output canvas.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Configuration for a static image overlay (decoded once at init).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct ImageOverlayConfig {
    /// Base64-encoded image data (PNG or JPEG). Decoded once during
    /// initialization, not per-frame.
    pub data_base64: String,
    /// Destination rectangle on the output canvas.
    pub rect: Rect,
    /// Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

/// Configuration for a text overlay (rasterized once per `UpdateParams`).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct TextOverlayConfig {
    /// The text string to render.
    pub text: String,
    /// Destination rectangle on the output canvas.
    pub rect: Rect,
    /// RGBA colour, e.g. `[255, 255, 255, 255]`.
    #[serde(default = "default_text_color")]
    pub color: [u8; 4],
    /// Font size in pixels.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

const fn default_opacity() -> f32 {
    1.0
}

const fn default_text_color() -> [u8; 4] {
    [255, 255, 255, 255]
}

const fn default_font_size() -> u32 {
    24
}

/// Layer configuration for a single compositing input.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct LayerConfig {
    /// Destination rectangle on the output canvas. If `None`, the input is
    /// scaled to fill the entire canvas.
    pub rect: Option<Rect>,
    /// Opacity (0.0 .. 1.0). Default 1.0.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self { rect: None, opacity: default_opacity() }
    }
}

/// Configuration for the video compositor node.
///
/// The compositor supports an arbitrary number of dynamic video inputs
/// (created at runtime via `PinManagementMessage`) plus static image/text
/// overlays configured here.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[serde(default)]
pub struct CompositorConfig {
    /// Output canvas width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output canvas height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Number of input pins to pre-create.
    /// Required for stateless/oneshot pipelines where pins must exist before
    /// graph building. Optional for dynamic pipelines where pins are created
    /// on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.
    pub num_inputs: Option<usize>,
    /// Output pixel format: "rgba8" (default) or "i420".
    /// Use "i420" when feeding a VP9/VP8 encoder downstream.
    #[serde(default = "default_output_pixel_format")]
    pub output_pixel_format: String,
    /// Per-layer configuration, keyed by pin name (e.g. `"in_0"`).
    /// Layers without an entry here are scaled to fill the canvas.
    #[serde(default)]
    pub layers: HashMap<String, LayerConfig>,
    /// Static image overlays (decoded once during init).
    #[serde(default)]
    pub image_overlays: Vec<ImageOverlayConfig>,
    /// Text overlays (rasterized once per `UpdateParams`).
    #[serde(default)]
    pub text_overlays: Vec<TextOverlayConfig>,
}

fn default_output_pixel_format() -> String {
    "rgba8".to_string()
}

fn parse_pixel_format(s: &str) -> Result<PixelFormat, StreamKitError> {
    match s.to_lowercase().as_str() {
        "rgba8" | "rgba" => Ok(PixelFormat::Rgba8),
        "i420" => Ok(PixelFormat::I420),
        other => Err(StreamKitError::Configuration(format!(
            "Unsupported output pixel format '{}'. Use 'rgba8' or 'i420'.",
            other
        ))),
    }
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            num_inputs: None,
            output_pixel_format: default_output_pixel_format(),
            layers: HashMap::new(),
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        }
    }
}

impl CompositorConfig {
    /// Validate compositor parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if width/height are zero or if opacity values
    /// are out of range.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("Canvas width and height must be > 0".to_string());
        }
        for (name, layer) in &self.layers {
            if !layer.opacity.is_finite() || layer.opacity < 0.0 || layer.opacity > 1.0 {
                return Err(format!("Layer '{name}' opacity must be in [0.0, 1.0]"));
            }
        }
        for (i, img) in self.image_overlays.iter().enumerate() {
            if !img.opacity.is_finite() || img.opacity < 0.0 || img.opacity > 1.0 {
                return Err(format!("Image overlay {i} opacity must be in [0.0, 1.0]"));
            }
        }
        for (i, txt) in self.text_overlays.iter().enumerate() {
            if !txt.opacity.is_finite() || txt.opacity < 0.0 || txt.opacity > 1.0 {
                return Err(format!("Text overlay {i} opacity must be in [0.0, 1.0]"));
            }
        }
        Ok(())
    }
}

// ── Decoded overlay bitmap ──────────────────────────────────────────────────

/// A pre-decoded RGBA bitmap overlay ready for per-frame blitting.
#[derive(Clone)]
struct DecodedOverlay {
    rgba_data: Vec<u8>,
    width: u32,
    height: u32,
    rect: Rect,
    opacity: f32,
}

/// Decode a base64-encoded image (PNG/JPEG) into an RGBA8 bitmap.
fn decode_image_overlay(config: &ImageOverlayConfig) -> Result<DecodedOverlay, StreamKitError> {
    use image::GenericImageView;

    use base64::Engine;
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(&config.data_base64).map_err(|e| {
            StreamKitError::Configuration(format!("Invalid base64 in image overlay: {e}"))
        })?;

    let img = image::load_from_memory(&bytes).map_err(|e| {
        StreamKitError::Configuration(format!("Failed to decode image overlay: {e}"))
    })?;

    let rgba = img.to_rgba8();
    let (w, h) = img.dimensions();

    Ok(DecodedOverlay {
        rgba_data: rgba.into_raw(),
        width: w,
        height: h,
        rect: config.rect.clone(),
        opacity: config.opacity,
    })
}

/// Rasterize a text overlay into an RGBA8 bitmap using `tiny-skia`.
fn rasterize_text_overlay(config: &TextOverlayConfig) -> DecodedOverlay {
    let w = config.rect.width.max(1);
    let h = config.rect.height.max(1);

    // Create a tiny-skia pixmap for the overlay region.
    // Safety: w and h are both >= 1 due to .max(1) above, so this should never be None.
    // The fallback handles any edge case from tiny-skia dimension limits.
    let mut pixmap = tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| {
        #[allow(clippy::expect_used)]
        tiny_skia::Pixmap::new(1, 1).expect("1x1 pixmap should always succeed")
    });

    // Use a simple built-in glyph renderer: each character is drawn as a
    // filled rectangle of `font_size` height. This is intentionally
    // simplistic for the MVP; a proper font rasterizer can replace this
    // once a font dependency is approved.
    let font_size = config.font_size.max(1);
    let glyph_w = font_size * 3 / 5; // approximate glyph width
    let glyph_h = font_size;

    let color = tiny_skia::Color::from_rgba8(
        config.color[0],
        config.color[1],
        config.color[2],
        config.color[3],
    );

    let mut paint = tiny_skia::Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    let transform = tiny_skia::Transform::identity();

    for (i, _ch) in config.text.chars().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let x = (i as u32) * glyph_w;
        if x + glyph_w > w {
            break; // clip to rect
        }
        // Draw a filled rectangle per glyph as a placeholder.
        #[allow(clippy::cast_precision_loss)]
        if let Some(rect) =
            tiny_skia::Rect::from_xywh(x as f32, 0.0, glyph_w as f32, glyph_h as f32)
        {
            pixmap.fill_rect(rect, &paint, transform, None);
        }
    }

    let rgba_data = pixmap.data().to_vec();

    DecodedOverlay {
        rgba_data,
        width: pixmap.width(),
        height: pixmap.height(),
        rect: config.rect.clone(),
        opacity: config.opacity,
    }
}

// ── Compositing helpers ─────────────────────────────────────────────────────

/// Scale and blit a source RGBA8 buffer onto a destination RGBA8 buffer at the
/// given destination rectangle. Uses nearest-neighbor sampling and clips to
/// canvas bounds.
///
/// Rows are processed in parallel via `rayon` when the blit region is large
/// enough to benefit from multi-core dispatch.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::too_many_arguments)]
fn scale_blit_rgba(
    dst: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_rect: &Rect,
    opacity: f32,
) {
    if src_width == 0 || src_height == 0 || dst_rect.width == 0 || dst_rect.height == 0 {
        return;
    }

    let dw = dst_width as usize;
    let dh = dst_height as usize;
    let sw = src_width as usize;
    let sh = src_height as usize;
    let rx = dst_rect.x as usize;
    let ry = dst_rect.y as usize;
    let rw = dst_rect.width as usize;
    let rh = dst_rect.height as usize;

    // Clamp the number of rows we actually process to the canvas height.
    let effective_rh = rh.min(dh.saturating_sub(ry));
    if effective_rh == 0 {
        return;
    }

    // Clamp the number of columns to the canvas width.
    let effective_rw = rw.min(dw.saturating_sub(rx));
    if effective_rw == 0 {
        return;
    }

    // Split the destination buffer into per-row slices so that each row can
    // be processed independently (and therefore in parallel).
    let row_stride = dw * 4;

    // Use rayon for parallel row processing when the region is large enough.
    use rayon::prelude::*;

    // We need to give each row its own mutable slice. Split the dst buffer
    // at the first output row.
    let first_row_byte = ry * row_stride;
    let dst_rows = &mut dst[first_row_byte..];

    dst_rows
        .par_chunks_mut(row_stride)
        .take(effective_rh)
        .enumerate()
        .for_each(|(dy, row_slice)| {
            let sy = dy * sh / rh;
            blit_row(
                row_slice, rx, effective_rw, src, sw, sh, sy, rw, opacity,
            );
        });
}

/// Blit a single row of the source onto a destination row slice.
///
/// This is the inner kernel extracted so that `scale_blit_rgba` can dispatch
/// rows in parallel.  The `row_slice` covers exactly one destination row
/// starting at pixel column 0 (i.e. byte offset `rx * 4` is the first column
/// we write to).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::inline_always
)]
#[inline(always)]
fn blit_row(
    row_slice: &mut [u8],
    rx: usize,
    effective_rw: usize,
    src: &[u8],
    sw: usize,
    sh: usize,
    sy: usize,
    rw: usize,
    opacity: f32,
) {
    // Fast path: when opacity is 1.0, we can skip the f32 multiply on alpha
    // and branch more cheaply.
    if opacity >= 1.0 {
        blit_row_opaque(row_slice, rx, effective_rw, src, sw, sh, sy, rw);
    } else {
        blit_row_alpha(row_slice, rx, effective_rw, src, sw, sh, sy, rw, opacity);
    }
}

/// Inner blit for fully-opaque layers (`opacity >= 1.0`).  Skips the
/// per-pixel f32 multiply on the source alpha channel.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::suboptimal_flops,
    clippy::inline_always
)]
#[inline(always)]
fn blit_row_opaque(
    row_slice: &mut [u8],
    rx: usize,
    effective_rw: usize,
    src: &[u8],
    sw: usize,
    _sh: usize,
    sy: usize,
    rw: usize,
) {
    for dx in 0..effective_rw {
        let sx = dx * sw / rw;
        let src_idx = (sy * sw + sx) * 4;
        if src_idx + 3 >= src.len() {
            continue;
        }

        let sr = src[src_idx];
        let sg = src[src_idx + 1];
        let sb = src[src_idx + 2];
        let sa = src[src_idx + 3];

        let out_x = rx + dx;
        let dst_idx = out_x * 4;
        if dst_idx + 3 >= row_slice.len() {
            continue;
        }

        if sa == 255 {
            row_slice[dst_idx] = sr;
            row_slice[dst_idx + 1] = sg;
            row_slice[dst_idx + 2] = sb;
            row_slice[dst_idx + 3] = 255;
        } else if sa > 0 {
            let alpha = f32::from(sa) / 255.0;
            let inv_alpha = 1.0 - alpha;
            row_slice[dst_idx] =
                f32::from(sr).mul_add(alpha, f32::from(row_slice[dst_idx]) * inv_alpha) as u8;
            row_slice[dst_idx + 1] =
                f32::from(sg).mul_add(alpha, f32::from(row_slice[dst_idx + 1]) * inv_alpha) as u8;
            row_slice[dst_idx + 2] =
                f32::from(sb).mul_add(alpha, f32::from(row_slice[dst_idx + 2]) * inv_alpha) as u8;
            row_slice[dst_idx + 3] =
                f32::from(row_slice[dst_idx + 3]).mul_add(inv_alpha, f32::from(sa)) as u8;
        }
    }
}

/// Inner blit with a sub-1.0 `opacity` multiplier.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::suboptimal_flops,
    clippy::inline_always
)]
#[inline(always)]
fn blit_row_alpha(
    row_slice: &mut [u8],
    rx: usize,
    effective_rw: usize,
    src: &[u8],
    sw: usize,
    _sh: usize,
    sy: usize,
    rw: usize,
    opacity: f32,
) {
    for dx in 0..effective_rw {
        let sx = dx * sw / rw;
        let src_idx = (sy * sw + sx) * 4;
        if src_idx + 3 >= src.len() {
            continue;
        }

        let sr = src[src_idx];
        let sg = src[src_idx + 1];
        let sb = src[src_idx + 2];
        let sa = src[src_idx + 3];

        let sa_eff = (f32::from(sa) * opacity).round() as u8;

        let out_x = rx + dx;
        let dst_idx = out_x * 4;
        if dst_idx + 3 >= row_slice.len() {
            continue;
        }

        if sa_eff == 255 {
            row_slice[dst_idx] = sr;
            row_slice[dst_idx + 1] = sg;
            row_slice[dst_idx + 2] = sb;
            row_slice[dst_idx + 3] = 255;
        } else if sa_eff > 0 {
            let alpha = f32::from(sa_eff) / 255.0;
            let inv_alpha = 1.0 - alpha;
            row_slice[dst_idx] =
                f32::from(sr).mul_add(alpha, f32::from(row_slice[dst_idx]) * inv_alpha) as u8;
            row_slice[dst_idx + 1] =
                f32::from(sg).mul_add(alpha, f32::from(row_slice[dst_idx + 1]) * inv_alpha) as u8;
            row_slice[dst_idx + 2] =
                f32::from(sb).mul_add(alpha, f32::from(row_slice[dst_idx + 2]) * inv_alpha) as u8;
            row_slice[dst_idx + 3] =
                f32::from(row_slice[dst_idx + 3]).mul_add(inv_alpha, f32::from(sa_eff)) as u8;
        }
    }
}

/// Blit a decoded overlay onto the destination canvas with alpha blending.
fn blit_overlay(dst: &mut [u8], dst_width: u32, dst_height: u32, overlay: &DecodedOverlay) {
    scale_blit_rgba(
        dst,
        dst_width,
        dst_height,
        &overlay.rgba_data,
        overlay.width,
        overlay.height,
        &overlay.rect,
        overlay.opacity,
    );
}

// ── Pixel format conversion helpers ─────────────────────────────────────────

/// Convert an I420 frame buffer to packed RGBA8.
///
/// Uses BT.601 studio-range YUV→RGB conversion.
/// Rows are processed in parallel via `rayon`.
fn i420_to_rgba8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;
    let u_offset = y_stride * h;
    let v_offset = u_offset + chroma_w * chroma_h;

    let mut rgba = vec![0u8; w * h * 4];
    let rgba_row_stride = w * 4;

    rgba.par_chunks_mut(rgba_row_stride)
        .take(h)
        .enumerate()
        .for_each(|(row, rgba_row)| {
            for col in 0..w {
                let y_val = data[row * y_stride + col] as i32;
                let u_val = data[u_offset + (row / 2) * chroma_w + col / 2] as i32;
                let v_val = data[v_offset + (row / 2) * chroma_w + col / 2] as i32;

                let c = y_val - 16;
                let d = u_val - 128;
                let e = v_val - 128;
                let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

                let off = col * 4;
                rgba_row[off] = r;
                rgba_row[off + 1] = g;
                rgba_row[off + 2] = b;
                rgba_row[off + 3] = 255;
            }
        });
    rgba
}

/// Convert a packed RGBA8 buffer to I420.
///
/// Uses BT.601 full-range RGB→YUV conversion.
fn rgba8_to_i420(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;
    let y_size = y_stride * h;
    let chroma_size = chroma_w * chroma_h;
    let mut out = vec![0u8; y_size + chroma_size * 2];

    // Split the output buffer into Y plane and chroma planes so we can
    // parallelise each independently.
    let (y_plane, chroma_planes) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = chroma_planes.split_at_mut(chroma_size);

    // Y plane — parallelise by row.
    y_plane
        .par_chunks_mut(y_stride)
        .take(h)
        .enumerate()
        .for_each(|(row, y_row)| {
            for col in 0..w {
                let off = (row * w + col) * 4;
                let r = data[off] as i32;
                let g = data[off + 1] as i32;
                let b = data[off + 2] as i32;
                let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                y_row[col] = y.clamp(0, 255) as u8;
            }
        });

    // U and V planes — parallelise by chroma row.
    let u_rows: Vec<&mut [u8]> = u_plane.chunks_mut(chroma_w).collect();
    let v_rows: Vec<&mut [u8]> = v_plane.chunks_mut(chroma_w).collect();

    u_rows
        .into_par_iter()
        .zip(v_rows)
        .enumerate()
        .for_each(|(crow, (u_row, v_row))| {
            for ccol in 0..chroma_w {
                let r0 = crow * 2;
                let c0 = ccol * 2;
                let mut sr = 0i32;
                let mut sg = 0i32;
                let mut sb = 0i32;
                let mut count = 0i32;
                for dr in 0..2 {
                    for dc in 0..2 {
                        let rr = r0 + dr;
                        let cc = c0 + dc;
                        if rr < h && cc < w {
                            let off = (rr * w + cc) * 4;
                            sr += data[off] as i32;
                            sg += data[off + 1] as i32;
                            sb += data[off + 2] as i32;
                            count += 1;
                        }
                    }
                }
                let r = sr / count;
                let g = sg / count;
                let b = sb / count;
                let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                u_row[ccol] = u.clamp(0, 255) as u8;
                v_row[ccol] = v.clamp(0, 255) as u8;
            }
        });

    out
}

// ── Input slot ──────────────────────────────────────────────────────────────

/// Holds a receiver and the most-recently-received frame for one input layer.
struct InputSlot {
    name: String,
    rx: mpsc::Receiver<Packet>,
    latest_frame: Option<VideoFrame>,
}

// ── Node ────────────────────────────────────────────────────────────────────

/// Composites multiple raw video inputs onto a single RGBA8 canvas with
/// optional image/text overlays.
///
/// Inputs are dynamic (`PinCardinality::Dynamic`) and can be attached at
/// runtime. Each input accepts `RawVideo(RGBA8)` with wildcard dimensions.
///
/// Output `"out"` produces `RawVideo` at the configured canvas size and
/// pixel format (RGBA8 by default, or I420 if `output_pixel_format` is set).
pub struct CompositorNode {
    config: CompositorConfig,
    /// Resolved output pixel format.
    output_format: PixelFormat,
    /// Current input pins (may grow dynamically).
    input_pins: Vec<InputPin>,
    /// Next input ID for dynamic pin naming.
    next_input_id: usize,
}

impl CompositorNode {
    #[must_use]
    pub fn new(config: CompositorConfig) -> Self {
        let (input_pins, next_input_id) = config.num_inputs.map_or_else(
            || {
                // Dynamic mode - start with no pins
                (Vec::new(), 0)
            },
            |num_inputs| {
                // Pre-create pins for stateless/oneshot pipelines.
                // Follow the YAML convention: single input uses "in",
                // multiple inputs use "in_0", "in_1", etc.
                let mut pins = Vec::with_capacity(num_inputs);
                if num_inputs == 1 {
                    pins.push(Self::make_input_pin("in".to_string()));
                } else {
                    for i in 0..num_inputs {
                        pins.push(Self::make_input_pin(format!("in_{i}")));
                    }
                }
                (pins, num_inputs)
            },
        );

        let output_format =
            parse_pixel_format(&config.output_pixel_format).unwrap_or(PixelFormat::Rgba8);

        Self { config, output_format, input_pins, next_input_id }
    }

    /// Returns the definition-time pins for registry (dynamic template).
    pub fn definition_pins() -> (Vec<InputPin>, Vec<OutputPin>) {
        let inputs = vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
                PacketType::RawVideo(VideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Rgba8,
                }),
                PacketType::RawVideo(VideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::I420,
                }),
            ],
            cardinality: PinCardinality::Dynamic { prefix: "in".to_string() },
        }];

        let outputs = vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(VideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Rgba8,
            }),
            cardinality: PinCardinality::Broadcast,
        }];

        (inputs, outputs)
    }

    /// Create a concrete `InputPin` for a given name.
    fn make_input_pin(name: String) -> InputPin {
        InputPin {
            name,
            accepts_types: vec![
                PacketType::RawVideo(VideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Rgba8,
                }),
                PacketType::RawVideo(VideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::I420,
                }),
            ],
            cardinality: PinCardinality::One,
        }
    }
}

#[async_trait]
impl ProcessorNode for CompositorNode {
    fn input_pins(&self) -> Vec<InputPin> {
        self.input_pins.clone()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(VideoFormat {
                width: Some(self.config.width),
                height: Some(self.config.height),
                pixel_format: self.output_format,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "CompositorNode starting: {}x{} canvas, {} image overlays, {} text overlays",
            self.config.width,
            self.config.height,
            self.config.image_overlays.len(),
            self.config.text_overlays.len(),
        );

        // Decode image overlays (once).
        let mut image_overlays: Vec<DecodedOverlay> =
            Vec::with_capacity(self.config.image_overlays.len());
        for (i, img_cfg) in self.config.image_overlays.iter().enumerate() {
            match decode_image_overlay(img_cfg) {
                Ok(overlay) => {
                    tracing::info!(
                        "Decoded image overlay {}: {}x{} -> rect ({},{} {}x{})",
                        i,
                        overlay.width,
                        overlay.height,
                        overlay.rect.x,
                        overlay.rect.y,
                        overlay.rect.width,
                        overlay.rect.height,
                    );
                    image_overlays.push(overlay);
                },
                Err(e) => {
                    tracing::warn!("Failed to decode image overlay {}: {}", i, e);
                },
            }
        }

        // Rasterize text overlays (once; re-done on UpdateParams).
        let mut text_overlays: Vec<DecodedOverlay> =
            Vec::with_capacity(self.config.text_overlays.len());
        for txt_cfg in &self.config.text_overlays {
            text_overlays.push(rasterize_text_overlay(txt_cfg));
        }

        // Collect initial input slots from pre-connected pins.
        let mut slots: Vec<InputSlot> = Vec::new();
        for pin_name in context.inputs.keys() {
            let pin = Self::make_input_pin(pin_name.clone());
            self.input_pins.push(pin);
            // Track next_input_id for dynamically named pins.
            if let Some(num_str) = pin_name.strip_prefix("in_") {
                if let Ok(n) = num_str.parse::<usize>() {
                    self.next_input_id = self.next_input_id.max(n + 1);
                }
            }
        }
        // Drain all pre-connected inputs into slots.
        let pre_inputs: Vec<(String, mpsc::Receiver<Packet>)> = context.inputs.drain().collect();
        for (name, rx) in pre_inputs {
            tracing::info!("CompositorNode: pre-connected input '{}'", name);
            slots.push(InputSlot { name, rx, latest_frame: None });
        }

        // Pin management channel (optional).
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Shared state for the compositing thread.
        let video_pool = context.video_pool.clone();

        // ── Persistent compositing thread ───────────────────────────────
        // Instead of spawning a new blocking task per frame, we keep a
        // single long-lived thread that processes compositing work items
        // sent via a channel.  This avoids per-frame thread-pool
        // scheduling overhead and keeps CPU caches warm.
        let (work_tx, mut work_rx) =
            tokio::sync::mpsc::channel::<CompositeWorkItem>(2);
        let (result_tx, mut result_rx) =
            tokio::sync::mpsc::channel::<CompositeResult>(2);

        let composite_thread = tokio::task::spawn_blocking(move || {
            while let Some(work) = work_rx.blocking_recv() {
                let rgba_buf = composite_frame(
                    work.canvas_w,
                    work.canvas_h,
                    &work.layers,
                    &work.image_overlays,
                    &work.text_overlays,
                    work.video_pool.as_deref(),
                );
                let result = if work.output_format == PixelFormat::I420 {
                    let i420 = rgba8_to_i420(rgba_buf.as_slice(), work.canvas_w, work.canvas_h);
                    CompositeResult {
                        output_format: work.output_format,
                        rgba_data: None,
                        i420_data: Some(i420),
                    }
                } else {
                    CompositeResult {
                        output_format: work.output_format,
                        rgba_data: Some(rgba_buf),
                        i420_data: None,
                    }
                };
                if result_tx.blocking_send(result).is_err() {
                    break;
                }
            }
        });

        let mut output_seq: u64 = 0;
        let mut stop_reason: &str = "shutdown";

        loop {
            // ── Take at most one frame from every slot (non-blocking) ───
            // We intentionally take only one frame per slot per iteration so
            // that every produced frame is composited and forwarded.  The old
            // "drain-to-latest" approach dropped intermediate frames when the
            // compositing step was slower than the producer.
            let mut got_any_frame = false;
            for slot in &mut slots {
                if let Ok(packet) = slot.rx.try_recv() {
                    if let Packet::Video(frame) = packet {
                        slot.latest_frame = Some(frame);
                        got_any_frame = true;
                    }
                }
            }

            // ── Wait for at least one frame if none are available yet ────
            if !got_any_frame && !slots.is_empty() {
                // Use select! to wait for any input, control, or pin management.
                let mut received_frame = false;
                let mut should_break = false;

                tokio::select! {
                    biased;

                    // Control messages (highest priority).
                    Some(ctrl_msg) = context.control_rx.recv() => {
                        match ctrl_msg {
                            NodeControlMessage::Shutdown => {
                                tracing::info!("CompositorNode received shutdown");
                                should_break = true;
                            },
                            NodeControlMessage::UpdateParams(params) => {
                                Self::apply_update_params(
                                    &mut self.config,
                                    &mut image_overlays,
                                    &mut text_overlays,
                                    params,
                                    &mut stats_tracker,
                                );
                            },
                            NodeControlMessage::Start => {},
                        }
                    }

                    // Pin management.
                    Some(msg) = async {
                        match &mut pin_mgmt_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        Self::handle_pin_management(
                            &mut self,
                            msg,
                            &mut slots,
                        );
                    }

                    // Wait for a frame from any connected input.
                    result = recv_from_any_slot(&mut slots) => {
                        if let Some((slot_idx, frame)) = result {
                            slots[slot_idx].latest_frame = Some(frame);
                            received_frame = true;
                        } else {
                            // All inputs closed.
                            stop_reason = "all_inputs_closed";
                            should_break = true;
                        }
                    }
                }

                if should_break {
                    break;
                }
                if !received_frame {
                    continue;
                }
            }

            if slots.is_empty() {
                // No inputs at all — wait for pin management or control.
                tokio::select! {
                    Some(ctrl_msg) = context.control_rx.recv() => {
                        match ctrl_msg {
                            NodeControlMessage::Shutdown => {
                                tracing::info!("CompositorNode received shutdown (no inputs)");
                                break;
                            },
                            NodeControlMessage::UpdateParams(params) => {
                                Self::apply_update_params(
                                    &mut self.config,
                                    &mut image_overlays,
                                    &mut text_overlays,
                                    params,
                                    &mut stats_tracker,
                                );
                            },
                            NodeControlMessage::Start => {},
                        }
                    }
                    Some(msg) = async {
                        match &mut pin_mgmt_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        Self::handle_pin_management(
                            &mut self,
                            msg,
                            &mut slots,
                        );
                    }
                }
                continue;
            }

            // ── Check for non-blocking control / pin management ──────────
            let mut should_stop = false;
            while let Ok(ctrl_msg) = context.control_rx.try_recv() {
                match ctrl_msg {
                    NodeControlMessage::Shutdown => {
                        tracing::info!("CompositorNode received shutdown during compositing");
                        stop_reason = "shutdown";
                        should_stop = true;
                        break;
                    },
                    NodeControlMessage::UpdateParams(params) => {
                        Self::apply_update_params(
                            &mut self.config,
                            &mut image_overlays,
                            &mut text_overlays,
                            params,
                            &mut stats_tracker,
                        );
                    },
                    NodeControlMessage::Start => {},
                }
            }
            if should_stop {
                break;
            }
            if let Some(ref mut pmrx) = pin_mgmt_rx {
                while let Ok(msg) = pmrx.try_recv() {
                    Self::handle_pin_management(&mut self, msg, &mut slots);
                }
            }

            // ── Send work to persistent compositing thread ─────────────
            // Collect the data we need to send to the blocking thread.
            let num_slots = slots.len();
            let layers: Vec<Option<LayerSnapshot>> = slots
                .iter()
                .enumerate()
                .map(|(idx, slot)| {
                    slot.latest_frame.as_ref().map(|f| {
                        let layer_cfg = self.config.layers.get(&slot.name);
                        let (rect, opacity) = if let Some(lc) = layer_cfg {
                            // Explicit per-layer config.
                            (lc.rect.clone(), lc.opacity)
                        } else if idx > 0 && num_slots > 1 {
                            // Auto-PiP: non-first layers without explicit config
                            // are placed in the bottom-right corner at 1/3 canvas
                            // size with slight transparency.
                            let pip_w = self.config.width / 3;
                            let pip_h = self.config.height / 3;
                            let pip_x = self.config.width - pip_w - 20;
                            let pip_y = self.config.height - pip_h - 20;
                            (Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }), 0.9)
                        } else {
                            // First layer (or single input): fill the canvas.
                            (None, 1.0)
                        };
                        LayerSnapshot {
                            data: f.data.clone(),
                            width: f.width,
                            height: f.height,
                            pixel_format: f.pixel_format,
                            rect,
                            opacity,
                        }
                    })
                })
                .collect();

            stats_tracker.received();

            let work_item = CompositeWorkItem {
                canvas_w: self.config.width,
                canvas_h: self.config.height,
                layers,
                image_overlays: image_overlays.clone(),
                text_overlays: text_overlays.clone(),
                video_pool: video_pool.clone(),
                output_format: self.output_format,
            };

            if work_tx.send(work_item).await.is_err() {
                tracing::debug!("Compositing thread gone, stopping CompositorNode");
                stop_reason = "compositor_thread_gone";
                break;
            }

            let composite_result = match result_rx.recv().await {
                Some(r) => r,
                None => {
                    tracing::debug!("Compositing result channel closed");
                    stop_reason = "compositor_thread_gone";
                    break;
                },
            };

            // Build metadata from the first available input frame.
            let src_metadata =
                slots.iter().find_map(|s| s.latest_frame.as_ref()).and_then(|f| f.metadata.clone());

            let metadata = Some(PacketMetadata {
                timestamp_us: src_metadata.as_ref().and_then(|m| m.timestamp_us),
                duration_us: src_metadata.as_ref().and_then(|m| m.duration_us),
                sequence: Some(output_seq),
                keyframe: Some(true),
            });

            let out_frame = if composite_result.output_format == PixelFormat::I420 {
                let i420_data = composite_result
                    .i420_data
                    .expect("I420 output data must be present");
                VideoFrame::with_metadata(
                    self.config.width,
                    self.config.height,
                    PixelFormat::I420,
                    i420_data,
                    metadata,
                )
            } else {
                let pooled = composite_result
                    .rgba_data
                    .expect("RGBA8 output data must be present");
                VideoFrame::from_pooled(
                    self.config.width,
                    self.config.height,
                    PixelFormat::Rgba8,
                    pooled,
                    metadata,
                )
            };

            if context.output_sender.send("out", Packet::Video(out_frame)).await.is_err() {
                tracing::debug!("Output channel closed, stopping CompositorNode");
                stop_reason = "output_closed";
                break;
            }

            stats_tracker.sent();
            stats_tracker.maybe_send();
            output_seq += 1;
        }

        // Drop the work sender to signal the compositing thread to exit.
        drop(work_tx);
        let _ = composite_thread.await;

        stats_tracker.force_send();
        state_helpers::emit_stopped(&context.state_tx, &node_name, stop_reason);
        Ok(())
    }
}

// ── Private helpers on CompositorNode ───────────────────────────────────────

impl CompositorNode {
    fn apply_update_params(
        config: &mut CompositorConfig,
        image_overlays: &mut Vec<DecodedOverlay>,
        text_overlays: &mut Vec<DecodedOverlay>,
        params: serde_json::Value,
        stats_tracker: &mut NodeStatsTracker,
    ) {
        match serde_json::from_value::<CompositorConfig>(params) {
            Ok(new_config) => match new_config.validate() {
                Ok(()) => {
                    tracing::info!(
                        old_w = config.width,
                        old_h = config.height,
                        new_w = new_config.width,
                        new_h = new_config.height,
                        "Updating compositor config"
                    );

                    // Always re-decode image overlays (content may have changed
                    // even if the count is the same).
                    image_overlays.clear();
                    for img_cfg in &new_config.image_overlays {
                        match decode_image_overlay(img_cfg) {
                            Ok(ov) => image_overlays.push(ov),
                            Err(e) => tracing::warn!("Image overlay decode failed: {e}"),
                        }
                    }

                    // Re-rasterize text overlays.
                    text_overlays.clear();
                    for txt_cfg in &new_config.text_overlays {
                        text_overlays.push(rasterize_text_overlay(txt_cfg));
                    }

                    *config = new_config;
                },
                Err(e) => {
                    tracing::warn!("Rejected invalid compositor config: {e}");
                    stats_tracker.errored();
                },
            },
            Err(e) => {
                tracing::warn!("Failed to deserialize compositor UpdateParams: {e}");
                stats_tracker.errored();
            },
        }
    }

    fn handle_pin_management(
        node: &mut Box<Self>,
        msg: PinManagementMessage,
        slots: &mut Vec<InputSlot>,
    ) {
        match msg {
            PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| {
                    let name = format!("in_{}", node.next_input_id);
                    node.next_input_id += 1;
                    name
                });
                let pin = Self::make_input_pin(pin_name);
                node.input_pins.push(pin.clone());
                let _ = response_tx.send(Ok(pin));
            },
            PinManagementMessage::AddedInputPin { pin, channel } => {
                tracing::info!("CompositorNode: activated input pin '{}'", pin.name);
                slots.push(InputSlot { name: pin.name, rx: channel, latest_frame: None });
            },
            PinManagementMessage::RemoveInputPin { pin_name } => {
                tracing::info!("CompositorNode: removed input pin '{}'", pin_name);
                slots.retain(|s| s.name != pin_name);
                node.input_pins.retain(|p| p.name != pin_name);
            },
            _ => {},
        }
    }
}

// ── Frame receive helper ────────────────────────────────────────────────────

/// Wait for a video frame from any of the input slots. Returns the slot index
/// and the received frame, or `None` if all channels are closed.
async fn recv_from_any_slot(slots: &mut [InputSlot]) -> Option<(usize, VideoFrame)> {
    if slots.is_empty() {
        return None;
    }

    // Use futures to poll all receivers concurrently.
    type SlotRecvFut<'a> =
        Pin<Box<dyn futures::Future<Output = (usize, Option<Packet>)> + Send + 'a>>;

    let futs: Vec<SlotRecvFut<'_>> = slots
        .iter_mut()
        .enumerate()
        .map(|(i, slot)| {
            let fut = async move {
                let pkt = slot.rx.recv().await;
                (i, pkt)
            };
            Box::pin(fut) as Pin<Box<dyn futures::Future<Output = _> + Send + '_>>
        })
        .collect();

    if futs.is_empty() {
        return None;
    }

    let (result, _idx, _remaining) = select_all(futs).await;
    let (slot_idx, maybe_packet) = result;

    maybe_packet.and_then(|pkt| match pkt {
        Packet::Video(frame) => Some((slot_idx, frame)),
        _ => None,
    })
}

// ── Compositing kernel (runs on a persistent blocking thread) ────────────────

/// Snapshot of one input layer's data for the blocking compositor thread.
struct LayerSnapshot {
    data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    rect: Option<Rect>,
    opacity: f32,
}

/// Work item sent from the async loop to the persistent compositing thread.
struct CompositeWorkItem {
    canvas_w: u32,
    canvas_h: u32,
    layers: Vec<Option<LayerSnapshot>>,
    image_overlays: Vec<DecodedOverlay>,
    text_overlays: Vec<DecodedOverlay>,
    video_pool: Option<Arc<streamkit_core::VideoFramePool>>,
    output_format: PixelFormat,
}

/// Result sent back from the compositing thread to the async loop.
struct CompositeResult {
    output_format: PixelFormat,
    rgba_data: Option<streamkit_core::frame_pool::PooledVideoData>,
    i420_data: Option<Vec<u8>>,
}

/// Composite all layers + overlays onto a fresh RGBA8 canvas buffer.
/// Allocates from the video pool if available.
fn composite_frame(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[DecodedOverlay],
    text_overlays: &[DecodedOverlay],
    video_pool: Option<&streamkit_core::VideoFramePool>,
) -> streamkit_core::frame_pool::PooledVideoData {
    let total_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;

    let mut pooled = video_pool.map_or_else(
        || streamkit_core::frame_pool::PooledVideoData::from_vec(vec![0u8; total_bytes]),
        |pool| pool.get(total_bytes),
    );

    // Zero the buffer (transparent black).
    let buf = pooled.as_mut_slice();
    buf[..total_bytes].fill(0);

    // Blit each layer (in order — first layer is bottom, last is top).
    // I420 layers are converted to RGBA8 on-the-fly inside this blocking task.
    for layer in layers.iter().flatten() {
        let dst_rect =
            layer.rect.clone().unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });

        let rgba_data;
        let src_data: &[u8] = match layer.pixel_format {
            PixelFormat::Rgba8 => layer.data.as_slice(),
            PixelFormat::I420 => {
                rgba_data = i420_to_rgba8(layer.data.as_slice(), layer.width, layer.height);
                &rgba_data
            },
        };

        scale_blit_rgba(
            buf,
            canvas_w,
            canvas_h,
            src_data,
            layer.width,
            layer.height,
            &dst_rect,
            layer.opacity,
        );
    }

    // Blit image overlays.
    for ov in image_overlays {
        blit_overlay(buf, canvas_w, canvas_h, ov);
    }

    // Blit text overlays.
    for ov in text_overlays {
        blit_overlay(buf, canvas_w, canvas_h, ov);
    }

    pooled
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used)]
pub fn register_compositor_nodes(registry: &mut NodeRegistry) {
    let (def_inputs, def_outputs) = CompositorNode::definition_pins();

    registry.register_static_with_description(
        "video::compositor",
        |params| {
            let config: CompositorConfig = config_helpers::parse_config_optional(params)?;
            if let Err(e) = config.validate() {
                return Err(StreamKitError::Configuration(e));
            }
            Ok(Box::new(CompositorNode::new(config)))
        },
        serde_json::to_value(schema_for!(CompositorConfig))
            .expect("CompositorConfig schema should serialize to JSON"),
        StaticPins { inputs: def_inputs, outputs: def_outputs },
        vec!["video".to_string(), "compositing".to_string()],
        false,
        "Composites multiple raw video inputs (RGBA8) onto a single canvas with \
         image and text overlays. Supports dynamic pin creation for attaching \
         arbitrary inputs at runtime.",
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    /// Create a solid-colour RGBA8 VideoFrame.
    fn make_rgba_frame(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> VideoFrame {
        let total = (width as usize) * (height as usize) * 4;
        let mut data = vec![0u8; total];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
            pixel[3] = a;
        }
        VideoFrame::new(width, height, PixelFormat::Rgba8, data)
    }

    // ── Unit tests for compositing helpers ───────────────────────────────

    #[test]
    fn test_scale_blit_identity() {
        // 2x2 red source blitted onto a 4x4 canvas at (1,1) 2x2 rect.
        let src = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 128, 128, 255];
        let mut dst = vec![0u8; 4 * 4 * 4]; // 4x4 RGBA, all transparent black

        scale_blit_rgba(&mut dst, 4, 4, &src, 2, 2, &Rect { x: 1, y: 1, width: 2, height: 2 }, 1.0);

        // Pixel at (1,1) should be red.
        let idx = (1 * 4 + 1) * 4;
        assert_eq!(dst[idx], 255);
        assert_eq!(dst[idx + 1], 0);
        assert_eq!(dst[idx + 2], 0);
        assert_eq!(dst[idx + 3], 255);

        // Pixel at (0,0) should remain transparent black.
        assert_eq!(dst[0], 0);
        assert_eq!(dst[3], 0);
    }

    #[test]
    fn test_scale_blit_with_opacity() {
        // White source at 50% opacity over black background.
        let src = vec![255, 255, 255, 255]; // 1x1 white
        let mut dst = vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]; // 2x2 black

        scale_blit_rgba(&mut dst, 2, 2, &src, 1, 1, &Rect { x: 0, y: 0, width: 1, height: 1 }, 0.5);

        // Pixel (0,0): white at 50% over opaque black -> ~128 grey.
        let r = dst[0];
        assert!(r > 120 && r < 135, "Expected ~128, got {r}");
    }

    #[test]
    fn test_scale_blit_scaling() {
        // 1x1 red source scaled to 4x4 rect on an 8x8 canvas.
        let src = vec![255, 0, 0, 255];
        let mut dst = vec![0u8; 8 * 8 * 4];

        scale_blit_rgba(&mut dst, 8, 8, &src, 1, 1, &Rect { x: 2, y: 2, width: 4, height: 4 }, 1.0);

        // All pixels in the 4x4 destination rect should be red.
        for y in 2..6u32 {
            for x in 2..6u32 {
                let idx = ((y * 8 + x) * 4) as usize;
                assert_eq!(dst[idx], 255, "Red at ({x},{y})");
                assert_eq!(dst[idx + 1], 0, "Green at ({x},{y})");
            }
        }
        // Outside should remain black.
        assert_eq!(dst[0], 0);
    }

    #[test]
    fn test_composite_frame_empty_layers() {
        // No layers, no overlays -> transparent black canvas.
        let result = composite_frame(4, 4, &[], &[], &[], None);
        let buf = result.as_slice();
        assert_eq!(buf.len(), 4 * 4 * 4);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_composite_frame_single_layer() {
        let data = make_rgba_frame(2, 2, 255, 0, 0, 255);
        let layer = LayerSnapshot {
            data: data.data.clone(),
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
            opacity: 1.0,
        };

        let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None);
        let buf = result.as_slice();

        // Entire canvas should be red (scaled from 2x2 to 4x4).
        for pixel in buf.chunks_exact(4) {
            assert_eq!(pixel[0], 255, "Red channel");
            assert_eq!(pixel[1], 0, "Green channel");
            assert_eq!(pixel[2], 0, "Blue channel");
            assert_eq!(pixel[3], 255, "Alpha channel");
        }
    }

    #[test]
    fn test_composite_frame_two_layers() {
        // Bottom: full-canvas red. Top: small green square at (1,1) 2x2.
        let red = make_rgba_frame(4, 4, 255, 0, 0, 255);
        let green = make_rgba_frame(2, 2, 0, 255, 0, 255);

        let layer0 = LayerSnapshot {
            data: red.data.clone(),
            width: 4,
            height: 4,
            pixel_format: PixelFormat::Rgba8,
            rect: None,
            opacity: 1.0,
        };
        let layer1 = LayerSnapshot {
            data: green.data.clone(),
            width: 2,
            height: 2,
            pixel_format: PixelFormat::Rgba8,
            rect: Some(Rect { x: 1, y: 1, width: 2, height: 2 }),
            opacity: 1.0,
        };

        let result = composite_frame(4, 4, &[Some(layer0), Some(layer1)], &[], &[], None);
        let buf = result.as_slice();

        // (0,0) should be red.
        assert_eq!(buf[0], 255);
        assert_eq!(buf[1], 0);

        // (1,1) should be green (overwritten by top layer).
        let idx = (1 * 4 + 1) * 4;
        assert_eq!(buf[idx], 0);
        assert_eq!(buf[idx + 1], 255);
        assert_eq!(buf[idx + 2], 0);
    }

    #[test]
    fn test_rasterize_text_overlay_produces_pixels() {
        let cfg = TextOverlayConfig {
            text: "Hi".to_string(),
            rect: Rect { x: 0, y: 0, width: 64, height: 32 },
            color: [255, 255, 0, 255],
            font_size: 24,
            opacity: 1.0,
        };
        let overlay = rasterize_text_overlay(&cfg);
        assert_eq!(overlay.width, 64);
        assert_eq!(overlay.height, 32);
        // Should have some non-zero pixels (text was drawn).
        assert!(overlay.rgba_data.iter().any(|&b| b > 0));
    }

    #[test]
    fn test_config_validate_ok() {
        let cfg = CompositorConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_zero_dimensions() {
        let cfg = CompositorConfig { width: 0, height: 720, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validate_bad_opacity() {
        let mut cfg = CompositorConfig::default();
        cfg.layers.insert("in_0".to_string(), LayerConfig { rect: None, opacity: 1.5 });
        assert!(cfg.validate().is_err());
    }

    // ── Integration test: node run() with mock context ──────────────────

    #[tokio::test]
    async fn test_compositor_node_run_main_only() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = CompositorConfig { width: 4, height: 4, ..Default::default() };
        let node = CompositorNode::new(config);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send a red frame.
        let frame = make_rgba_frame(2, 2, 255, 0, 0, 255);
        input_tx.send(Packet::Video(frame)).await.unwrap();

        // Give time for processing.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Close input.
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty(), "Expected at least 1 output frame");

        // Verify output is 4x4 RGBA.
        if let Packet::Video(ref out_frame) = output_packets[0] {
            assert_eq!(out_frame.width, 4);
            assert_eq!(out_frame.height, 4);
            assert_eq!(out_frame.pixel_format, PixelFormat::Rgba8);
            // Should be red (2x2 scaled to fill 4x4).
            assert_eq!(out_frame.data()[0], 255); // R
            assert_eq!(out_frame.data()[1], 0); // G
        } else {
            panic!("Expected video packet");
        }
    }

    #[tokio::test]
    async fn test_compositor_node_preserves_metadata() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in_0".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = CompositorConfig { width: 2, height: 2, ..Default::default() };
        let node = CompositorNode::new(config);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let mut frame = make_rgba_frame(2, 2, 100, 100, 100, 255);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(42_000),
            duration_us: Some(33_333),
            sequence: Some(7),
            keyframe: Some(true),
        });
        input_tx.send(Packet::Video(frame)).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty());

        if let Packet::Video(ref out_frame) = output_packets[0] {
            let meta = out_frame.metadata.as_ref().expect("metadata should be preserved");
            assert_eq!(meta.timestamp_us, Some(42_000));
            assert_eq!(meta.duration_us, Some(33_333));
            assert_eq!(meta.sequence, Some(0)); // output sequence starts at 0
        } else {
            panic!("Expected video packet");
        }
    }

    #[test]
    fn test_compositor_definition_pins() {
        let (inputs, outputs) = CompositorNode::definition_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert!(matches!(inputs[0].cardinality, PinCardinality::Dynamic { .. }));
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
    }

    #[test]
    fn test_compositor_pool_usage() {
        use streamkit_core::frame_pool::FramePool;

        let canvas_w = 4u32;
        let canvas_h = 4u32;
        let total = (canvas_w as usize) * (canvas_h as usize) * 4; // 64 bytes

        let pool = FramePool::<u8>::preallocated(&[total], 2);
        assert_eq!(pool.stats().buckets[0].available, 2);

        let result = composite_frame(canvas_w, canvas_h, &[], &[], &[], Some(&pool));
        assert_eq!(result.as_slice().len(), total);
        // One buffer was taken from the pool.
        assert_eq!(pool.stats().buckets[0].available, 1);

        // Drop returns to pool.
        drop(result);
        assert_eq!(pool.stats().buckets[0].available, 2);
    }
}
