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
use super::pixel_ops::{
    blit_overlay, i420_to_rgba8_buf, rgba8_to_i420_buf, scale_blit_rgba_rotated,
};

// ── Compositing kernel (runs on a persistent blocking thread) ────────────────

/// Snapshot of one input layer's data for the blocking compositor thread.
pub struct LayerSnapshot {
    pub data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub rect: Option<Rect>,
    pub opacity: f32,
    /// Visual stacking order.  Lower values are drawn first (bottom).
    /// Used to sort layers before compositing; ties broken by slot index.
    pub z_index: i32,
    /// Clockwise rotation in degrees around the destination rect centre.
    /// Default `0.0` means no rotation.
    pub rotation_degrees: f32,
}

/// Work item sent from the async loop to the persistent compositing thread.
pub struct CompositeWorkItem {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub layers: Vec<Option<LayerSnapshot>>,
    /// Shared, immutable overlay lists.  Using `Arc<[…]>` means cloning
    /// into the work item each frame is a single ref-count bump instead
    /// of cloning the entire `Vec`.
    pub image_overlays: Arc<[Arc<DecodedOverlay>]>,
    pub text_overlays: Arc<[Arc<DecodedOverlay>]>,
    pub video_pool: Option<Arc<streamkit_core::VideoFramePool>>,
}

/// Result sent back from the compositing thread to the async loop.
pub struct CompositeResult {
    /// Composited frame data.  The pixel format matches the
    /// `output_pixel_format` passed to [`composite_frame`].
    pub data: streamkit_core::frame_pool::PooledVideoData,
    /// The pixel format of the returned data.
    pub pixel_format: PixelFormat,
}

/// Composite all layers + overlays onto a fresh RGBA8 canvas buffer.
/// Allocates from the video pool if available.
///
/// `i420_scratch` is a reusable buffer for I420→RGBA8 conversion, avoiding
/// per-frame allocation.
///
/// When `output_pixel_format` is [`PixelFormat::I420`], the finished RGBA8
/// canvas is converted to I420 on this thread (while the data is still
/// cache-warm from blitting), and the returned [`CompositeResult`] contains
/// I420 data.  This avoids a redundant RGBA8→I420 conversion in a
/// downstream VP9 encoder.
pub fn composite_frame(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    video_pool: Option<&streamkit_core::VideoFramePool>,
    i420_scratch: &mut Vec<u8>,
    output_pixel_format: PixelFormat,
) -> CompositeResult {
    let rgba_total_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;

    let mut pooled = video_pool.map_or_else(
        || streamkit_core::frame_pool::PooledVideoData::from_vec(vec![0u8; rgba_total_bytes]),
        |pool| pool.get(rgba_total_bytes),
    );

    // Zero the buffer (transparent black).
    let buf = pooled.as_mut_slice();
    buf[..rgba_total_bytes].fill(0);

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

        scale_blit_rgba_rotated(
            buf,
            canvas_w,
            canvas_h,
            src_data,
            layer.width,
            layer.height,
            &dst_rect,
            layer.opacity,
            layer.rotation_degrees,
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

    // Convert to I420 if requested.  The RGBA8 canvas data is still in
    // L1/L2 cache from the blitting above, so this conversion is
    // significantly cheaper than doing it on a separate thread later.
    if output_pixel_format == PixelFormat::I420 {
        let w = canvas_w as usize;
        let h = canvas_h as usize;
        let chroma_w = w.div_ceil(2);
        let chroma_h = h.div_ceil(2);
        let i420_size = w * h + 2 * chroma_w * chroma_h;

        let mut i420_pooled = video_pool.map_or_else(
            || streamkit_core::frame_pool::PooledVideoData::from_vec(vec![0u8; i420_size]),
            |pool| pool.get(i420_size),
        );

        rgba8_to_i420_buf(pooled.as_slice(), canvas_w, canvas_h, i420_pooled.as_mut_slice());

        return CompositeResult { data: i420_pooled, pixel_format: PixelFormat::I420 };
    }

    CompositeResult { data: pooled, pixel_format: PixelFormat::Rgba8 }
}
