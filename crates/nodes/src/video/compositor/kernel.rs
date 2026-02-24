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
use super::pixel_ops::{blit_overlay, i420_to_rgba8, scale_blit_rgba};

// ── Compositing kernel (runs on a persistent blocking thread) ────────────────

/// Snapshot of one input layer's data for the blocking compositor thread.
pub(crate) struct LayerSnapshot {
    pub data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub rect: Option<Rect>,
    pub opacity: f32,
}

/// Work item sent from the async loop to the persistent compositing thread.
pub(crate) struct CompositeWorkItem {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub layers: Vec<Option<LayerSnapshot>>,
    pub image_overlays: Vec<DecodedOverlay>,
    pub text_overlays: Vec<DecodedOverlay>,
    pub video_pool: Option<Arc<streamkit_core::VideoFramePool>>,
    pub output_format: PixelFormat,
}

/// Result sent back from the compositing thread to the async loop.
pub(crate) struct CompositeResult {
    pub output_format: PixelFormat,
    pub rgba_data: Option<streamkit_core::frame_pool::PooledVideoData>,
    pub i420_data: Option<Vec<u8>>,
}

/// Composite all layers + overlays onto a fresh RGBA8 canvas buffer.
/// Allocates from the video pool if available.
pub(crate) fn composite_frame(
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
