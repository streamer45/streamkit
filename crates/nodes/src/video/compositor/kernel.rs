// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Compositing kernel — CPU-based frame compositing.
//!
//! This module defines the data types exchanged between the async node
//! loop (in `mod.rs`) and the persistent blocking compositing thread,
//! plus the core [`composite_frame`] function.
//!
//! ## Threading model
//! The async run-loop sends a [`CompositeWorkItem`] per tick to a
//! long-lived `spawn_blocking` thread.  The thread composites layers and
//! overlays onto an RGBA8 canvas and returns a [`CompositeResult`].
//! Keeping a single persistent thread avoids per-frame thread-pool
//! scheduling overhead and keeps CPU caches warm.
//!
//! ## Pixel-format conversion
//! Input layers may arrive as I420 or NV12.  [`ConversionCache`] caches
//! the per-slot YUV → RGBA8 conversion keyed by `Arc` pointer identity,
//! so unchanged frames skip the conversion entirely.

use std::sync::Arc;
use streamkit_core::types::PixelFormat;

use super::config::Rect;
use super::overlay::DecodedOverlay;
use crate::video::pixel_ops::{
    all_alpha_opaque, i420_to_rgba8_buf, nv12_to_rgba8_buf, scale_blit_rgba_rotated, BlitRect,
};

impl From<Rect> for BlitRect {
    fn from(r: Rect) -> Self {
        Self { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

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

    /// Evict a cached conversion entry for the given slot index.
    ///
    /// Called when a slot is removed (input disconnected or pin removed)
    /// so that the potentially large RGBA buffer is freed rather than
    /// persisting for the lifetime of the compositor.
    pub fn evict(&mut self, slot_idx: usize) {
        if slot_idx < self.entries.len() {
            self.entries[slot_idx] = None;
        }
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

/// Snapshot of one input layer's video data, sent from the async loop to
/// the blocking compositor thread as part of a [`CompositeWorkItem`].
pub struct LayerSnapshot {
    pub data: Arc<streamkit_core::frame_pool::PooledVideoData>,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub rect: Option<Rect>,
    pub opacity: f32,
    /// Visual stacking order.  Used by `composite_frame` to interleave
    /// video layers with overlays in a single z-sorted compositing pass.
    pub z_index: i32,
    /// Clockwise rotation in degrees around the destination rect centre.
    /// Default `0.0` means no rotation.
    pub rotation_degrees: f32,
    /// Mirror horizontally (flip left ↔ right).
    pub mirror_horizontal: bool,
    /// Mirror vertically (flip top ↔ bottom).
    pub mirror_vertical: bool,
}

/// Work item sent from the async loop to the persistent compositing thread.
///
/// Contains the canvas dimensions, per-slot layer snapshots (in draw
/// order), and shared overlay lists.  One item is sent per compositor
/// tick.
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
    /// Slot indices whose conversion cache entries should be evicted
    /// before compositing this frame (freed when inputs disconnect).
    pub evict_cache_slots: Vec<usize>,
}

/// Result sent back from the compositing thread to the async loop.
///
/// Contains the fully-composited RGBA8 canvas buffer, ready to be
/// wrapped in a [`VideoFrame`](streamkit_core::types::VideoFrame) and
/// forwarded downstream.
pub struct CompositeResult {
    pub rgba_data: streamkit_core::frame_pool::PooledVideoData,
}

/// A resolved, ready-to-composite item.  Unifies video layers and decoded
/// overlays into a single type for the z-sorted compositing loop.
struct CompositeItem<'a> {
    src_data: &'a [u8],
    src_width: u32,
    src_height: u32,
    dst_rect: BlitRect,
    opacity: f32,
    rotation_degrees: f32,
    /// When `true`, all source pixels have alpha == 255.  Allows the blit
    /// function to skip per-row alpha scanning and always use the memcpy path.
    src_opaque: bool,
    /// `(z_index, insertion_order)` for stable sorting.
    sort_key: (i32, usize),
    mirror_horizontal: bool,
    mirror_vertical: bool,
}

/// Composite all layers and overlays onto a fresh RGBA8 canvas buffer.
///
/// Layers and overlays are unified into a single z-sorted list and
/// blitted in order (lowest `z_index` first).  The canvas is allocated
/// from `video_pool` when available, falling back to a heap allocation.
///
/// `conversion_cache` persists across frames so that unchanged
/// I420/NV12 layers skip the YUV → RGBA8 conversion entirely.
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

    let mut items: Vec<CompositeItem<'_>> =
        Vec::with_capacity(layers.len() + image_overlays.len() + text_overlays.len());
    let mut insertion_order: usize = 0;

    // Video layers.
    for (layer, src_data) in resolved.iter().flatten() {
        let dst_rect: BlitRect = layer
            .rect
            .clone()
            .unwrap_or(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h })
            .into();
        // NV12/I420 → RGBA8 conversion always writes alpha = 255.
        let src_opaque = layer.pixel_format != PixelFormat::Rgba8;
        items.push(CompositeItem {
            src_data,
            src_width: layer.width,
            src_height: layer.height,
            dst_rect,
            opacity: layer.opacity,
            rotation_degrees: layer.rotation_degrees,
            src_opaque,
            sort_key: (layer.z_index, insertion_order),
            mirror_horizontal: layer.mirror_horizontal,
            mirror_vertical: layer.mirror_vertical,
        });
        insertion_order += 1;
    }

    // Image overlays.
    for ov in image_overlays {
        items.push(CompositeItem {
            src_data: &ov.rgba_data,
            src_width: ov.width,
            src_height: ov.height,
            dst_rect: ov.rect.clone().into(),
            opacity: ov.opacity,
            rotation_degrees: ov.rotation_degrees,
            src_opaque: false,
            sort_key: (ov.z_index, insertion_order),
            mirror_horizontal: ov.mirror_horizontal,
            mirror_vertical: ov.mirror_vertical,
        });
        insertion_order += 1;
    }

    // Text overlays.
    for ov in text_overlays {
        items.push(CompositeItem {
            src_data: &ov.rgba_data,
            src_width: ov.width,
            src_height: ov.height,
            dst_rect: ov.rect.clone().into(),
            opacity: ov.opacity,
            rotation_degrees: ov.rotation_degrees,
            src_opaque: false,
            sort_key: (ov.z_index, insertion_order),
            mirror_horizontal: ov.mirror_horizontal,
            mirror_vertical: ov.mirror_vertical,
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
            item.src_opaque,
            item.mirror_horizontal,
            item.mirror_vertical,
        );
    }

    pooled
}
