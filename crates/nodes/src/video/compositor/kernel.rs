// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Compositing kernel — runs on a persistent blocking thread.
//!
//! Contains the data types exchanged between the async node loop and the
//! blocking compositing thread, plus the core `composite_frame` function
//! that blits layers and overlays onto an RGBA8 canvas.

use std::sync::Arc;
use streamkit_core::types::PixelFormat;

use super::config::Rect;
use super::overlay::DecodedOverlay;
use super::pixel_ops::{blit_overlay, i420_to_rgba8_buf, scale_blit_rgba};

// ── Compositing kernel (runs on a persistent blocking thread) ────────────────

/// Snapshot of one input layer's data for the blocking compositor thread.
pub struct LayerSnapshot {
    pub data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub rect: Option<Rect>,
    pub opacity: f32,
}

/// Work item sent from the async loop to the persistent compositing thread.
pub struct CompositeWorkItem {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub layers: Vec<Option<LayerSnapshot>>,
    pub image_overlays: Vec<Arc<DecodedOverlay>>,
    pub text_overlays: Vec<Arc<DecodedOverlay>>,
    pub video_pool: Option<Arc<streamkit_core::VideoFramePool>>,
    pub output_format: PixelFormat,
}

/// Result sent back from the compositing thread to the async loop.
pub struct CompositeResult {
    pub output_format: PixelFormat,
    pub rgba_data: Option<streamkit_core::frame_pool::PooledVideoData>,
    pub i420_data: Option<streamkit_core::frame_pool::PooledVideoData>,
}

/// Composite all layers + overlays onto a fresh RGBA8 canvas buffer.
/// Allocates from the video pool if available.
///
/// `i420_scratch` is a reusable buffer for I420→RGBA8 conversion, avoiding
/// per-frame allocation.
pub fn composite_frame(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    video_pool: Option<&streamkit_core::VideoFramePool>,
    i420_scratch: &mut Vec<u8>,
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
    // I420 layers are converted to RGBA8 on-the-fly using the scratch buffer.
    for layer in layers.iter().flatten() {
        let dst_rect =
            layer.rect.clone().unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });

        let src_data: &[u8] = match layer.pixel_format {
            PixelFormat::Rgba8 => layer.data.as_slice(),
            PixelFormat::I420 => {
                let needed = layer.width as usize * layer.height as usize * 4;
                if i420_scratch.len() < needed {
                    i420_scratch.resize(needed, 0);
                }
                i420_to_rgba8_buf(layer.data.as_slice(), layer.width, layer.height, i420_scratch);
                &i420_scratch[..needed]
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

/// Check whether the I420→RGBA8→I420 round-trip can be skipped entirely.
///
/// This is possible when:
/// - The output format is I420
/// - There is exactly one active layer that is already I420
/// - That layer fills the full canvas (no rect / full-canvas rect)
/// - Opacity is 1.0
/// - There are no image or text overlays
///
/// Returns the index of the pass-through layer, or `None` if compositing is needed.
pub fn try_i420_passthrough(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    output_format: PixelFormat,
) -> Option<usize> {
    if output_format != PixelFormat::I420 {
        return None;
    }
    if !image_overlays.is_empty() || !text_overlays.is_empty() {
        return None;
    }

    // Find the single active layer.
    let mut active_idx = None;
    for (i, slot) in layers.iter().enumerate() {
        if slot.is_some() {
            if active_idx.is_some() {
                return None; // more than one active layer
            }
            active_idx = Some(i);
        }
    }
    let idx = active_idx?;
    let layer = layers[idx].as_ref().unwrap();

    if layer.pixel_format != PixelFormat::I420 {
        return None;
    }
    if layer.opacity < 1.0 {
        return None;
    }
    // Check dimensions match canvas.
    if layer.width != canvas_w || layer.height != canvas_h {
        return None;
    }
    // Check the rect fills the full canvas (or is None).
    if let Some(ref rect) = layer.rect {
        if rect.x != 0 || rect.y != 0 || rect.width != canvas_w || rect.height != canvas_h {
            return None;
        }
    }

    Some(idx)
}
