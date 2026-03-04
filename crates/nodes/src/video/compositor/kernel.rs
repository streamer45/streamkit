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
    all_alpha_opaque, i420_to_rgba8_buf, nv12_to_rgba8_buf, scale_blit_rgba_rotated,
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
///
/// Also caches the first-layer alpha-scan result so that the canvas-clear
/// skip check doesn't re-scan every frame when the source hasn't changed.
pub struct ConversionCache {
    entries: Vec<Option<CachedConversion>>,
    /// Cached result of the alpha-opaqueness scan for the first visible layer.
    /// `(data_identity, all_opaque)` — valid when the `Arc` pointer matches.
    first_layer_alpha_cache: Option<(usize, bool)>,
}

impl Default for ConversionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversionCache {
    pub const fn new() -> Self {
        Self { entries: Vec::new(), first_layer_alpha_cache: None }
    }

    /// Check whether the first visible layer's source data is fully opaque.
    ///
    /// For I420/NV12 layers, the converted RGBA always has alpha == 255, so
    /// we return `true` immediately without scanning.  For RGBA layers we
    /// scan once and cache the result keyed by `Arc::as_ptr`.
    fn first_layer_all_opaque(&mut self, layer: &LayerSnapshot, rgba_data: &[u8]) -> bool {
        // I420/NV12 → RGBA conversion always writes alpha = 255.
        if layer.pixel_format != PixelFormat::Rgba8 {
            return true;
        }

        let identity = Arc::as_ptr(&layer.data) as usize;
        if let Some((cached_id, cached_result)) = self.first_layer_alpha_cache {
            if cached_id == identity {
                return cached_result;
            }
        }

        let all_opaque = all_alpha_opaque(rgba_data);
        self.first_layer_alpha_cache = Some((identity, all_opaque));
        all_opaque
    }

    /// Return a previously-cached RGBA slice for `slot_idx`.
    ///
    /// # Panics
    ///
    /// Panics if the slot has not been populated by a prior `get_or_convert`
    /// call for the same `layer`.  This is only called in the second pass of
    /// `composite_frame` after the first pass has ensured every non-RGBA
    /// layer has been converted.
    fn get_cached(&self, slot_idx: usize, layer: &LayerSnapshot) -> &[u8] {
        #[allow(clippy::expect_used)]
        let cached =
            self.entries[slot_idx].as_ref().expect("get_cached called before get_or_convert");
        let needed = layer.width as usize * layer.height as usize * 4;
        &cached.rgba[..needed]
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
        let needs_convert = self.entries[slot_idx].as_ref().is_none_or(|cached| {
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

/// A resolved, ready-to-blit item.  Unifies video layers and decoded
/// overlays into a single type for the z-sorted compositing loop.
struct BlitItem<'a> {
    src_data: &'a [u8],
    src_width: u32,
    src_height: u32,
    dst_rect: Rect,
    opacity: f32,
    rotation_degrees: f32,
    /// `(z_index, insertion_order)` for stable sorting.
    sort_key: (i32, usize),
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

    // Two-pass source resolution.
    //
    // Pass 1: populate the conversion cache for every non-RGBA layer.
    // `slot_idx` uses the position in the `layers` slice (which preserves
    // `None` holes) so that cache indices stay stable even when some slots
    // have no frame.
    for (slot_idx, entry) in layers.iter().enumerate() {
        if let Some(layer) = entry {
            if layer.pixel_format != PixelFormat::Rgba8 {
                conversion_cache.get_or_convert(slot_idx, layer);
            }
        }
    }

    // Between pass 1 and pass 2: check whether the first layer allows
    // skipping the canvas clear.  We do the alpha-opaqueness check here
    // while `conversion_cache` is still mutably available.  The result
    // is a simple bool so no borrows leak into pass 2.
    let skip_clear =
        layers.iter().enumerate().find_map(|(i, e)| e.as_ref().map(|l| (i, l))).is_some_and(
            |(_slot_idx, layer)| {
                // Quick checks that don't need the pixel data.
                if layer.opacity < 1.0 || layer.rotation_degrees.abs() >= 0.01 {
                    return false;
                }
                let covers = layer.rect.as_ref().is_none_or(|r| {
                    r.x <= 0
                        && r.y <= 0
                        && i64::from(r.width) + i64::from(r.x) >= i64::from(canvas_w)
                        && i64::from(r.height) + i64::from(r.y) >= i64::from(canvas_h)
                });
                if !covers {
                    return false;
                }
                // Alpha check — needs mutable access to conversion_cache.
                match layer.pixel_format {
                    // I420/NV12 → RGBA conversion always writes alpha = 255.
                    PixelFormat::I420 | PixelFormat::Nv12 => true,
                    PixelFormat::Rgba8 => {
                        conversion_cache.first_layer_all_opaque(layer, layer.data.as_slice())
                    },
                }
            },
        );
    if !skip_clear {
        buf[..total_bytes].fill(0);
    }

    // Pass 2: build resolved references.  The mutable borrow of
    // `conversion_cache` from pass 1 is released, so we can now take
    // shared references into the cache alongside references into `layers`.
    let resolved: Vec<Option<(&LayerSnapshot, &[u8])>> = layers
        .iter()
        .enumerate()
        .map(|(slot_idx, entry)| {
            entry.as_ref().map(|layer| {
                let src_data: &[u8] = match layer.pixel_format {
                    PixelFormat::Rgba8 => layer.data.as_slice(),
                    PixelFormat::I420 | PixelFormat::Nv12 => {
                        // Cache was populated in pass 1; this is a shared
                        // read that cannot fail.
                        conversion_cache.get_cached(slot_idx, layer)
                    },
                };
                (layer, src_data)
            })
        })
        .collect();

    // ── Unified z-sorted blit ─────────────────────────────────────────────
    //
    // Collect all blittable items (video layers + image/text overlays) into
    // a single list, sort by (z_index, insertion_order), then blit in order.
    // This replaces the former three separate loops and allows overlays to
    // be interleaved with video layers via z_index.

    let mut items: Vec<BlitItem<'_>> = Vec::new();
    let mut insertion_order: usize = 0;

    // Video layers.
    for (layer, src_data) in resolved.iter().flatten() {
        let dst_rect =
            layer.rect.clone().unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h });
        items.push(BlitItem {
            src_data,
            src_width: layer.width,
            src_height: layer.height,
            dst_rect,
            opacity: layer.opacity,
            rotation_degrees: layer.rotation_degrees,
            sort_key: (layer.z_index, insertion_order),
        });
        insertion_order += 1;
    }

    // Image overlays.
    for ov in image_overlays {
        items.push(BlitItem {
            src_data: &ov.rgba_data,
            src_width: ov.width,
            src_height: ov.height,
            dst_rect: ov.rect.clone(),
            opacity: ov.opacity,
            rotation_degrees: ov.rotation_degrees,
            sort_key: (ov.z_index, insertion_order),
        });
        insertion_order += 1;
    }

    // Text overlays.
    for ov in text_overlays {
        items.push(BlitItem {
            src_data: &ov.rgba_data,
            src_width: ov.width,
            src_height: ov.height,
            dst_rect: ov.rect.clone(),
            opacity: ov.opacity,
            rotation_degrees: ov.rotation_degrees,
            sort_key: (ov.z_index, insertion_order),
        });
        insertion_order += 1;
    }

    // Stable sort: lower z_index drawn first (bottom), ties broken by
    // insertion order (video layers first, then image, then text).
    items.sort_by_key(|item| item.sort_key);

    for item in &items {
        scale_blit_rgba_rotated(
            buf,
            canvas_w,
            canvas_h,
            item.src_data,
            item.src_width,
            item.src_height,
            &item.dst_rect,
            item.opacity,
            item.rotation_degrees,
        );
    }

    pooled
}
