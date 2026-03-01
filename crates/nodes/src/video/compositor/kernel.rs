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
    blit_overlay, i420_to_rgba8_buf, nv12_to_rgba8_buf, scale_blit_rgba_rotated,
};

// ── Compositing kernel (runs on a persistent blocking thread) ────────────────

// ── YUV → RGBA conversion cache ─────────────────────────────────────────────

/// Cached RGBA conversion result for a single layer slot.
struct CachedConversion {
    /// Identity of the source data (`Arc::as_ptr` cast to `usize`).
    /// When the `Arc<PooledVideoData>` pointer hasn't changed between frames
    /// the underlying data is identical and the conversion can be skipped.
    data_identity: usize,
    width: u32,
    height: u32,
    /// Pre-converted RGBA8 data, stored as a plain `Vec<u8>`.
    rgba: Vec<u8>,
}

/// Per-slot cache for YUV → RGBA conversions.
///
/// Avoids redundant per-frame I420/NV12 → RGBA8 conversion when the source
/// `Arc<PooledVideoData>` hasn't changed since the previous frame.
pub struct ConversionCache {
    entries: Vec<Option<CachedConversion>>,
}

impl ConversionCache {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Look up or perform a YUV→RGBA conversion for layer at `slot_idx`.
    /// Returns a slice of RGBA8 data.
    fn get_or_convert(&mut self, slot_idx: usize, layer: &LayerSnapshot) -> &[u8] {
        let identity = Arc::as_ptr(&layer.data) as usize;

        // Ensure the cache Vec is large enough.
        if self.entries.len() <= slot_idx {
            self.entries.resize_with(slot_idx + 1, || None);
        }

        // Check if the cached entry is still valid.
        let needs_convert = self.entries[slot_idx].as_ref().map_or(true, |cached| {
            cached.data_identity != identity
                || cached.width != layer.width
                || cached.height != layer.height
        });

        if needs_convert {
            let needed = layer.width as usize * layer.height as usize * 4;
            // Reuse the existing allocation if possible.
            let mut rgba = self.entries[slot_idx].take().map(|c| c.rgba).unwrap_or_default();
            if rgba.len() < needed {
                rgba.resize(needed, 0);
            }

            match layer.pixel_format {
                PixelFormat::I420 => {
                    i420_to_rgba8_buf(layer.data.as_slice(), layer.width, layer.height, &mut rgba);
                },
                PixelFormat::Nv12 => {
                    nv12_to_rgba8_buf(layer.data.as_slice(), layer.width, layer.height, &mut rgba);
                },
                PixelFormat::Rgba8 => {
                    // Should not be called for RGBA, but handle gracefully.
                    rgba[..needed].copy_from_slice(&layer.data.as_slice()[..needed]);
                },
            }

            self.entries[slot_idx] = Some(CachedConversion {
                data_identity: identity,
                width: layer.width,
                height: layer.height,
                rgba,
            });
        }

        // SAFETY: we just inserted into this slot above when `needs_convert` was true,
        // and the slot was already `Some` when `needs_convert` was false.
        #[allow(clippy::expect_used)]
        let cached = self.entries[slot_idx].as_ref().expect("just inserted");
        let needed = layer.width as usize * layer.height as usize * 4;
        &cached.rgba[..needed]
    }
}

/// Returns `true` if the first visible layer is fully opaque, unrotated, and
/// covers the entire canvas — meaning the canvas clear can be skipped.
fn first_layer_covers_canvas(
    layers: &[Option<LayerSnapshot>],
    canvas_w: u32,
    canvas_h: u32,
) -> bool {
    let Some(first) = layers.iter().flatten().next() else {
        return false;
    };

    if first.opacity < 1.0 || first.rotation_degrees.abs() >= 0.01 {
        return false;
    }

    // Check if the layer fully covers the canvas.
    // A layer with no rect fills the entire canvas by default.
    first.rect.as_ref().map_or(true, |r| {
        r.x <= 0
            && r.y <= 0
            && i64::from(r.width) + i64::from(r.x) >= i64::from(canvas_w)
            && i64::from(r.height) + i64::from(r.y) >= i64::from(canvas_h)
    })
}

/// Snapshot of one input layer's data for the blocking compositor thread.
pub struct LayerSnapshot {
    pub data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub rect: Option<Rect>,
    pub opacity: f32,
    /// Visual stacking order.  Retained in the snapshot for diagnostic /
    /// logging purposes even though sorting now happens before snapshot
    /// construction.
    #[allow(dead_code)]
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
    pub rgba_data: streamkit_core::frame_pool::PooledVideoData,
}

/// Composite all layers + overlays onto a fresh RGBA8 canvas buffer.
/// Allocates from the video pool if available.
///
/// `conversion_cache` caches YUV→RGBA8 conversions across frames so that
/// unchanged layers skip the conversion entirely.
pub fn composite_frame(
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    video_pool: Option<&streamkit_core::VideoFramePool>,
    conversion_cache: &mut ConversionCache,
) -> streamkit_core::frame_pool::PooledVideoData {
    let total_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;

    let mut pooled = video_pool.map_or_else(
        || streamkit_core::frame_pool::PooledVideoData::from_vec(vec![0u8; total_bytes]),
        |pool| pool.get(total_bytes),
    );

    let buf = pooled.as_mut_slice();

    // Skip the canvas clear when the first layer is opaque, unrotated, and
    // covers the entire canvas — the blit will fully overwrite every pixel.
    if !first_layer_covers_canvas(layers, canvas_w, canvas_h) {
        buf[..total_bytes].fill(0);
    }

    // Blit each layer (in order — first layer is bottom, last is top).
    // Non-RGBA layers use the conversion cache to avoid redundant per-frame
    // YUV→RGBA8 conversion when the source data hasn't changed.
    for (slot_idx, layer) in layers.iter().flatten().enumerate() {
        let dst_rect =
            layer.rect.clone().unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });

        let src_data: &[u8] = match layer.pixel_format {
            PixelFormat::Rgba8 => layer.data.as_slice(),
            PixelFormat::I420 | PixelFormat::Nv12 => {
                conversion_cache.get_or_convert(slot_idx, layer)
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

    pooled
}
