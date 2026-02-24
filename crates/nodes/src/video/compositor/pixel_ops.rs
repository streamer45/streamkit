// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pixel-level operations for the video compositor.
//!
//! Contains RGBA8 blitting (with nearest-neighbor scaling), alpha blending,
//! overlay compositing, and I420 ↔ RGBA8 colour-space conversion.
//!
//! All hot loops use row-level parallelism via `rayon`.

use super::config::Rect;
use super::overlay::DecodedOverlay;

// ── Compositing helpers ─────────────────────────────────────────────────────

/// Scale and blit a source RGBA8 buffer onto a destination RGBA8 buffer at the
/// given destination rectangle. Uses nearest-neighbor sampling and clips to
/// canvas bounds.
///
/// Rows are processed in parallel via `rayon` when the blit region is large
/// enough to benefit from multi-core dispatch.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::too_many_arguments)]
pub(crate) fn scale_blit_rgba(
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

    dst_rows.par_chunks_mut(row_stride).take(effective_rh).enumerate().for_each(
        |(dy, row_slice)| {
            let sy = dy * sh / rh;
            blit_row(row_slice, rx, effective_rw, src, sw, sh, sy, rw, opacity);
        },
    );
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

/// Fixed-point alpha blend: `(src * alpha + dst * (255 - alpha) + 128) / 255`
/// using the well-known `((x + (x >> 8)) >> 8)` fast approximation of `x / 255`.
#[allow(clippy::inline_always)]
#[inline(always)]
const fn blend_u8(src: u8, dst: u8, alpha: u16) -> u8 {
    let inv = 255 - alpha;
    let val = src as u16 * alpha + dst as u16 * inv + 128;
    ((val + (val >> 8)) >> 8) as u8
}

/// Inner blit for fully-opaque layers (`opacity >= 1.0`).  Skips the
/// per-pixel f32 multiply on the source alpha channel.
///
/// Uses integer-only alpha blending for semi-transparent source pixels.
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
    let src_row_base = sy * sw * 4;
    for dx in 0..effective_rw {
        let sx = dx * sw / rw;
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
            let a16 = sa as u16;
            row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], a16);
            row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], a16);
            row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], a16);
            // Composite alpha: a_out = a_src + a_dst * (1 - a_src)
            let da = row_slice[dst_idx + 3] as u16;
            row_slice[dst_idx + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
        }
    }
}

/// Inner blit for layers with fractional opacity (`opacity < 1.0`).
/// Applies the opacity multiplier to every source pixel's alpha channel.
///
/// Uses integer-only alpha blending.
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
    // Pre-compute opacity as a 0..255 integer multiplier.
    let opacity_u16 = (opacity * 255.0 + 0.5) as u16;
    let src_row_base = sy * sw * 4;

    for dx in 0..effective_rw {
        let sx = dx * sw / rw;
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

        // Effective alpha: (sa * opacity) / 255, done in integer.
        let sa_eff = ((sa as u16 * opacity_u16 + 128) >> 8).min(255) as u16;
        if sa_eff == 255 {
            row_slice[dst_idx] = sr;
            row_slice[dst_idx + 1] = sg;
            row_slice[dst_idx + 2] = sb;
            row_slice[dst_idx + 3] = 255;
        } else if sa_eff > 0 {
            row_slice[dst_idx] = blend_u8(sr, row_slice[dst_idx], sa_eff);
            row_slice[dst_idx + 1] = blend_u8(sg, row_slice[dst_idx + 1], sa_eff);
            row_slice[dst_idx + 2] = blend_u8(sb, row_slice[dst_idx + 2], sa_eff);
            let da = row_slice[dst_idx + 3] as u16;
            row_slice[dst_idx + 3] = (sa_eff + ((da * (255 - sa_eff) + 128) >> 8)).min(255) as u8;
        }
    }
}

/// Blit a pre-decoded overlay onto the canvas (full alpha blend at the
/// overlay's configured opacity).
pub(crate) fn blit_overlay(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    overlay: &DecodedOverlay,
) {
    scale_blit_rgba(
        canvas,
        canvas_w,
        canvas_h,
        &overlay.rgba_data,
        overlay.width,
        overlay.height,
        &overlay.rect,
        overlay.opacity,
    );
}

// ── Pixel format conversion ─────────────────────────────────────────────────

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8, writing into `out`.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Rows are processed in parallel via `rayon`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn i420_to_rgba8_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;
    let u_offset = y_stride * h;
    let v_offset = u_offset + chroma_w * chroma_h;
    let rgba_row_stride = w * 4;

    out[..w * h * 4].par_chunks_mut(rgba_row_stride).take(h).enumerate().for_each(
        |(row, rgba_row)| {
            // Sub-slice the Y/U/V input planes for this row so the compiler
            // can reason about lengths and potentially elide bounds checks
            // on the inner indexing.
            let y_base = row * y_stride;
            let chroma_row = row / 2;
            let u_base = u_offset + chroma_row * chroma_w;
            let v_base = v_offset + chroma_row * chroma_w;

            for col in 0..w {
                let y_val = data[y_base + col] as i32;
                let u_val = data[u_base + col / 2] as i32;
                let v_val = data[v_base + col / 2] as i32;

                let c = y_val - 16;
                let d = u_val - 128;
                let e = v_val - 128;

                let off = col * 4;
                rgba_row[off] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                rgba_row[off + 1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                rgba_row[off + 2] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
                rgba_row[off + 3] = 255;
            }
        },
    );
}

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8 (allocating variant).
///
/// Prefer [`i420_to_rgba8_buf`] with a pooled buffer to avoid per-frame allocation.
#[allow(dead_code, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn i420_to_rgba8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    i420_to_rgba8_buf(data, width, height, &mut rgba);
    rgba
}

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + 2 * ((w+1)/2) * ((h+1)/2)`.
/// Y, U and V planes are processed in parallel via `rayon`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn rgba8_to_i420_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;
    let y_size = y_stride * h;
    let chroma_size = chroma_w * chroma_h;

    // Split output into Y and chroma planes.
    let (y_plane, chroma_planes) = out[..y_size + 2 * chroma_size].split_at_mut(y_size);
    let (u_plane, v_plane) = chroma_planes.split_at_mut(chroma_size);

    // Y plane — parallelise by row.
    y_plane.par_chunks_mut(y_stride).take(h).enumerate().for_each(|(row, y_row)| {
        let rgba_base = row * w * 4;

        for col in 0..w {
            let off = rgba_base + col * 4;
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

    u_rows.into_par_iter().zip(v_rows).enumerate().for_each(|(crow, (u_row, v_row))| {
        let r0 = crow * 2;
        for ccol in 0..chroma_w {
            let c0 = ccol * 2;
            let mut sr = 0i32;
            let mut sg = 0i32;
            let mut sb = 0i32;
            let mut count = 0i32;
            for dr in 0..2 {
                let rr = r0 + dr;
                if rr >= h {
                    continue;
                }
                for dc in 0..2 {
                    let cc = c0 + dc;
                    if cc < w {
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
}

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar) (allocating variant).
///
/// Prefer [`rgba8_to_i420_buf`] with a pooled buffer to avoid per-frame allocation.
#[allow(dead_code, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn rgba8_to_i420(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let chroma_w = (w + 1) / 2;
    let chroma_h = (h + 1) / 2;
    let total = w * h + 2 * chroma_w * chroma_h;
    let mut out = vec![0u8; total];
    rgba8_to_i420_buf(data, width, height, &mut out);
    out
}
