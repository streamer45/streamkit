// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pixel-level operations for the video compositor.
//!
//! Contains RGBA8 blitting (with nearest-neighbor scaling), alpha blending,
//! overlay compositing, and I420 ↔ RGBA8 colour-space conversion.
//!
//! All hot loops use row-level parallelism via `rayon` when the region is
//! large enough to amortise the thread-pool dispatch overhead.  Below the
//! threshold the same per-row closures run sequentially.

use super::config::Rect;
use super::overlay::DecodedOverlay;

/// Minimum number of output rows before we dispatch to rayon.  Below this
/// threshold the per-row work is small enough that the rayon scheduling
/// overhead (work-stealing queue push/pop, thread wake-up) dominates.
/// 64 rows at 1280-wide RGBA8 ≈ 320 KiB — a reasonable crossover point
/// on modern x86-64 cores.
const RAYON_ROW_THRESHOLD: usize = 64;

// ── Compositing helpers ─────────────────────────────────────────────────────

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

    if effective_rh >= RAYON_ROW_THRESHOLD {
        dst_rows.par_chunks_mut(row_stride).take(effective_rh).enumerate().for_each(
            |(dy, row_slice)| {
                let sy = (dy + src_row_skip) * sh / rh;
                blit_row(
                    row_slice,
                    rx,
                    effective_rect_w,
                    src,
                    sw,
                    sh,
                    sy,
                    rw,
                    opacity,
                    src_col_skip,
                );
            },
        );
    } else {
        for (dy, row_slice) in dst_rows.chunks_mut(row_stride).take(effective_rh).enumerate() {
            let sy = (dy + src_row_skip) * sh / rh;
            blit_row(row_slice, rx, effective_rect_w, src, sw, sh, sy, rw, opacity, src_col_skip);
        }
    }
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
    src_col_skip: usize,
) {
    // Fast path: when opacity is 1.0, we can skip the f32 multiply on alpha
    // and branch more cheaply.
    if opacity >= 1.0 {
        blit_row_opaque(row_slice, rx, effective_rw, src, sw, sh, sy, rw, src_col_skip);
    } else {
        blit_row_alpha(row_slice, rx, effective_rw, src, sw, sh, sy, rw, opacity, src_col_skip);
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
    src_col_skip: usize,
) {
    let src_row_base = sy * sw * 4;
    for dx in 0..effective_rw {
        let sx = (dx + src_col_skip) * sw / rw;
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
            // Composite alpha: a_out = a_src + a_dst * (1 - a_src)
            let da = u16::from(row_slice[dst_idx + 3]);
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
    src_col_skip: usize,
) {
    // Pre-compute opacity as a 0..255 integer multiplier.
    let opacity_u16 = (opacity * 255.0 + 0.5) as u16;
    let src_row_base = sy * sw * 4;

    for dx in 0..effective_rw {
        let sx = (dx + src_col_skip) * sw / rw;
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

/// Blit a pre-decoded overlay onto the canvas (full alpha blend at the
/// overlay's configured opacity).
pub fn blit_overlay(canvas: &mut [u8], canvas_w: u32, canvas_h: u32, overlay: &DecodedOverlay) {
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

// ── Rotated blitting ────────────────────────────────────────────────────────

/// Scale and blit a source RGBA8 buffer onto a destination RGBA8 buffer at the
/// given destination rectangle with clockwise rotation around the rect centre.
///
/// Uses inverse-affine mapping with nearest-neighbor sampling.  Falls back to
/// [`scale_blit_rgba`] when `rotation_deg` is effectively zero.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::cast_precision_loss
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
    // Fast path: no rotation → delegate to the optimised non-rotated blit.
    if rotation_deg.abs() < 0.01 {
        scale_blit_rgba(dst, dst_width, dst_height, src, src_width, src_height, dst_rect, opacity);
        return;
    }

    if src_width == 0 || src_height == 0 || dst_rect.width == 0 || dst_rect.height == 0 {
        return;
    }

    let dw = dst_width as i32;
    let dh = dst_height as i32;
    let sw = src_width as usize;
    let sh = src_height as usize;
    let rw = dst_rect.width as f32;
    let rh = dst_rect.height as f32;

    // Rotation centre = centre of the destination rect.
    let cx = dst_rect.x as f32 + rw * 0.5;
    let cy = dst_rect.y as f32 + rh * 0.5;

    // Pre-compute sin/cos for the *inverse* rotation (clockwise rotation
    // means we apply counter-clockwise to map destination → source).
    let angle_rad = rotation_deg.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    // Compute the axis-aligned bounding box of the rotated rect so we only
    // iterate over pixels that could possibly be covered.
    let half_w = rw * 0.5;
    let half_h = rh * 0.5;
    let corners = [(-half_w, -half_h), (half_w, -half_h), (half_w, half_h), (-half_w, half_h)];
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

    let bb_x0 = (min_x.floor() as i32).max(0);
    let bb_y0 = (min_y.floor() as i32).max(0);
    let bb_x1 = (max_x.ceil() as i32).min(dw);
    let bb_y1 = (max_y.ceil() as i32).min(dh);

    let row_stride = dst_width as usize * 4;

    // Pre-compute opacity as a 0..255 integer multiplier.
    let opacity_u16 = if opacity < 1.0 {
        (opacity * 255.0 + 0.5) as u16
    } else {
        256 // sentinel: means "fully opaque, skip per-pixel multiply"
    };

    for py in bb_y0..bb_y1 {
        let dy = py as f32 - cy;
        let row_base = py as usize * row_stride;
        for px in bb_x0..bb_x1 {
            let dx_f = px as f32 - cx;

            // Inverse-rotate to get position in the un-rotated rect's
            // local coordinate system (origin at rect centre).
            let local_x = dx_f * cos_a + dy * sin_a;
            let local_y = -dx_f * sin_a + dy * cos_a;

            // Map from local rect coords ([-half_w, half_w]) to source
            // pixel coords ([0, sw)).
            let norm_x = (local_x + half_w) / rw;
            let norm_y = (local_y + half_h) / rh;

            if !(0.0..1.0).contains(&norm_x) || !(0.0..1.0).contains(&norm_y) {
                continue; // outside the source rectangle
            }

            let sx = (norm_x * sw as f32) as usize;
            let sy = (norm_y * sh as f32) as usize;
            let sx = sx.min(sw - 1);
            let sy = sy.min(sh - 1);

            let src_idx = (sy * sw + sx) * 4;
            if src_idx + 3 >= src.len() {
                continue;
            }

            let sr = src[src_idx];
            let sg = src[src_idx + 1];
            let sb = src[src_idx + 2];
            let mut sa = src[src_idx + 3];

            if opacity_u16 < 256 {
                sa = ((u16::from(sa) * opacity_u16 + 128) >> 8).min(255) as u8;
            }

            if sa == 0 {
                continue;
            }

            let dst_idx = row_base + px as usize * 4;
            if dst_idx + 3 >= dst.len() {
                continue;
            }

            if sa == 255 {
                dst[dst_idx] = sr;
                dst[dst_idx + 1] = sg;
                dst[dst_idx + 2] = sb;
                dst[dst_idx + 3] = 255;
            } else {
                let a16 = u16::from(sa);
                dst[dst_idx] = blend_u8(sr, dst[dst_idx], a16);
                dst[dst_idx + 1] = blend_u8(sg, dst[dst_idx + 1], a16);
                dst[dst_idx + 2] = blend_u8(sb, dst[dst_idx + 2], a16);
                let da = u16::from(dst[dst_idx + 3]);
                dst[dst_idx + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
            }
        }
    }
}

// ── Pixel format conversion ─────────────────────────────────────────────────

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8, writing into `out`.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Rows are processed in parallel via `rayon`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn i420_to_rgba8_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let u_offset = y_stride * h;
    let v_offset = u_offset + chroma_w * chroma_h;
    let rgba_row_stride = w * 4;

    let convert_row = |row: usize, rgba_row: &mut [u8]| {
        let y_base = row * y_stride;
        let chroma_row = row / 2;
        let u_base = u_offset + chroma_row * chroma_w;
        let v_base = v_offset + chroma_row * chroma_w;

        for col in 0..w {
            let y_val = i32::from(data[y_base + col]);
            let u_val = i32::from(data[u_base + col / 2]);
            let v_val = i32::from(data[v_base + col / 2]);

            let c = y_val - 16;
            let d = u_val - 128;
            let e = v_val - 128;

            let off = col * 4;
            rgba_row[off] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            rgba_row[off + 1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            rgba_row[off + 2] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            rgba_row[off + 3] = 255;
        }
    };

    if h >= RAYON_ROW_THRESHOLD {
        out[..w * h * 4]
            .par_chunks_mut(rgba_row_stride)
            .take(h)
            .enumerate()
            .for_each(|(row, rgba_row)| convert_row(row, rgba_row));
    } else {
        for (row, rgba_row) in out[..w * h * 4].chunks_mut(rgba_row_stride).take(h).enumerate() {
            convert_row(row, rgba_row);
        }
    }
}

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8 (allocating variant).
///
/// Prefer [`i420_to_rgba8_buf`] with a pooled buffer to avoid per-frame allocation.
#[allow(dead_code, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn i420_to_rgba8(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    i420_to_rgba8_buf(data, width, height, &mut rgba);
    rgba
}

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + 2 * ((w+1)/2) * ((h+1)/2)`.
/// Y, U and V planes are processed in parallel via `rayon`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn rgba8_to_i420_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let y_size = y_stride * h;
    let chroma_size = chroma_w * chroma_h;

    // Split output into Y and chroma planes.
    let (y_plane, chroma_planes) = out[..y_size + 2 * chroma_size].split_at_mut(y_size);
    let (u_plane, v_plane) = chroma_planes.split_at_mut(chroma_size);

    // Y plane — parallelise by row.
    let convert_y_row = |row: usize, y_row: &mut [u8]| {
        let rgba_base = row * w * 4;
        for (col, y_out) in y_row.iter_mut().enumerate().take(w) {
            let off = rgba_base + col * 4;
            let r = i32::from(data[off]);
            let g = i32::from(data[off + 1]);
            let b = i32::from(data[off + 2]);
            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            *y_out = y.clamp(0, 255) as u8;
        }
    };

    if h >= RAYON_ROW_THRESHOLD {
        y_plane
            .par_chunks_mut(y_stride)
            .take(h)
            .enumerate()
            .for_each(|(row, y_row)| convert_y_row(row, y_row));
    } else {
        for (row, y_row) in y_plane.chunks_mut(y_stride).take(h).enumerate() {
            convert_y_row(row, y_row);
        }
    }

    // U and V planes — parallelise by chroma row.
    let convert_chroma_row = |crow: usize, u_row: &mut [u8], v_row: &mut [u8]| {
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
                        sr += i32::from(data[off]);
                        sg += i32::from(data[off + 1]);
                        sb += i32::from(data[off + 2]);
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
    };

    let u_rows: Vec<&mut [u8]> = u_plane.chunks_mut(chroma_w).collect();
    let v_rows: Vec<&mut [u8]> = v_plane.chunks_mut(chroma_w).collect();

    if chroma_h >= RAYON_ROW_THRESHOLD / 2 {
        u_rows.into_par_iter().zip(v_rows).enumerate().for_each(|(crow, (u_row, v_row))| {
            convert_chroma_row(crow, u_row, v_row);
        });
    } else {
        for (crow, (u_row, v_row)) in u_rows.into_iter().zip(v_rows).enumerate() {
            convert_chroma_row(crow, u_row, v_row);
        }
    }
}

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar) (allocating variant).
///
/// Prefer [`rgba8_to_i420_buf`] with a pooled buffer to avoid per-frame allocation.
#[allow(dead_code, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn rgba8_to_i420(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let total = w * h + 2 * chroma_w * chroma_h;
    let mut out = vec![0u8; total];
    rgba8_to_i420_buf(data, width, height, &mut out);
    out
}
