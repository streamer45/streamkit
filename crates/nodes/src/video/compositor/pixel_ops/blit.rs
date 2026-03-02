// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! RGBA8 blitting operations for the video compositor.
//!
//! Contains:
//! - [`scale_blit_rgba`]: axis-aligned scale + blit with nearest-neighbor sampling.
//! - [`scale_blit_rgba_rotated`]: rotated scale + blit with anti-aliased edges.
//!
//! Both functions use row-level parallelism via `rayon` when the blit region
//! is large enough to amortise the thread-pool dispatch overhead.

use super::{blend_u8, rayon_chunk_rows, RAYON_ROW_THRESHOLD};
use crate::video::compositor::config::Rect;

#[cfg(target_arch = "x86_64")]
use super::simd::{
    all_alpha_opaque_avx2, all_alpha_opaque_sse2, blend_4px_alpha_sse2, blend_4px_opaque_sse2,
    blend_8px_alpha_avx2, blend_8px_opaque_avx2, read_rgba_u32,
};

// ── Scalar blend helper ─────────────────────────────────────────────────────

/// Blend a single source pixel onto a destination row slice at `dst_off`.
///
/// Handles fully-opaque, semi-transparent, and fully-transparent cases.
/// `opacity_u16` is a 0..255 multiplier applied to the source alpha, or 256
/// as a sentinel meaning "fully opaque, skip per-pixel opacity multiply".
///
/// This is the shared scalar path used by both the x86_64 remainder loop and
/// the non-x86_64 fallback in [`scale_blit_rgba_rotated`].
#[allow(clippy::inline_always)]
#[inline(always)]
fn blend_pixel_scalar(
    row_slice: &mut [u8],
    dst_off: usize,
    src: &[u8],
    src_idx: usize,
    opacity_u16: u16,
) {
    let ir = src[src_idx];
    let ig = src[src_idx + 1];
    let ib = src[src_idx + 2];
    let mut ia = src[src_idx + 3];

    if opacity_u16 < 256 {
        ia = ((u16::from(ia) * opacity_u16 + 128) >> 8).min(255) as u8;
    }
    if ia > 0 && dst_off + 3 < row_slice.len() {
        if ia == 255 {
            row_slice[dst_off] = ir;
            row_slice[dst_off + 1] = ig;
            row_slice[dst_off + 2] = ib;
            row_slice[dst_off + 3] = 255;
        } else {
            let a16 = u16::from(ia);
            row_slice[dst_off] = blend_u8(ir, row_slice[dst_off], a16);
            row_slice[dst_off + 1] = blend_u8(ig, row_slice[dst_off + 1], a16);
            row_slice[dst_off + 2] = blend_u8(ib, row_slice[dst_off + 2], a16);
            let da = u16::from(row_slice[dst_off + 3]);
            row_slice[dst_off + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
        }
    }
}

// ── Axis-aligned blit ───────────────────────────────────────────────────────

/// Scale and blit a source RGBA8 buffer onto a destination RGBA8 buffer at the
/// given destination rectangle. Uses nearest-neighbor sampling and clips to
/// canvas bounds.
///
/// Rows are processed in parallel via `rayon` when the blit region is large
/// enough to benefit from multi-core dispatch.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::too_many_arguments)]
pub fn scale_blit_rgba(
    dst: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_rect: &Rect,
    opacity: f32,
) {
    use rayon::prelude::*;

    if src_width == 0 || src_height == 0 || dst_rect.width == 0 || dst_rect.height == 0 {
        return;
    }

    let dw = dst_width as usize;
    let dh = dst_height as usize;
    let sw = src_width as usize;
    let sh = src_height as usize;
    let rw = dst_rect.width as usize;
    let rh = dst_rect.height as usize;

    // Compute the visible region after clipping the (possibly negative)
    // rect position to the canvas bounds.
    let (rx, src_col_skip) = if dst_rect.x < 0 {
        (0usize, (-dst_rect.x) as usize)
    } else {
        (dst_rect.x as usize, 0usize)
    };
    let (ry, src_row_skip) = if dst_rect.y < 0 {
        (0usize, (-dst_rect.y) as usize)
    } else {
        (dst_rect.y as usize, 0usize)
    };

    // Clamp the number of rows we actually process to the canvas height.
    let effective_rh = rh.saturating_sub(src_row_skip).min(dh.saturating_sub(ry));
    if effective_rh == 0 {
        return;
    }

    // Clamp the number of columns to the canvas width.
    let effective_rect_w = rw.saturating_sub(src_col_skip).min(dw.saturating_sub(rx));
    if effective_rect_w == 0 {
        return;
    }

    // Split the destination buffer into per-row slices so that each row can
    // be processed independently (and therefore in parallel).
    let row_stride = dw * 4;

    // We need to give each row its own mutable slice. Split the dst buffer
    // at the first output row.
    let first_row_byte = ry * row_stride;
    let dst_rows = &mut dst[first_row_byte..];

    // ── Identity-scale fast path ───────────────────────────────────────
    // When source dimensions exactly match the destination rect and opacity
    // is fully opaque, we can avoid per-pixel scaling entirely and use
    // direct row copies (memcpy) for fully-opaque source rows.
    //
    // Rows are processed in parallel via `rayon` when the blit region is
    // large enough to benefit from multi-core dispatch.
    if rw == sw && rh == sh && opacity >= 1.0 && src_col_skip == 0 && src_row_skip == 0 {
        let src_row_bytes = sw * 4;
        let copy_bytes = effective_rect_w * 4;
        // Pre-validate that the source buffer can satisfy all rows,
        // so the inner closure doesn't need per-row bounds checks.
        let max_src_end = (effective_rh.saturating_sub(1)) * src_row_bytes + copy_bytes;
        if max_src_end > src.len() {
            // Fall through to the scaled path for safety.
        } else {
            let dst_start = rx * 4;

            let blit_identity_row = |dy: usize, row_slice: &mut [u8]| {
                let src_start = dy * src_row_bytes;
                let src_row = &src[src_start..src_start + copy_bytes];
                let dst_end = dst_start + copy_bytes;
                if dst_end > row_slice.len() {
                    return;
                }
                // Check if the source row has any semi-transparent pixels.
                // For fully-opaque rows, use bulk memcpy.  For rows with alpha,
                // fall back to per-pixel blending.
                let all_opaque;
                #[cfg(target_arch = "x86_64")]
                {
                    all_opaque = if is_x86_feature_detected!("avx2") {
                        unsafe { all_alpha_opaque_avx2(src_row) }
                    } else {
                        unsafe { all_alpha_opaque_sse2(src_row) }
                    };
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    all_opaque = src_row.chunks_exact(4).all(|px| px[3] == 255);
                }

                if all_opaque {
                    row_slice[dst_start..dst_end].copy_from_slice(src_row);
                } else {
                    // Per-pixel alpha blend (identity scale, so sx == dx).
                    for dx in 0..effective_rect_w {
                        let si = dx * 4;
                        let sa = src_row[si + 3];
                        if sa == 255 {
                            row_slice[dst_start + dx * 4..dst_start + dx * 4 + 4]
                                .copy_from_slice(&src_row[si..si + 4]);
                        } else if sa > 0 {
                            let di = dst_start + dx * 4;
                            let a16 = u16::from(sa);
                            row_slice[di] = blend_u8(src_row[si], row_slice[di], a16);
                            row_slice[di + 1] = blend_u8(src_row[si + 1], row_slice[di + 1], a16);
                            row_slice[di + 2] = blend_u8(src_row[si + 2], row_slice[di + 2], a16);
                            let da = u16::from(row_slice[di + 3]);
                            row_slice[di + 3] =
                                (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
                        }
                    }
                }
            };

            if effective_rh >= RAYON_ROW_THRESHOLD {
                dst_rows.par_chunks_mut(row_stride).take(effective_rh).enumerate().for_each(
                    |(dy, row_slice)| {
                        blit_identity_row(dy, row_slice);
                    },
                );
            } else {
                for (dy, row_slice) in
                    dst_rows.chunks_mut(row_stride).take(effective_rh).enumerate()
                {
                    blit_identity_row(dy, row_slice);
                }
            }
            return;
        }
    }

    // ── Scaled blit path ───────────────────────────────────────────────
    // Precompute the source-X lookup table once.  This replaces the per-pixel
    // `(dx + src_col_skip) * sw / rw` integer division with a single table
    // lookup in the inner blit loops.
    let x_map: Vec<usize> = (0..effective_rect_w).map(|dx| (dx + src_col_skip) * sw / rw).collect();

    if effective_rh >= RAYON_ROW_THRESHOLD {
        dst_rows.par_chunks_mut(row_stride).take(effective_rh).enumerate().for_each(
            |(dy, row_slice)| {
                let sy = (dy + src_row_skip) * sh / rh;
                blit_row(row_slice, rx, effective_rect_w, src, sw, sy, opacity, &x_map);
            },
        );
    } else {
        for (dy, row_slice) in dst_rows.chunks_mut(row_stride).take(effective_rh).enumerate() {
            let sy = (dy + src_row_skip) * sh / rh;
            blit_row(row_slice, rx, effective_rect_w, src, sw, sy, opacity, &x_map);
        }
    }
}

/// Blit a single row of the source onto a destination row slice.
///
/// This is the inner kernel extracted so that `scale_blit_rgba` can dispatch
/// rows in parallel.  The `row_slice` covers exactly one destination row
/// starting at pixel column 0 (i.e. byte offset `rx * 4` is the first column
/// we write to).
///
/// `x_map` is a precomputed table mapping each destination column to the
/// corresponding source column, eliminating per-pixel integer division.
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
    sy: usize,
    opacity: f32,
    x_map: &[usize],
) {
    // Fast path: when opacity is 1.0, we can skip the f32 multiply on alpha
    // and branch more cheaply.
    if opacity >= 1.0 {
        blit_row_opaque(row_slice, rx, effective_rw, src, sw, sy, x_map);
    } else {
        blit_row_alpha(row_slice, rx, effective_rw, src, sw, sy, opacity, x_map);
    }
}

/// Inner blit for fully-opaque layers (`opacity >= 1.0`).  Skips the
/// per-pixel f32 multiply on the source alpha channel.
///
/// Uses integer-only alpha blending for semi-transparent source pixels.
/// `x_map` provides precomputed source-X indices (one per destination column).
///
/// On x86-64, processes 4 pixels at a time using SSE2 SIMD when the row is
/// wide enough and bounds can be pre-validated.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::suboptimal_flops,
    clippy::inline_always,
    // dx is used as both x_map index and dst offset, so an iterator is non-trivial.
    clippy::needless_range_loop,
    // AVX2 block has side-effects (SIMD writes) before assigning dx_start.
    clippy::useless_let_if_seq
)]
#[inline(always)]
fn blit_row_opaque(
    row_slice: &mut [u8],
    rx: usize,
    effective_rw: usize,
    src: &[u8],
    sw: usize,
    sy: usize,
    x_map: &[usize],
) {
    let src_row_base = sy * sw * 4;

    // ── SIMD fast path: AVX2 (8px) → SSE2 (4px) → scalar tail ─────────
    #[cfg(target_arch = "x86_64")]
    {
        // Pre-validate bounds so the inner SIMD loop is branch-free.
        let src_row_end = src_row_base + sw * 4;
        let dst_end = (rx + effective_rw) * 4;
        if src_row_end <= src.len() && dst_end <= row_slice.len() {
            let mut dx_start = 0usize;

            // AVX2: process 8 pixels at a time.
            if is_x86_feature_detected!("avx2") {
                let chunks8 = effective_rw / 8;
                for c in 0..chunks8 {
                    let dx = c * 8;
                    unsafe {
                        let pixels = [
                            read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 4] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 5] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 6] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 7] * 4),
                        ];
                        blend_8px_opaque_avx2(row_slice.as_mut_ptr().add((rx + dx) * 4), pixels);
                    }
                }
                dx_start = chunks8 * 8;
            }

            // SSE2: process remaining pixels in 4-pixel chunks.
            let chunks4 = (effective_rw - dx_start) / 4;
            for c in 0..chunks4 {
                let dx = dx_start + c * 4;
                unsafe {
                    let pixels = [
                        read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                    ];
                    blend_4px_opaque_sse2(row_slice.as_mut_ptr().add((rx + dx) * 4), pixels);
                }
            }

            // Scalar tail for remaining 0-3 pixels.
            let tail_start = dx_start + chunks4 * 4;
            for dx in tail_start..effective_rw {
                let sx = x_map[dx];
                let src_idx = src_row_base + sx * 4;
                let sr = src[src_idx];
                let sg = src[src_idx + 1];
                let sb = src[src_idx + 2];
                let sa = src[src_idx + 3];
                let dst_idx = (rx + dx) * 4;
                if sa == 255 {
                    row_slice[dst_idx] = sr;
                    row_slice[dst_idx + 1] = sg;
                    row_slice[dst_idx + 2] = sb;
                    row_slice[dst_idx + 3] = 255;
                } else if sa > 0 {
                    let a16 = u16::from(sa);
                    row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], a16);
                    row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], a16);
                    row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], a16);
                    let da = u16::from(row_slice[dst_idx + 3]);
                    row_slice[dst_idx + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
                }
            }
            return;
        }
    }

    // ── Scalar fallback (bounds-checked per pixel) ─────────────────────
    for dx in 0..effective_rw {
        let sx = x_map[dx];
        let src_idx = src_row_base + sx * 4;
        if src_idx + 3 >= src.len() {
            continue;
        }

        let sr = src[src_idx];
        let sg = src[src_idx + 1];
        let sb = src[src_idx + 2];
        let sa = src[src_idx + 3];

        let dst_idx = (rx + dx) * 4;
        if dst_idx + 3 >= row_slice.len() {
            continue;
        }

        if sa == 255 {
            row_slice[dst_idx] = sr;
            row_slice[dst_idx + 1] = sg;
            row_slice[dst_idx + 2] = sb;
            row_slice[dst_idx + 3] = 255;
        } else if sa > 0 {
            let a16 = u16::from(sa);
            row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], a16);
            row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], a16);
            row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], a16);
            let da = u16::from(row_slice[dst_idx + 3]);
            row_slice[dst_idx + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
        }
    }
}

/// Inner blit for layers with fractional opacity (`opacity < 1.0`).
/// Applies the opacity multiplier to every source pixel's alpha channel.
///
/// Uses integer-only alpha blending.
/// `x_map` provides precomputed source-X indices (one per destination column).
///
/// On x86-64, processes 4 pixels at a time using SSE2 SIMD when the row is
/// wide enough and bounds can be pre-validated.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::suboptimal_flops,
    clippy::inline_always,
    // dx is used as both x_map index and dst offset, so an iterator is non-trivial.
    clippy::needless_range_loop,
    // AVX2 block has side-effects (SIMD writes) before assigning dx_start.
    clippy::useless_let_if_seq
)]
#[inline(always)]
fn blit_row_alpha(
    row_slice: &mut [u8],
    rx: usize,
    effective_rw: usize,
    src: &[u8],
    sw: usize,
    sy: usize,
    opacity: f32,
    x_map: &[usize],
) {
    // Pre-compute opacity as a 0..255 integer multiplier.
    let opacity_u16 = (opacity * 255.0 + 0.5) as u16;
    let src_row_base = sy * sw * 4;

    // ── SIMD fast path: AVX2 (8px) → SSE2 (4px) → scalar tail ─────────
    #[cfg(target_arch = "x86_64")]
    {
        let src_row_end = src_row_base + sw * 4;
        let dst_end = (rx + effective_rw) * 4;
        if src_row_end <= src.len() && dst_end <= row_slice.len() {
            let mut dx_start = 0usize;

            // AVX2: process 8 pixels at a time.
            if is_x86_feature_detected!("avx2") {
                let chunks8 = effective_rw / 8;
                for c in 0..chunks8 {
                    let dx = c * 8;
                    unsafe {
                        let pixels = [
                            read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 4] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 5] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 6] * 4),
                            read_rgba_u32(src, src_row_base + x_map[dx + 7] * 4),
                        ];
                        blend_8px_alpha_avx2(
                            row_slice.as_mut_ptr().add((rx + dx) * 4),
                            pixels,
                            opacity_u16,
                        );
                    }
                }
                dx_start = chunks8 * 8;
            }

            // SSE2: process remaining pixels in 4-pixel chunks.
            let chunks4 = (effective_rw - dx_start) / 4;
            for c in 0..chunks4 {
                let dx = dx_start + c * 4;
                unsafe {
                    let pixels = [
                        read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                    ];
                    blend_4px_alpha_sse2(
                        row_slice.as_mut_ptr().add((rx + dx) * 4),
                        pixels,
                        opacity_u16,
                    );
                }
            }

            // Scalar tail.
            let tail_start = dx_start + chunks4 * 4;
            for dx in tail_start..effective_rw {
                let sx = x_map[dx];
                let src_idx = src_row_base + sx * 4;
                let sr = src[src_idx];
                let sg = src[src_idx + 1];
                let sb = src[src_idx + 2];
                let sa = src[src_idx + 3];
                let dst_idx = (rx + dx) * 4;
                let sa_eff = ((u16::from(sa) * opacity_u16 + 128) >> 8).min(255);
                if sa_eff == 255 {
                    row_slice[dst_idx] = sr;
                    row_slice[dst_idx + 1] = sg;
                    row_slice[dst_idx + 2] = sb;
                    row_slice[dst_idx + 3] = 255;
                } else if sa_eff > 0 {
                    row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], sa_eff);
                    row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], sa_eff);
                    row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], sa_eff);
                    let da = u16::from(row_slice[dst_idx + 3]);
                    row_slice[dst_idx + 3] =
                        (sa_eff + ((da * (255 - sa_eff) + 128) >> 8)).min(255) as u8;
                }
            }
            return;
        }
    }

    // ── Scalar fallback ────────────────────────────────────────────────
    for dx in 0..effective_rw {
        let sx = x_map[dx];
        let src_idx = src_row_base + sx * 4;
        if src_idx + 3 >= src.len() {
            continue;
        }

        let sr = src[src_idx];
        let sg = src[src_idx + 1];
        let sb = src[src_idx + 2];
        let sa = src[src_idx + 3];

        let dst_idx = (rx + dx) * 4;
        if dst_idx + 3 >= row_slice.len() {
            continue;
        }

        let sa_eff = ((u16::from(sa) * opacity_u16 + 128) >> 8).min(255);
        if sa_eff == 255 {
            row_slice[dst_idx] = sr;
            row_slice[dst_idx + 1] = sg;
            row_slice[dst_idx + 2] = sb;
            row_slice[dst_idx + 3] = 255;
        } else if sa_eff > 0 {
            row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], sa_eff);
            row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], sa_eff);
            row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], sa_eff);
            let da = u16::from(row_slice[dst_idx + 3]);
            row_slice[dst_idx + 3] = (sa_eff + ((da * (255 - sa_eff) + 128) >> 8)).min(255) as u8;
        }
    }
}

// ── Rotated blitting ────────────────────────────────────────────────────────

/// Scale and blit a source RGBA8 buffer onto a destination RGBA8 buffer at the
/// given destination rectangle with clockwise rotation around the rect centre.
///
/// The source is stretched to fill the destination rect (no aspect-ratio-
/// preserving fit).  Aspect ratio handling is the responsibility of the
/// caller / presentation layer.  The stretched content is then rotated
/// around the rect centre.
///
/// Uses inverse-affine mapping with nearest-neighbor sampling.  Edge pixels
/// receive fractional alpha coverage computed from the signed distance to each
/// of the four rect edges in the un-rotated local coordinate system.  This
/// eliminates the staircase aliasing that a hard binary inside/outside test
/// would produce.
///
/// For near-zero rotation angles (< 0.01°), a fast path delegates directly
/// to [`scale_blit_rgba`] which performs the same stretch-to-fill without
/// the rotation overhead.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::cast_possible_wrap
)]
pub fn scale_blit_rgba_rotated(
    dst: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_rect: &Rect,
    opacity: f32,
    rotation_deg: f32,
) {
    if src_width == 0 || src_height == 0 || dst_rect.width == 0 || dst_rect.height == 0 {
        return;
    }

    let rw = dst_rect.width as f32;
    let rh = dst_rect.height as f32;

    // ── Near-zero rotation fast path ──────────────────────────────────
    // Delegate to the optimised non-rotated blit which stretches the
    // source to fill the destination rect (no aspect-ratio fitting).
    if rotation_deg.abs() < 0.01 {
        scale_blit_rgba(dst, dst_width, dst_height, src, src_width, src_height, dst_rect, opacity);
        return;
    }

    let dw = dst_width.cast_signed();
    let dh = dst_height.cast_signed();
    let sw = src_width as usize;
    let sh = src_height as usize;

    // Pre-compute sin/cos for the rotation (needed for the bounding-box
    // computation and for the per-pixel inverse mapping).
    let angle_rad = rotation_deg.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    // ── Stretch-to-fill scaling ──────────────────────────────────────
    // The source is stretched to fill the destination rect (no
    // aspect-ratio-preserving fit).  Aspect ratio handling is the
    // responsibility of the client / presentation layer.
    let half_cw = rw * 0.5;
    let half_ch = rh * 0.5;
    let inv_scale_x = src_width as f32 / rw;
    let inv_scale_y = src_height as f32 / rh;

    // Rotation centre = centre of the destination rect.
    let cx = rw.mul_add(0.5, dst_rect.x as f32);
    let cy = rh.mul_add(0.5, dst_rect.y as f32);

    // Compute the axis-aligned bounding box of the rotated *content* area
    // (not the full rect) so we only iterate over pixels that could
    // possibly be covered by actual source content.
    let corners =
        [(-half_cw, -half_ch), (half_cw, -half_ch), (half_cw, half_ch), (-half_cw, half_ch)];
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for (lx, ly) in &corners {
        let rx = lx * cos_a - ly * sin_a + cx;
        let ry = lx * sin_a + ly * cos_a + cy;
        min_x = min_x.min(rx);
        max_x = max_x.max(rx);
        min_y = min_y.min(ry);
        max_y = max_y.max(ry);
    }

    // Expand bounding box by 1px on each side so the AA fringe is included.
    let bb_x0 = ((min_x.floor() as i32) - 1).max(0);
    let bb_y0 = ((min_y.floor() as i32) - 1).max(0);
    let bb_x1 = ((max_x.ceil() as i32) + 1).min(dw);
    let bb_y1 = ((max_y.ceil() as i32) + 1).min(dh);

    let row_stride = dst_width as usize * 4;

    // Pre-compute opacity as a 0..255 integer multiplier.
    let opacity_u16 = if opacity < 1.0 {
        opacity.mul_add(255.0, 0.5) as u16
    } else {
        256 // sentinel: means "fully opaque, skip per-pixel multiply"
    };

    // Per-row closure that processes all columns in a single row of the
    // bounding box.  Uses an incremental stepper: since `dx_f` increments
    // by 1.0 each column, `local_x` and `local_y` change by `+cos_a` and
    // `-sin_a` respectively — replacing 2 multiplies with 2 adds per pixel.
    //
    // Edge anti-aliasing distances are computed against the rect boundary
    // (`half_cw` × `half_ch`), so the visible edge matches the rect.
    //
    // For interior pixels where `min_dist >= 1.0` the edge-coverage clamp
    // is a no-op, so we skip the coverage math entirely for the bulk of
    // each span.
    let process_row = |py: i32, row_slice: &mut [u8]| {
        let dy = py as f32 - cy;

        // Seed the stepper at the first column of the bounding box.
        let dx_f0 = bb_x0 as f32 - cx;
        let mut local_x = dx_f0 * cos_a + dy * sin_a;
        let mut local_y = (-dx_f0).mul_add(sin_a, dy * cos_a);

        let mut px = bb_x0;
        while px < bb_x1 {
            // ── Edge anti-aliasing via signed distance ──────────────
            // Distances are relative to the content boundary, not the
            // full destination rect.
            let d_left = local_x + half_cw;
            let d_right = half_cw - local_x;
            let d_top = local_y + half_ch;
            let d_bottom = half_ch - local_y;
            let min_dist = d_left.min(d_right).min(d_top).min(d_bottom);

            if min_dist <= 0.0 {
                // Fully outside content area — step and continue.
                local_x += cos_a;
                local_y -= sin_a;
                px += 1;
                continue;
            }

            // Map from rect-local coords to source pixel coords.
            // `local_x/y` ∈ [-half_cw, half_cw] × [-half_ch, half_ch]
            // for points inside the rect.  Convert to source pixel
            // space via the per-axis inverse scale.
            let src_fx = (local_x + half_cw) * inv_scale_x;
            let src_fy = (local_y + half_ch) * inv_scale_y;

            let sxi = (src_fx as usize).min(sw - 1);
            let syi = (src_fy as usize).min(sh - 1);

            let src_idx = (syi * sw + sxi) * 4;
            if src_idx + 3 >= src.len() {
                local_x += cos_a;
                local_y -= sin_a;
                px += 1;
                continue;
            }

            let sr = src[src_idx];
            let sg = src[src_idx + 1];
            let sb = src[src_idx + 2];
            let mut sa = src[src_idx + 3];

            // Apply layer opacity.
            if opacity_u16 < 256 {
                sa = ((u16::from(sa) * opacity_u16 + 128) >> 8).min(255) as u8;
            }

            // Apply edge coverage only when near a border.
            if min_dist < 1.0 {
                sa = f32::from(sa).mul_add(min_dist, 0.5) as u8;
            }

            if sa > 0 {
                let dst_off = px as usize * 4;
                if dst_off + 3 < row_slice.len() {
                    if sa == 255 {
                        row_slice[dst_off] = sr;
                        row_slice[dst_off + 1] = sg;
                        row_slice[dst_off + 2] = sb;
                        row_slice[dst_off + 3] = 255;
                    } else {
                        let a16 = u16::from(sa);
                        row_slice[dst_off] = blend_u8(sr, row_slice[dst_off], a16);
                        row_slice[dst_off + 1] = blend_u8(sg, row_slice[dst_off + 1], a16);
                        row_slice[dst_off + 2] = blend_u8(sb, row_slice[dst_off + 2], a16);
                        let da = u16::from(row_slice[dst_off + 3]);
                        row_slice[dst_off + 3] =
                            (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
                    }
                }
            }

            // Interior fast-forward: if we are well inside the content
            // area (min_dist >= 2.0), subsequent pixels will also be
            // interior until we approach an edge.
            //
            // The minimum distance decreases by at most 1.0 per column
            // step (the directional derivative of each edge distance w.r.t.
            // the column step is at most ±1 since |cos_a|, |sin_a| ≤ 1).
            // So if min_dist >= 2.0, at least the next pixel is also fully
            // interior (min_dist ≥ 1.0).  We use this to batch interior
            // pixels with a tighter loop that skips the coverage branch.
            if min_dist >= 2.0 {
                // Number of pixels we can safely process without AA.
                // Conservative: (min_dist - 1.0).floor() guarantees
                // min_dist stays >= 1.0 for all skipped pixels.
                let skip = ((min_dist - 1.0).floor() as i32).min(bb_x1 - px - 1);
                if skip > 0 {
                    let skip_u = skip as usize;

                    // ── SSE2 batched path: process 4 interior pixels at a time ──
                    #[cfg(target_arch = "x86_64")]
                    {
                        let mut done = 0usize;
                        // Process groups of 4 pixels with SIMD blending.
                        while done + 4 <= skip_u {
                            // Gather 4 source pixels into a u32 array for the
                            // existing SSE2 blend helpers.
                            let mut src_pixels = [0u32; 4];
                            let mut all_valid = true;
                            // Save stepper state so we can restore on early
                            // break from the gather loop.
                            let snap_local_x = local_x;
                            let snap_local_y = local_y;
                            for sp in &mut src_pixels {
                                local_x += cos_a;
                                local_y -= sin_a;
                                let isx =
                                    (((local_x + half_cw) * inv_scale_x) as usize).min(sw - 1);
                                let isy =
                                    (((local_y + half_ch) * inv_scale_y) as usize).min(sh - 1);
                                let si = (isy * sw + isx) * 4;
                                if si + 3 < src.len() {
                                    // SAFETY: bounds checked above.
                                    *sp = unsafe { read_rgba_u32(src, si) };
                                } else {
                                    all_valid = false;
                                    break;
                                }
                            }

                            if !all_valid {
                                // Restore stepper; fall through to scalar
                                // remainder which handles partial pixels.
                                local_x = snap_local_x;
                                local_y = snap_local_y;
                                break;
                            }

                            // `done` tracks offset from the current `px`;
                            // `px` is NOT incremented inside this loop to
                            // avoid double-counting.
                            let dst_off = (px as usize + 1 + done) * 4;
                            if dst_off + 15 < row_slice.len() {
                                // SAFETY: blend helpers require 16 writable
                                // bytes at dst_ptr; bounds checked above.
                                // SSE2 is always available on x86_64.
                                unsafe {
                                    let dst_ptr = row_slice.as_mut_ptr().add(dst_off);
                                    if opacity_u16 >= 256 {
                                        blend_4px_opaque_sse2(dst_ptr, src_pixels);
                                    } else {
                                        blend_4px_alpha_sse2(dst_ptr, src_pixels, opacity_u16);
                                    }
                                }
                            }

                            done += 4;
                        }

                        // Advance px by the number of pixels handled above.
                        #[allow(clippy::cast_possible_wrap)]
                        {
                            px += done as i32;
                        }

                        // Scalar remainder for leftover pixels.
                        for _ in done..skip_u {
                            local_x += cos_a;
                            local_y -= sin_a;
                            px += 1;

                            let isx = (((local_x + half_cw) * inv_scale_x) as usize).min(sw - 1);
                            let isy = (((local_y + half_ch) * inv_scale_y) as usize).min(sh - 1);
                            let si = (isy * sw + isx) * 4;
                            if si + 3 >= src.len() {
                                continue;
                            }

                            blend_pixel_scalar(row_slice, px as usize * 4, src, si, opacity_u16);
                        }
                    }

                    // ── Non-x86_64 fallback: scalar loop ──
                    #[cfg(not(target_arch = "x86_64"))]
                    {
                        for _ in 0..skip_u {
                            local_x += cos_a;
                            local_y -= sin_a;
                            px += 1;

                            let isx = (((local_x + half_cw) * inv_scale_x) as usize).min(sw - 1);
                            let isy = (((local_y + half_ch) * inv_scale_y) as usize).min(sh - 1);
                            let si = (isy * sw + isx) * 4;
                            if si + 3 >= src.len() {
                                continue;
                            }

                            blend_pixel_scalar(row_slice, px as usize * 4, src, si, opacity_u16);
                        }
                    }
                }
            }

            local_x += cos_a;
            local_y -= sin_a;
            px += 1;
        }
    };

    // Early-out when the bounding box is empty (rect entirely off-screen).
    if bb_y1 <= bb_y0 || bb_x1 <= bb_x0 {
        return;
    }

    let bb_rows = (bb_y1 - bb_y0) as usize;
    let first_row_byte = bb_y0 as usize * row_stride;
    let dst_region = &mut dst[first_row_byte..first_row_byte + bb_rows * row_stride];

    if bb_rows >= RAYON_ROW_THRESHOLD {
        use rayon::prelude::*;
        let chunk_rows = rayon_chunk_rows(bb_rows);
        let chunk_bytes = row_stride * chunk_rows;
        dst_region.par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, row_slice) in chunk.chunks_mut(row_stride).enumerate() {
                let row = base_row + j;
                // `row` is bounded by `bb_rows` which derives from i32 bounding-box coords.
                #[allow(clippy::cast_possible_wrap)]
                process_row(bb_y0 + row as i32, row_slice);
            }
        });
    } else {
        for (i, row_slice) in dst_region.chunks_mut(row_stride).enumerate() {
            // `i` is bounded by `bb_rows` which derives from i32 bounding-box coords.
            #[allow(clippy::cast_possible_wrap)]
            process_row(bb_y0 + i as i32, row_slice);
        }
    }
}
