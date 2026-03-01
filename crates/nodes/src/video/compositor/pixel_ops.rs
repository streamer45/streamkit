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

/// Number of rows to bundle into a single rayon task once parallel mode is
/// entered.  Reduces work-stealing overhead from ~1 task/row to
/// ~rows/chunk tasks.
///
/// [`rayon_chunk_rows`] auto-tunes the chunk size based on workload:
/// wider or taller frames produce fewer, larger chunks, keeping
/// scheduling cost proportional to the actual parallelism available.
///
/// Formula: `max(8, total_rows / (num_cpus * 4))`, clamped to `[8, 64]`.
/// This keeps chunk counts proportional to hardware parallelism while
/// avoiding both excessive scheduling overhead (too many tiny chunks)
/// and poor load-balancing (too few large chunks).
///
/// The CPU count is cached in a `LazyLock` so we avoid a `sysconf` syscall
/// (~40 µs on Linux) on every call.
fn rayon_chunk_rows(total_rows: usize) -> usize {
    static CPUS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        std::thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(1)
    });
    let ideal = total_rows.div_ceil(*CPUS * 4);
    ideal.clamp(8, 64)
}

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

    // ── Identity-scale fast path ───────────────────────────────────────
    // When source dimensions exactly match the destination rect and opacity
    // is fully opaque, we can avoid per-pixel scaling entirely and use
    // direct row copies (memcpy) for fully-opaque source rows.
    if rw == sw && rh == sh && opacity >= 1.0 && src_col_skip == 0 && src_row_skip == 0 {
        let src_row_bytes = sw * 4;
        let copy_bytes = effective_rect_w * 4;
        for (dy, row_slice) in dst_rows.chunks_mut(row_stride).take(effective_rh).enumerate() {
            let src_start = dy * src_row_bytes;
            let src_end = src_start + copy_bytes;
            if src_end > src.len() {
                break;
            }
            let dst_start = rx * 4;
            let dst_end = dst_start + copy_bytes;
            if dst_end > row_slice.len() {
                break;
            }
            // Check if the source row has any semi-transparent pixels.
            // For fully-opaque rows, use bulk memcpy.  For rows with alpha,
            // fall back to per-pixel blending.
            let src_row = &src[src_start..src_end];
            let all_opaque = src_row.chunks_exact(4).all(|px| px[3] == 255);
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
                        row_slice[di + 3] = (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
                    }
                }
            }
        }
        return;
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

/// Fixed-point alpha blend: `(src * alpha + dst * (255 - alpha) + 128) / 255`
/// using the well-known `((x + (x >> 8)) >> 8)` fast approximation of `x / 255`.
#[allow(clippy::inline_always)]
#[inline(always)]
const fn blend_u8(src: u8, dst: u8, alpha: u16) -> u8 {
    let inv = 255 - alpha;
    let val = src as u16 * alpha + dst as u16 * inv + 128;
    ((val + (val >> 8)) >> 8) as u8
}

// ── SSE2 alpha-blend helpers (x86-64) ──────────────────────────────────────
//
// Process 4 RGBA pixels at a time using SSE2 integer arithmetic.
// Source pixels are gathered (non-contiguous via x_map), destination pixels
// are contiguous.  The blend formula is identical to the scalar `blend_u8`:
//   result = ((src*alpha + dst*(255-alpha) + 128) + ((…) >> 8)) >> 8
//
// For the alpha channel we set source-alpha to 255 before blending so that
// `blend_u8(255, dst_alpha, src_alpha)` naturally computes the standard
// over-composite alpha `a_src + a_dst*(1-a_src)` (within ±1 of the scalar
// approximation — both are approximate divisions by 255).

/// Read 4 bytes from `src` at `offset` as a native-endian `u32`.
///
/// # Safety
///
/// Caller must ensure `offset + 3 < src.len()`.
#[inline(always)]
unsafe fn read_rgba_u32(src: &[u8], offset: usize) -> u32 {
    std::ptr::read_unaligned(src.as_ptr().add(offset) as *const u32)
}

/// Blend 4 gathered source RGBA pixels onto 4 contiguous destination pixels
/// using SSE2 "over" compositing (no opacity modifier).
///
/// # Safety
///
/// `dst_ptr` must point to at least 16 writable bytes.  Source pixel values
/// in `src_pixels` must be valid RGBA `u32` values.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn blend_4px_over_sse2(dst_ptr: *mut u8, src_pixels: [u32; 4]) {
    use std::arch::x86_64::*;

    let zero = _mm_setzero_si128();
    let c255 = _mm_set1_epi16(255);
    let c128 = _mm_set1_epi16(128);

    // Assemble 4 gathered source pixels into one register.
    let src4 = _mm_set_epi32(
        src_pixels[3] as i32,
        src_pixels[2] as i32,
        src_pixels[1] as i32,
        src_pixels[0] as i32,
    );

    // Mask with 0xFF at each pixel's alpha-byte position (bytes 3,7,11,15).
    let alpha_byte_mask = _mm_set1_epi32(0xFF00_0000_u32 as i32);

    // Fast path: all 4 source pixels fully opaque → direct copy.
    let alpha_bytes = _mm_and_si128(src4, alpha_byte_mask);
    if _mm_movemask_epi8(_mm_cmpeq_epi8(alpha_bytes, alpha_byte_mask)) == 0xFFFF {
        _mm_storeu_si128(dst_ptr as *mut __m128i, src4);
        return;
    }

    // Fast path: all 4 source pixels fully transparent → nothing to do.
    if _mm_movemask_epi8(_mm_cmpeq_epi8(alpha_bytes, zero)) == 0xFFFF {
        return;
    }

    let dst4 = _mm_loadu_si128(dst_ptr as *const __m128i);

    // Replace source alpha channel with 255 for correct composite-alpha
    // via blend_u8(255, dst_alpha, src_alpha).
    let src_blend = _mm_or_si128(src4, alpha_byte_mask);

    // --- Low 2 pixels (u16 arithmetic) ---
    let src_lo = _mm_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm_unpacklo_epi8(dst4, zero);

    // Extract original source alpha and broadcast within each 4-u16 pixel group.
    let src_orig_lo = _mm_unpacklo_epi8(src4, zero);
    // _MM_SHUFFLE(3,3,3,3) = 0xFF → replicate element 3 (alpha) to all 4 positions.
    let alpha_lo = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);

    let inv_alpha_lo = _mm_sub_epi16(c255, alpha_lo);
    let val_lo = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_lo, alpha_lo), _mm_mullo_epi16(dst_lo, inv_alpha_lo)),
        c128,
    );
    let result_lo = _mm_srli_epi16(_mm_add_epi16(val_lo, _mm_srli_epi16(val_lo, 8)), 8);

    // --- High 2 pixels ---
    let src_hi = _mm_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm_unpackhi_epi8(dst4, zero);
    let src_orig_hi = _mm_unpackhi_epi8(src4, zero);
    let alpha_hi = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);

    let inv_alpha_hi = _mm_sub_epi16(c255, alpha_hi);
    let val_hi = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_hi, alpha_hi), _mm_mullo_epi16(dst_hi, inv_alpha_hi)),
        c128,
    );
    let result_hi = _mm_srli_epi16(_mm_add_epi16(val_hi, _mm_srli_epi16(val_hi, 8)), 8);

    // Pack back to u8 and store.
    _mm_storeu_si128(dst_ptr as *mut __m128i, _mm_packus_epi16(result_lo, result_hi));
}

/// Blend 4 gathered source RGBA pixels onto 4 contiguous destination pixels
/// using SSE2 "over" compositing **with** an opacity multiplier applied to
/// each pixel's source alpha.
///
/// # Safety
///
/// `dst_ptr` must point to at least 16 writable bytes.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn blend_4px_over_alpha_sse2(dst_ptr: *mut u8, src_pixels: [u32; 4], opacity: u16) {
    use std::arch::x86_64::*;

    let zero = _mm_setzero_si128();
    let c255 = _mm_set1_epi16(255);
    let c128 = _mm_set1_epi16(128);
    let opacity_v = _mm_set1_epi16(opacity as i16);

    let src4 = _mm_set_epi32(
        src_pixels[3] as i32,
        src_pixels[2] as i32,
        src_pixels[1] as i32,
        src_pixels[0] as i32,
    );

    let dst4 = _mm_loadu_si128(dst_ptr as *const __m128i);
    let alpha_byte_mask = _mm_set1_epi32(0xFF00_0000_u32 as i32);
    let src_blend = _mm_or_si128(src4, alpha_byte_mask);

    // --- Low 2 pixels ---
    let src_lo = _mm_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm_unpacklo_epi8(dst4, zero);

    // Extract original alpha, apply opacity: sa_eff = (sa * opacity + 128) >> 8.
    // Max value: (255*255+128)>>8 = 254, so no clamping needed.
    let src_orig_lo = _mm_unpacklo_epi8(src4, zero);
    let raw_alpha_lo = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);
    let alpha_lo = _mm_srli_epi16(_mm_add_epi16(_mm_mullo_epi16(raw_alpha_lo, opacity_v), c128), 8);

    let inv_alpha_lo = _mm_sub_epi16(c255, alpha_lo);
    let val_lo = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_lo, alpha_lo), _mm_mullo_epi16(dst_lo, inv_alpha_lo)),
        c128,
    );
    let result_lo = _mm_srli_epi16(_mm_add_epi16(val_lo, _mm_srli_epi16(val_lo, 8)), 8);

    // --- High 2 pixels ---
    let src_hi = _mm_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm_unpackhi_epi8(dst4, zero);
    let src_orig_hi = _mm_unpackhi_epi8(src4, zero);
    let raw_alpha_hi = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);
    let alpha_hi = _mm_srli_epi16(_mm_add_epi16(_mm_mullo_epi16(raw_alpha_hi, opacity_v), c128), 8);

    let inv_alpha_hi = _mm_sub_epi16(c255, alpha_hi);
    let val_hi = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_hi, alpha_hi), _mm_mullo_epi16(dst_hi, inv_alpha_hi)),
        c128,
    );
    let result_hi = _mm_srli_epi16(_mm_add_epi16(val_hi, _mm_srli_epi16(val_hi, 8)), 8);

    _mm_storeu_si128(dst_ptr as *mut __m128i, _mm_packus_epi16(result_lo, result_hi));
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
    clippy::needless_range_loop
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

    // ── SSE2 fast path: process 4 pixels at a time ─────────────────────
    #[cfg(target_arch = "x86_64")]
    {
        // Pre-validate bounds so the inner SIMD loop is branch-free.
        let src_row_end = src_row_base + sw * 4;
        let dst_end = (rx + effective_rw) * 4;
        if src_row_end <= src.len() && dst_end <= row_slice.len() {
            let chunks = effective_rw / 4;
            for c in 0..chunks {
                let dx = c * 4;
                // SAFETY: bounds pre-validated above; x_map values < sw;
                // dst range (rx+dx)*4..(rx+dx+4)*4 < dst_end <= row_slice.len().
                unsafe {
                    let pixels = [
                        read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                    ];
                    blend_4px_over_sse2(row_slice.as_mut_ptr().add((rx + dx) * 4), pixels);
                }
            }

            // Scalar tail for remaining 0-3 pixels.
            let tail_start = chunks * 4;
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
    clippy::needless_range_loop
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

    // ── SSE2 fast path ─────────────────────────────────────────────────
    #[cfg(target_arch = "x86_64")]
    {
        let src_row_end = src_row_base + sw * 4;
        let dst_end = (rx + effective_rw) * 4;
        if src_row_end <= src.len() && dst_end <= row_slice.len() {
            let chunks = effective_rw / 4;
            for c in 0..chunks {
                let dx = c * 4;
                unsafe {
                    let pixels = [
                        read_rgba_u32(src, src_row_base + x_map[dx] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 1] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 2] * 4),
                        read_rgba_u32(src, src_row_base + x_map[dx + 3] * 4),
                    ];
                    blend_4px_over_alpha_sse2(
                        row_slice.as_mut_ptr().add((rx + dx) * 4),
                        pixels,
                        opacity_u16,
                    );
                }
            }

            // Scalar tail.
            let tail_start = chunks * 4;
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
/// The source image is uniformly scaled to fit inside the destination rect
/// while preserving its aspect ratio (like CSS `object-fit: contain`).  When
/// the aspect ratios differ the image is centred and any padding is left
/// transparent.  This avoids the visual distortion that a naive stretch
/// would cause on rotated layers.
///
/// Uses inverse-affine mapping with nearest-neighbor sampling.  Edge pixels
/// receive fractional alpha coverage computed from the signed distance to each
/// of the four content edges in the un-rotated local coordinate system.  This
/// eliminates the staircase aliasing that a hard binary inside/outside test
/// would produce.
///
/// Falls back to [`scale_blit_rgba`] when `rotation_deg` is effectively zero.
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

    // ── Aspect-ratio-preserving fit ─────────────────────────────────────
    // Instead of stretching the source to fill the destination rect (which
    // distorts the image when rotated), compute a uniform scale that fits
    // the source within the rect (like CSS `object-fit: contain`) and
    // centre the result.  Pixels in the letterbox / pillarbox padding are
    // transparent.
    let sw_f = src_width as f32;
    let sh_f = src_height as f32;
    let fit_scale = (rw / sw_f).min(rh / sh_f);
    let content_w = sw_f * fit_scale; // actual content width  inside the rect
    let content_h = sh_f * fit_scale; // actual content height inside the rect
    let half_cw = content_w * 0.5;
    let half_ch = content_h * 0.5;

    // Rotation centre = centre of the destination rect.
    let cx = rw.mul_add(0.5, dst_rect.x as f32);
    let cy = rh.mul_add(0.5, dst_rect.y as f32);

    // Pre-compute sin/cos for the *inverse* rotation (clockwise rotation
    // means we apply counter-clockwise to map destination → source).
    let angle_rad = rotation_deg.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

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

    // Reciprocal of the fit scale for mapping content-local coords back to
    // source pixel coords.  Pre-computed to replace per-pixel divisions
    // with multiplies.
    let inv_fit_scale = 1.0 / fit_scale;

    // Per-row closure that processes all columns in a single row of the
    // bounding box.  Uses an incremental stepper: since `dx_f` increments
    // by 1.0 each column, `local_x` and `local_y` change by `+cos_a` and
    // `-sin_a` respectively — replacing 2 multiplies with 2 adds per pixel.
    //
    // Edge anti-aliasing distances are computed against the *content*
    // boundary (`half_cw` × `half_ch`), not the full rect, so the visible
    // edge matches the actual image boundary.
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

            // Map from content-local coords to source pixel coords.
            // `local_x/y` ∈ [-half_cw, half_cw] × [-half_ch, half_ch]
            // for points inside the content area.  Convert to source
            // pixel space via the inverse of the uniform fit scale.
            let src_fx = (local_x + half_cw) * inv_fit_scale;
            let src_fy = (local_y + half_ch) * inv_fit_scale;

            let sx = (src_fx as usize).min(sw - 1);
            let sy = (src_fy as usize).min(sh - 1);

            let src_idx = (sy * sw + sx) * 4;
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
                    // Advance stepper and process interior pixels.
                    for _ in 0..skip {
                        local_x += cos_a;
                        local_y -= sin_a;
                        px += 1;

                        // Source lookup (incremental local coords).
                        let isx = (((local_x + half_cw) * inv_fit_scale) as usize).min(sw - 1);
                        let isy = (((local_y + half_ch) * inv_fit_scale) as usize).min(sh - 1);
                        let si = (isy * sw + isx) * 4;
                        if si + 3 >= src.len() {
                            continue;
                        }

                        let ir = src[si];
                        let ig = src[si + 1];
                        let ib = src[si + 2];
                        let mut ia = src[si + 3];

                        if opacity_u16 < 256 {
                            ia = ((u16::from(ia) * opacity_u16 + 128) >> 8).min(255) as u8;
                        }
                        // No edge coverage — interior pixel.
                        if ia > 0 {
                            let doff = px as usize * 4;
                            if doff + 3 < row_slice.len() {
                                if ia == 255 {
                                    row_slice[doff] = ir;
                                    row_slice[doff + 1] = ig;
                                    row_slice[doff + 2] = ib;
                                    row_slice[doff + 3] = 255;
                                } else {
                                    let a16 = u16::from(ia);
                                    row_slice[doff] = blend_u8(ir, row_slice[doff], a16);
                                    row_slice[doff + 1] = blend_u8(ig, row_slice[doff + 1], a16);
                                    row_slice[doff + 2] = blend_u8(ib, row_slice[doff + 2], a16);
                                    let da = u16::from(row_slice[doff + 3]);
                                    row_slice[doff + 3] =
                                        (a16 + ((da * (255 - a16) + 128) >> 8)).min(255) as u8;
                                }
                            }
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

// ── SIMD helpers ──────────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
mod simd {
    //! SSE2 and AVX2 kernels for I420↔RGBA8 colour-space conversion.
    //!
    //! Each function processes a fixed number of pixels per iteration and
    //! returns the number of pixels it fully handled so the caller can fall
    //! back to scalar for any tail pixels.

    // ── I420 → RGBA8 (SSE2: 4 pixels / iter, i32 arithmetic) ──────────

    /// Convert up to `width` I420 pixels from one row to RGBA8 using SSE2.
    ///
    /// Returns the number of pixels converted (always a multiple of 4).
    /// The caller must handle the remaining `width - returned` tail pixels
    /// with the scalar path.
    ///
    /// Uses 32-bit arithmetic throughout to avoid i16 overflow with the
    /// BT.601 coefficients (298, 409, 516 exceed i16 range when multiplied
    /// by typical Y/U/V values).
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn i420_to_rgba8_row_sse2(
        y_row: &[u8],
        u_row: &[u8],
        v_row: &[u8],
        rgba_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi32,
            _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_storeu_si128,
            _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
        };

        let simd_width = width & !3; // round down to multiple of 4
        if simd_width == 0 {
            return 0;
        }

        let coeff_298 = _mm_set1_epi32(298);
        let coeff_409 = _mm_set1_epi32(409);
        let coeff_n100 = _mm_set1_epi32(-100);
        let coeff_n208 = _mm_set1_epi32(-208);
        let coeff_516 = _mm_set1_epi32(516);
        let bias_16 = _mm_set1_epi32(16);
        let bias_128 = _mm_set1_epi32(128);
        let rounding = _mm_set1_epi32(128);
        let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
        let zero = _mm_setzero_si128();

        let mut col = 0usize;
        while col < simd_width {
            // Load 4 Y values and widen to i32.
            let y0 = i32::from(y_row[col]);
            let y1 = i32::from(y_row[col + 1]);
            let y2 = i32::from(y_row[col + 2]);
            let y3 = i32::from(y_row[col + 3]);
            let y32 = _mm_set_epi32(y3, y2, y1, y0);

            // Load 2 U and 2 V values, duplicate each for 4 luma pixels.
            let chroma_col = col / 2;
            let u0 = i32::from(u_row[chroma_col]);
            let u1 = i32::from(u_row[chroma_col + 1]);
            let v0 = i32::from(v_row[chroma_col]);
            let v1 = i32::from(v_row[chroma_col + 1]);
            let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
            let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

            // c = Y - 16, d = U - 128, e = V - 128  (all i32, no overflow)
            let c = _mm_sub_epi32(y32, bias_16);
            let d = _mm_sub_epi32(u32x4, bias_128);
            let e = _mm_sub_epi32(v32x4, bias_128);

            // R = (298*c + 409*e + 128) >> 8   (i32 multiply via _mm_mullo_epi16
            //     works here because _mm_mullo_epi16 on 32-bit lanes gives the
            //     low 16 bits — we need the SSE2-compatible i32 multiply instead).
            //
            // SSE2 lacks _mm_mullo_epi32, so we use a helper: multiply pairs of
            // 16-bit values and accumulate to i32 via _mm_madd_epi16 with one
            // operand being the value and the other a [coeff, 0] pattern.
            // Simpler: since our values fit i16 (c ∈ [-16,239], d/e ∈ [-128,127])
            // but coefficients don't, we swap: put values in i16 slots and use
            // _mm_madd_epi16 with [coeff, 0] to get i32 products.

            // Actually, the simplest SSE2 i32 multiply is via two _mm_mul_epu32
            // calls (even/odd lanes) then re-interleave. Let's use that approach.

            let r32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_409, e)),
                    rounding,
                ),
                8,
            );

            let g32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_n100, d)),
                        mul32_sse2(coeff_n208, e),
                    ),
                    rounding,
                ),
                8,
            );

            let b32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_516, d)),
                    rounding,
                ),
                8,
            );

            // Pack i32 → i16 (signed saturate) → u8 (unsigned saturate).
            // packs_epi32 packs 4+4 i32 → 8 i16; packus_epi16 packs 8+8 i16 → 16 u8.
            let r16 = _mm_packs_epi32(r32, zero); // 4 values in low half
            let g16 = _mm_packs_epi32(g32, zero);
            let b16 = _mm_packs_epi32(b32, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            // Interleave to RGBA: [R0,G0,B0,A0, R1,G1,B1,A1, ...]
            let rg = _mm_unpacklo_epi8(r8, g8); // [R0,G0,R1,G1,...]
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1)); // [B0,FF,B1,FF,...]
            let rgba = _mm_unpacklo_epi16(rg, ba); // 4 RGBA pixels
            let rgba = _mm_or_si128(rgba, alpha_mask);

            // Store 4 RGBA pixels (16 bytes).
            let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
            _mm_storeu_si128(out_ptr.cast(), rgba);

            col += 4;
        }
        simd_width
    }

    /// SSE2-compatible signed 32-bit multiply (low 32 bits of each lane).
    ///
    /// SSE2 only has `_mm_mul_epu32` which multiplies lanes 0 and 2 as
    /// unsigned 32-bit → 64-bit.  We use it twice (even + odd lanes) and
    /// re-interleave to get all four i32 products.  The unsigned multiply
    /// gives the correct low-32 result for signed operands (two's complement).
    #[target_feature(enable = "sse2")]
    #[inline]
    unsafe fn mul32_sse2(
        a: std::arch::x86_64::__m128i,
        b: std::arch::x86_64::__m128i,
    ) -> std::arch::x86_64::__m128i {
        use std::arch::x86_64::{_mm_mul_epu32, _mm_shuffle_epi32, _mm_unpacklo_epi32};
        // Multiply even lanes (0, 2) → 64-bit results.
        let even = _mm_mul_epu32(a, b);
        // Shuffle odd lanes (1, 3) into even positions, then multiply.
        let odd =
            _mm_mul_epu32(_mm_shuffle_epi32(a, 0b11_11_01_01), _mm_shuffle_epi32(b, 0b11_11_01_01));
        // Extract the low 32 bits of each 64-bit product:
        //   even = [p0_lo, p0_hi, p2_lo, p2_hi]
        //   odd  = [p1_lo, p1_hi, p3_lo, p3_hi]
        // shuffle 0b00_00_10_00 picks dwords 0 and 2 → [p_lo0, p_lo2, ?, ?]
        let even_lo = _mm_shuffle_epi32(even, 0b00_00_10_00);
        let odd_lo = _mm_shuffle_epi32(odd, 0b00_00_10_00);
        // Interleave low halves → [p0, p1, p2, p3].
        _mm_unpacklo_epi32(even_lo, odd_lo)
    }

    // ── I420 → RGBA8 SSE4.1 variant ──────────────────────────────────

    /// Convert up to `width` I420 pixels from one row to RGBA8 using SSE4.1.
    ///
    /// Same logic as [`i420_to_rgba8_row_sse2`] but uses `_mm_mullo_epi32`
    /// for native i32 multiply instead of the 7-instruction `mul32_sse2`
    /// emulation.
    ///
    /// Returns the number of pixels converted (always a multiple of 4).
    #[target_feature(enable = "sse4.1")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn i420_to_rgba8_row_sse41(
        y_row: &[u8],
        u_row: &[u8],
        v_row: &[u8],
        rgba_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_mullo_epi32, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16,
            _mm_set1_epi32, _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32,
            _mm_storeu_si128, _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
        };

        let simd_width = width & !3;
        if simd_width == 0 {
            return 0;
        }

        let coeff_298 = _mm_set1_epi32(298);
        let coeff_409 = _mm_set1_epi32(409);
        let coeff_n100 = _mm_set1_epi32(-100);
        let coeff_n208 = _mm_set1_epi32(-208);
        let coeff_516 = _mm_set1_epi32(516);
        let bias_16 = _mm_set1_epi32(16);
        let bias_128 = _mm_set1_epi32(128);
        let rounding = _mm_set1_epi32(128);
        let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
        let zero = _mm_setzero_si128();

        let mut col = 0usize;
        while col < simd_width {
            let y0 = i32::from(y_row[col]);
            let y1 = i32::from(y_row[col + 1]);
            let y2 = i32::from(y_row[col + 2]);
            let y3 = i32::from(y_row[col + 3]);
            let y32 = _mm_set_epi32(y3, y2, y1, y0);

            let chroma_col = col / 2;
            let u0 = i32::from(u_row[chroma_col]);
            let u1 = i32::from(u_row[chroma_col + 1]);
            let v0 = i32::from(v_row[chroma_col]);
            let v1 = i32::from(v_row[chroma_col + 1]);
            let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
            let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

            let c = _mm_sub_epi32(y32, bias_16);
            let d = _mm_sub_epi32(u32x4, bias_128);
            let e = _mm_sub_epi32(v32x4, bias_128);

            let r32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(_mm_mullo_epi32(coeff_298, c), _mm_mullo_epi32(coeff_409, e)),
                    rounding,
                ),
                8,
            );

            let g32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_mullo_epi32(coeff_298, c),
                            _mm_mullo_epi32(coeff_n100, d),
                        ),
                        _mm_mullo_epi32(coeff_n208, e),
                    ),
                    rounding,
                ),
                8,
            );

            let b32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(_mm_mullo_epi32(coeff_298, c), _mm_mullo_epi32(coeff_516, d)),
                    rounding,
                ),
                8,
            );

            let r16 = _mm_packs_epi32(r32, zero);
            let g16 = _mm_packs_epi32(g32, zero);
            let b16 = _mm_packs_epi32(b32, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            let rg = _mm_unpacklo_epi8(r8, g8);
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
            let rgba = _mm_unpacklo_epi16(rg, ba);
            let rgba = _mm_or_si128(rgba, alpha_mask);

            let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
            _mm_storeu_si128(out_ptr.cast(), rgba);

            col += 4;
        }
        simd_width
    }

    // ── NV12 → RGBA8 (SSE2/SSE4.1: 4 pixels / iter, i32 arithmetic) ──

    /// Convert up to `width` NV12 pixels from one row to RGBA8 using SSE4.1.
    ///
    /// Same logic as [`nv12_to_rgba8_row_sse2`] but uses `_mm_mullo_epi32`
    /// for native i32 multiply instead of the 7-instruction `mul32_sse2`
    /// emulation.  Falls back to SSE2 on older hardware.
    ///
    /// Returns the number of pixels converted (always a multiple of 4).
    #[target_feature(enable = "sse4.1")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn nv12_to_rgba8_row_sse41(
        y_row: &[u8],
        uv_row: &[u8],
        rgba_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_mullo_epi32, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16,
            _mm_set1_epi32, _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32,
            _mm_storeu_si128, _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
        };

        let simd_width = width & !3;
        if simd_width == 0 {
            return 0;
        }

        let coeff_298 = _mm_set1_epi32(298);
        let coeff_409 = _mm_set1_epi32(409);
        let coeff_n100 = _mm_set1_epi32(-100);
        let coeff_n208 = _mm_set1_epi32(-208);
        let coeff_516 = _mm_set1_epi32(516);
        let bias_16 = _mm_set1_epi32(16);
        let bias_128 = _mm_set1_epi32(128);
        let rounding = _mm_set1_epi32(128);
        let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
        let zero = _mm_setzero_si128();

        let mut col = 0usize;
        while col < simd_width {
            let y0 = i32::from(y_row[col]);
            let y1 = i32::from(y_row[col + 1]);
            let y2 = i32::from(y_row[col + 2]);
            let y3 = i32::from(y_row[col + 3]);
            let y32 = _mm_set_epi32(y3, y2, y1, y0);

            let chroma_byte = (col / 2) * 2;
            let u0 = i32::from(uv_row[chroma_byte]);
            let v0 = i32::from(uv_row[chroma_byte + 1]);
            let u1 = i32::from(uv_row[chroma_byte + 2]);
            let v1 = i32::from(uv_row[chroma_byte + 3]);
            let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
            let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

            let c = _mm_sub_epi32(y32, bias_16);
            let d = _mm_sub_epi32(u32x4, bias_128);
            let e = _mm_sub_epi32(v32x4, bias_128);

            let r32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(_mm_mullo_epi32(coeff_298, c), _mm_mullo_epi32(coeff_409, e)),
                    rounding,
                ),
                8,
            );

            let g32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_mullo_epi32(coeff_298, c),
                            _mm_mullo_epi32(coeff_n100, d),
                        ),
                        _mm_mullo_epi32(coeff_n208, e),
                    ),
                    rounding,
                ),
                8,
            );

            let b32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(_mm_mullo_epi32(coeff_298, c), _mm_mullo_epi32(coeff_516, d)),
                    rounding,
                ),
                8,
            );

            let r16 = _mm_packs_epi32(r32, zero);
            let g16 = _mm_packs_epi32(g32, zero);
            let b16 = _mm_packs_epi32(b32, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            let rg = _mm_unpacklo_epi8(r8, g8);
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
            let rgba = _mm_unpacklo_epi16(rg, ba);
            let rgba = _mm_or_si128(rgba, alpha_mask);

            let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
            _mm_storeu_si128(out_ptr.cast(), rgba);

            col += 4;
        }
        simd_width
    }

    /// Convert up to `width` NV12 pixels from one row to RGBA8 using SSE2.
    ///
    /// Reads luma from `y_row` and chroma from interleaved `uv_row`
    /// (`[U0, V0, U1, V1, …]`) directly — no deinterleaving scratch
    /// buffers required.  Same BT.601 math as [`i420_to_rgba8_row_sse2`].
    ///
    /// Returns the number of pixels converted (always a multiple of 4).
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn nv12_to_rgba8_row_sse2(
        y_row: &[u8],
        uv_row: &[u8],
        rgba_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi32,
            _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_storeu_si128,
            _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
        };

        let simd_width = width & !3; // round down to multiple of 4
        if simd_width == 0 {
            return 0;
        }

        let coeff_298 = _mm_set1_epi32(298);
        let coeff_409 = _mm_set1_epi32(409);
        let coeff_n100 = _mm_set1_epi32(-100);
        let coeff_n208 = _mm_set1_epi32(-208);
        let coeff_516 = _mm_set1_epi32(516);
        let bias_16 = _mm_set1_epi32(16);
        let bias_128 = _mm_set1_epi32(128);
        let rounding = _mm_set1_epi32(128);
        let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
        let zero = _mm_setzero_si128();

        let mut col = 0usize;
        while col < simd_width {
            // Load 4 Y values and widen to i32.
            let y0 = i32::from(y_row[col]);
            let y1 = i32::from(y_row[col + 1]);
            let y2 = i32::from(y_row[col + 2]);
            let y3 = i32::from(y_row[col + 3]);
            let y32 = _mm_set_epi32(y3, y2, y1, y0);

            // Read 2 UV pairs directly from the interleaved NV12 plane.
            // uv_row layout: [U0, V0, U1, V1, U2, V2, …]
            let chroma_byte = (col / 2) * 2; // byte offset into uv_row
            let u0 = i32::from(uv_row[chroma_byte]);
            let v0 = i32::from(uv_row[chroma_byte + 1]);
            let u1 = i32::from(uv_row[chroma_byte + 2]);
            let v1 = i32::from(uv_row[chroma_byte + 3]);
            let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
            let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

            // c = Y - 16, d = U - 128, e = V - 128
            let c = _mm_sub_epi32(y32, bias_16);
            let d = _mm_sub_epi32(u32x4, bias_128);
            let e = _mm_sub_epi32(v32x4, bias_128);

            let r32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_409, e)),
                    rounding,
                ),
                8,
            );

            let g32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_n100, d)),
                        mul32_sse2(coeff_n208, e),
                    ),
                    rounding,
                ),
                8,
            );

            let b32 = _mm_srai_epi32(
                _mm_add_epi32(
                    _mm_add_epi32(mul32_sse2(coeff_298, c), mul32_sse2(coeff_516, d)),
                    rounding,
                ),
                8,
            );

            // Pack i32 → i16 → u8, interleave to RGBA.
            let r16 = _mm_packs_epi32(r32, zero);
            let g16 = _mm_packs_epi32(g32, zero);
            let b16 = _mm_packs_epi32(b32, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            let rg = _mm_unpacklo_epi8(r8, g8);
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
            let rgba = _mm_unpacklo_epi16(rg, ba);
            let rgba = _mm_or_si128(rgba, alpha_mask);

            let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
            _mm_storeu_si128(out_ptr.cast(), rgba);

            col += 4;
        }
        simd_width
    }

    // ── NV12 → RGBA8 (AVX2: 8 pixels / iter, i32 arithmetic) ──────────

    /// Convert up to `width` NV12 pixels from one row to RGBA8 using AVX2.
    ///
    /// Processes 8 pixels per iteration (256-bit registers) — double the
    /// throughput of the SSE4.1 variant.  The heavy i32 multiplies run in
    /// 256-bit lanes while the final u8 pack + RGBA interleave drops to
    /// 128-bit SSE to avoid AVX2 lane-crossing headaches.
    ///
    /// Returns the number of pixels converted (always a multiple of 8).
    #[target_feature(enable = "avx2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn nv12_to_rgba8_row_avx2(
        y_row: &[u8],
        uv_row: &[u8],
        rgba_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm256_add_epi32, _mm256_castsi256_si128, _mm256_cvtepu8_epi32,
            _mm256_extracti128_si256, _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_srai_epi32,
            _mm256_sub_epi32, _mm_loadl_epi64, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16,
            _mm_set1_epi32, _mm_set1_epi8, _mm_set_epi8, _mm_setzero_si128, _mm_shuffle_epi8,
            _mm_storeu_si128, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
        };

        let simd_width = width & !7; // round down to multiple of 8
        if simd_width == 0 {
            return 0;
        }

        let coeff_298 = _mm256_set1_epi32(298);
        let coeff_409 = _mm256_set1_epi32(409);
        let coeff_n100 = _mm256_set1_epi32(-100);
        let coeff_n208 = _mm256_set1_epi32(-208);
        let coeff_516 = _mm256_set1_epi32(516);
        let bias_16 = _mm256_set1_epi32(16);
        let bias_128 = _mm256_set1_epi32(128);
        let rounding = _mm256_set1_epi32(128);
        let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
        let zero = _mm_setzero_si128();

        // Shuffle controls for deinterleaving + duplicating NV12 UV pairs.
        // Input:  [U0,V0, U1,V1, U2,V2, U3,V3] (low 8 bytes)
        // U out:  [U0,U0, U1,U1, U2,U2, U3,U3]
        // V out:  [V0,V0, V1,V1, V2,V2, V3,V3]
        let u_shuf = _mm_set_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 6, 6, 4, 4, 2, 2, 0, 0);
        let v_shuf = _mm_set_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 7, 7, 5, 5, 3, 3, 1, 1);

        let mut col = 0usize;
        while col < simd_width {
            // Load 8 Y values and zero-extend u8→i32.
            let y8 = _mm_loadl_epi64(y_row.as_ptr().add(col).cast());
            let y32 = _mm256_cvtepu8_epi32(y8);

            // Load 4 interleaved UV pairs, deinterleave + duplicate for 8 pixels.
            let chroma_byte = (col / 2) * 2;
            let uv8 = _mm_loadl_epi64(uv_row.as_ptr().add(chroma_byte).cast());
            let u32x8 = _mm256_cvtepu8_epi32(_mm_shuffle_epi8(uv8, u_shuf));
            let v32x8 = _mm256_cvtepu8_epi32(_mm_shuffle_epi8(uv8, v_shuf));

            // c = Y - 16, d = U - 128, e = V - 128
            let c = _mm256_sub_epi32(y32, bias_16);
            let d = _mm256_sub_epi32(u32x8, bias_128);
            let e = _mm256_sub_epi32(v32x8, bias_128);

            // R = (298*c + 409*e + 128) >> 8
            let r32 = _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_mullo_epi32(coeff_298, c),
                        _mm256_mullo_epi32(coeff_409, e),
                    ),
                    rounding,
                ),
                8,
            );

            // G = (298*c - 100*d - 208*e + 128) >> 8
            let g32 = _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_add_epi32(
                            _mm256_mullo_epi32(coeff_298, c),
                            _mm256_mullo_epi32(coeff_n100, d),
                        ),
                        _mm256_mullo_epi32(coeff_n208, e),
                    ),
                    rounding,
                ),
                8,
            );

            // B = (298*c + 516*d + 128) >> 8
            let b32 = _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_mullo_epi32(coeff_298, c),
                        _mm256_mullo_epi32(coeff_516, d),
                    ),
                    rounding,
                ),
                8,
            );

            // ── Pack + interleave: split into two 4-pixel halves ──────
            // Drop to 128-bit SSE for pack/interleave to sidestep AVX2
            // lane-crossing issues in packs_epi32 / packus_epi16.
            let r_lo = _mm256_castsi256_si128(r32);
            let r_hi = _mm256_extracti128_si256(r32, 1);
            let g_lo = _mm256_castsi256_si128(g32);
            let g_hi = _mm256_extracti128_si256(g32, 1);
            let b_lo = _mm256_castsi256_si128(b32);
            let b_hi = _mm256_extracti128_si256(b32, 1);

            // Pixels 0–3
            let r16 = _mm_packs_epi32(r_lo, zero);
            let g16 = _mm_packs_epi32(g_lo, zero);
            let b16 = _mm_packs_epi32(b_lo, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            let rg = _mm_unpacklo_epi8(r8, g8);
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
            let rgba = _mm_unpacklo_epi16(rg, ba);
            let rgba = _mm_or_si128(rgba, alpha_mask);
            _mm_storeu_si128(rgba_out.as_mut_ptr().add(col * 4).cast(), rgba);

            // Pixels 4–7
            let r16 = _mm_packs_epi32(r_hi, zero);
            let g16 = _mm_packs_epi32(g_hi, zero);
            let b16 = _mm_packs_epi32(b_hi, zero);
            let r8 = _mm_packus_epi16(r16, zero);
            let g8 = _mm_packus_epi16(g16, zero);
            let b8 = _mm_packus_epi16(b16, zero);

            let rg = _mm_unpacklo_epi8(r8, g8);
            let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
            let rgba = _mm_unpacklo_epi16(rg, ba);
            let rgba = _mm_or_si128(rgba, alpha_mask);
            _mm_storeu_si128(rgba_out.as_mut_ptr().add((col + 4) * 4).cast(), rgba);

            col += 8;
        }
        simd_width
    }

    // ── RGBA8 → I420 Y-plane (SSE2/SSE4.1: 4 pixels / iter) ───────────

    /// Convert one row of RGBA8 pixels to Y values using SSE4.1.
    ///
    /// Same logic as [`rgba8_to_y_row_sse2`] but uses native i32 multiply.
    #[target_feature(enable = "sse4.1")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) unsafe fn rgba8_to_y_row_sse41(
        rgba_row: &[u8],
        y_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi32, _mm_packs_epi32,
            _mm_packus_epi16, _mm_set1_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_srli_epi32,
            _mm_storeu_si32,
        };

        let simd_width = width & !3;
        if simd_width == 0 {
            return 0;
        }

        let coeff_66 = _mm_set1_epi32(66);
        let coeff_129 = _mm_set1_epi32(129);
        let coeff_25 = _mm_set1_epi32(25);
        let rounding = _mm_set1_epi32(128);
        let bias_16 = _mm_set1_epi32(16);
        let zero = _mm_setzero_si128();
        let channel_mask = _mm_set1_epi32(0xFF);

        let mut col = 0usize;
        while col < simd_width {
            let src_ptr = rgba_row.as_ptr().add(col * 4);
            let px = _mm_loadu_si128(src_ptr.cast());

            let r = _mm_and_si128(px, channel_mask);
            let g = _mm_and_si128(_mm_srli_epi32(px, 8), channel_mask);
            let b = _mm_and_si128(_mm_srli_epi32(px, 16), channel_mask);

            let y32 = _mm_add_epi32(
                _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_add_epi32(
                                _mm_mullo_epi32(coeff_66, r),
                                _mm_mullo_epi32(coeff_129, g),
                            ),
                            _mm_mullo_epi32(coeff_25, b),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_16,
            );

            let y16 = _mm_packs_epi32(y32, zero);
            let y8 = _mm_packus_epi16(y16, zero);
            // Store 4 Y values via a single movd instruction.
            _mm_storeu_si32(y_out.as_mut_ptr().add(col).cast(), y8);

            col += 4;
        }
        simd_width
    }

    // ── RGBA8 → Y-plane (AVX2: 8 pixels / iter) ───────────────────────

    /// Convert one row of RGBA8 pixels to Y values using AVX2.
    ///
    /// Processes 8 pixels per iteration (256-bit registers) — double the
    /// throughput of the SSE4.1 variant.  Falls back gracefully: callers
    /// should check `is_x86_feature_detected!("avx2")` and use the
    /// SSE4.1/SSE2 kernels when AVX2 is unavailable.
    ///
    /// Returns the number of pixels converted (multiple of 8).
    #[target_feature(enable = "avx2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) unsafe fn rgba8_to_y_row_avx2(
        rgba_row: &[u8],
        y_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128, _mm256_extracti128_si256,
            _mm256_loadu_si256, _mm256_mullo_epi32, _mm256_packs_epi32, _mm256_packus_epi16,
            _mm256_set1_epi32, _mm256_setzero_si256, _mm256_srai_epi32, _mm256_srli_epi32,
            _mm_storel_epi64, _mm_unpacklo_epi32,
        };

        let simd_width = width & !7; // round down to multiple of 8
        if simd_width == 0 {
            return 0;
        }

        let coeff_66 = _mm256_set1_epi32(66);
        let coeff_129 = _mm256_set1_epi32(129);
        let coeff_25 = _mm256_set1_epi32(25);
        let rounding = _mm256_set1_epi32(128);
        let bias_16 = _mm256_set1_epi32(16);
        let zero = _mm256_setzero_si256();
        let channel_mask = _mm256_set1_epi32(0xFF);

        let mut col = 0usize;
        while col < simd_width {
            // Load 8 RGBA pixels (32 bytes = 256 bits).
            let src_ptr = rgba_row.as_ptr().add(col * 4);
            let px = _mm256_loadu_si256(src_ptr.cast());

            // Extract R, G, B channels from the 8 pixels.
            // AVX2 lane layout: pixels [0..3] in lane 0, [4..7] in lane 1.
            let r = _mm256_and_si256(px, channel_mask);
            let g = _mm256_and_si256(_mm256_srli_epi32(px, 8), channel_mask);
            let b = _mm256_and_si256(_mm256_srli_epi32(px, 16), channel_mask);

            // Y = ((66*R + 129*G + 25*B + 128) >> 8) + 16
            let y32 = _mm256_add_epi32(
                _mm256_srai_epi32(
                    _mm256_add_epi32(
                        _mm256_add_epi32(
                            _mm256_add_epi32(
                                _mm256_mullo_epi32(coeff_66, r),
                                _mm256_mullo_epi32(coeff_129, g),
                            ),
                            _mm256_mullo_epi32(coeff_25, b),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_16,
            );

            // Pack i32→i16→u8.  AVX2 pack ops work per 128-bit lane:
            //   packs_epi32:  lane0=[y0,y1,y2,y3, 0,0,0,0]  lane1=[y4,y5,y6,y7, 0,0,0,0]  (i16)
            //   packus_epi16: lane0=[y0,y1,y2,y3, 0..0]      lane1=[y4,y5,y6,y7, 0..0]     (u8)
            // Each lane has 4 Y bytes in the low dword.  Extract both halves
            // and interleave the low dwords to get [y0..y3, y4..y7] contiguous.
            let y16 = _mm256_packs_epi32(y32, zero);
            let y8 = _mm256_packus_epi16(y16, zero);
            let lo = _mm256_castsi256_si128(y8); // lane 0: [y0,y1,y2,y3, 0..0]
            let hi = _mm256_extracti128_si256(y8, 1); // lane 1: [y4,y5,y6,y7, 0..0]
            let combined = _mm_unpacklo_epi32(lo, hi); // [y0,y1,y2,y3, y4,y5,y6,y7, ...]

            // Store the low 8 bytes via a single movq instruction.
            _mm_storel_epi64(y_out.as_mut_ptr().add(col).cast(), combined);

            col += 8;
        }
        simd_width
    }

    /// Convert one row of RGBA8 pixels to Y values using SSE2.
    ///
    /// Returns the number of pixels converted (multiple of 4).
    ///
    /// Uses 32-bit arithmetic to avoid i16 overflow: coefficient 129
    /// multiplied by channel values up to 255 gives 32895, which exceeds
    /// the signed i16 maximum of 32767.
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) unsafe fn rgba8_to_y_row_sse2(
        rgba_row: &[u8],
        y_out: &mut [u8],
        width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_packs_epi32, _mm_packus_epi16,
            _mm_set1_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_srli_epi32, _mm_storeu_si32,
        };

        let simd_width = width & !3; // round down to multiple of 4
        if simd_width == 0 {
            return 0;
        }

        let coeff_66 = _mm_set1_epi32(66);
        let coeff_129 = _mm_set1_epi32(129);
        let coeff_25 = _mm_set1_epi32(25);
        let rounding = _mm_set1_epi32(128);
        let bias_16 = _mm_set1_epi32(16);
        let zero = _mm_setzero_si128();
        let channel_mask = _mm_set1_epi32(0xFF);

        let mut col = 0usize;
        while col < simd_width {
            // Load 4 RGBA pixels (16 bytes).
            let src_ptr = rgba_row.as_ptr().add(col * 4);
            let px = _mm_loadu_si128(src_ptr.cast());

            // Extract R, G, B channels as i32.
            let r = _mm_and_si128(px, channel_mask);
            let g = _mm_and_si128(_mm_srli_epi32(px, 8), channel_mask);
            let b = _mm_and_si128(_mm_srli_epi32(px, 16), channel_mask);

            // Y = ((66*R + 129*G + 25*B + 128) >> 8) + 16   (all i32)
            let y32 = _mm_add_epi32(
                _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_add_epi32(mul32_sse2(coeff_66, r), mul32_sse2(coeff_129, g)),
                            mul32_sse2(coeff_25, b),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_16,
            );

            // Pack i32 → i16 → u8 (saturating).
            let y16 = _mm_packs_epi32(y32, zero);
            let y8 = _mm_packus_epi16(y16, zero);
            // Store 4 Y values via a single movd instruction.
            _mm_storeu_si32(y_out.as_mut_ptr().add(col).cast(), y8);

            col += 4;
        }
        simd_width
    }

    // ── RGBA8 → I420 chroma row (SSE2: 8 chroma samples / iter) ──────────

    /// Convert one pair of RGBA8 rows to U and V chroma samples using SSE2.
    ///
    /// `rgba_row0` and `rgba_row1` are two consecutive rows of RGBA8 data.
    /// If the image has an odd height and this is the last chroma row,
    /// `rgba_row1` should equal `rgba_row0` (duplicate the last row).
    ///
    /// Returns the number of chroma samples converted (multiple of 8).
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn rgba8_to_chroma_row_sse2(
        rgba_row0: &[u8],
        rgba_row1: &[u8],
        u_out: &mut [u8],
        v_out: &mut [u8],
        chroma_width: usize,
        luma_width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi16, _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi16,
            _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32, _mm_set_epi16,
            _mm_setzero_si128, _mm_srai_epi16, _mm_srli_epi32, _mm_storeu_si32,
        };

        let simd_width = chroma_width & !3; // process 4 chroma samples (8 luma cols) at a time
        if simd_width == 0 || luma_width < 8 {
            return 0;
        }

        let coeff_cb_r = _mm_set1_epi16(-38);
        let coeff_cb_g = _mm_set1_epi16(-74);
        let coeff_cb_b = _mm_set1_epi16(112);
        let coeff_cr_r = _mm_set1_epi16(112);
        let coeff_cr_g = _mm_set1_epi16(-94);
        let coeff_cr_b = _mm_set1_epi16(-18);
        let rounding = _mm_set1_epi16(128);
        let bias_128 = _mm_set1_epi16(128);
        let zero = _mm_setzero_si128();
        let channel_mask = _mm_set1_epi32(0xFF);

        let mut ccol = 0usize;
        while ccol < simd_width {
            let luma_col = ccol * 2;
            if luma_col + 8 > luma_width {
                break;
            }

            // Load 8 pixels from row0 and row1.
            let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
            let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
            let px0_lo = _mm_loadu_si128(ptr0.cast());
            let px0_hi = _mm_loadu_si128(ptr0.add(16).cast());
            let px1_lo = _mm_loadu_si128(ptr1.cast());
            let px1_hi = _mm_loadu_si128(ptr1.add(16).cast());

            // Extract R from row0 pixels as i32, then convert to i16.
            let r0_lo = _mm_and_si128(px0_lo, channel_mask);
            let r0_hi = _mm_and_si128(px0_hi, channel_mask);
            let r1_lo = _mm_and_si128(px1_lo, channel_mask);
            let r1_hi = _mm_and_si128(px1_hi, channel_mask);

            // Average: (r0 + r1 + 1) >> 1 for vertical, then horizontal pairs.
            // First average vertically (row0 + row1).
            let r_v_lo = _mm_add_epi32(r0_lo, r1_lo); // sum of 2 rows, 4 pixels
            let r_v_hi = _mm_add_epi32(r0_hi, r1_hi);
            // Pack to i16.
            let r_v = _mm_packs_epi32(r_v_lo, r_v_hi); // 8x i16

            // Now average horizontal pairs: sum adjacent pairs.
            // r_v = [r0, r1, r2, r3, r4, r5, r6, r7]
            // We want: [(r0+r1)/4, (r2+r3)/4, (r4+r5)/4, (r6+r7)/4]
            let r_even = _mm_and_si128(r_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let r_odd = _mm_srli_epi32(r_v, 16);
            let r_sum = _mm_add_epi16(r_even, r_odd); // pairs summed in even i16 positions
                                                      // r_sum as 4×i32 = [sum01, sum23, sum45, sum67] (high 16 bits are 0).
                                                      // Pack to consecutive i16 lanes and divide by 4 with rounding.
            let r_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(r_sum, zero), _mm_set1_epi16(2)), 2);

            // Extract G.
            let g0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 8), channel_mask);
            let g0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 8), channel_mask);
            let g1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 8), channel_mask);
            let g1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 8), channel_mask);
            let g_v_lo = _mm_add_epi32(g0_lo, g1_lo);
            let g_v_hi = _mm_add_epi32(g0_hi, g1_hi);
            let g_v = _mm_packs_epi32(g_v_lo, g_v_hi);
            let g_even = _mm_and_si128(g_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let g_odd = _mm_srli_epi32(g_v, 16);
            let g_sum = _mm_add_epi16(g_even, g_odd);
            let g_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(g_sum, zero), _mm_set1_epi16(2)), 2);

            // Extract B.
            let b0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 16), channel_mask);
            let b0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 16), channel_mask);
            let b1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 16), channel_mask);
            let b1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 16), channel_mask);
            let b_v_lo = _mm_add_epi32(b0_lo, b1_lo);
            let b_v_hi = _mm_add_epi32(b0_hi, b1_hi);
            let b_v = _mm_packs_epi32(b_v_lo, b_v_hi);
            let b_even = _mm_and_si128(b_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let b_odd = _mm_srli_epi32(b_v, 16);
            let b_sum = _mm_add_epi16(b_even, b_odd);
            let b_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(b_sum, zero), _mm_set1_epi16(2)), 2);

            // U = ((-38*R - 74*G + 112*B + 128) >> 8) + 128
            let cb_result = _mm_add_epi16(
                _mm_srai_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_add_epi16(
                                _mm_mullo_epi16(coeff_cb_r, r_avg),
                                _mm_mullo_epi16(coeff_cb_g, g_avg),
                            ),
                            _mm_mullo_epi16(coeff_cb_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            // V = ((112*R - 94*G - 18*B + 128) >> 8) + 128
            let cr_result = _mm_add_epi16(
                _mm_srai_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_add_epi16(
                                _mm_mullo_epi16(coeff_cr_r, r_avg),
                                _mm_mullo_epi16(coeff_cr_g, g_avg),
                            ),
                            _mm_mullo_epi16(coeff_cr_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            // Pack and clamp to [0, 255].
            let cb_packed = _mm_packus_epi16(cb_result, zero);
            let cr_packed = _mm_packus_epi16(cr_result, zero);

            // Store 4 U and 4 V values via single movd instructions.
            _mm_storeu_si32(u_out.as_mut_ptr().add(ccol).cast(), cb_packed);
            _mm_storeu_si32(v_out.as_mut_ptr().add(ccol).cast(), cr_packed);

            ccol += 4;
        }
        ccol
    }

    // ── RGBA8 → NV12 chroma row (SSE2: 4 interleaved UV pairs / iter) ────

    /// Convert one pair of RGBA8 rows to interleaved `[U, V, U, V, …]`
    /// chroma samples for NV12 output, using SSE2.
    ///
    /// Identical 2×2 averaging and coefficient maths as
    /// [`rgba8_to_chroma_row_sse2`], but the final store interleaves U and V
    /// bytes instead of writing to separate planes.
    ///
    /// Returns the number of chroma *pairs* converted (multiple of 4).
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn rgba8_to_chroma_row_nv12_sse2(
        rgba_row0: &[u8],
        rgba_row1: &[u8],
        uv_out: &mut [u8],
        chroma_width: usize,
        luma_width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm_add_epi16, _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi16,
            _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32, _mm_set_epi16,
            _mm_setzero_si128, _mm_srai_epi16, _mm_srli_epi32, _mm_storel_epi64, _mm_unpacklo_epi8,
        };

        let simd_width = chroma_width & !3; // 4 chroma pairs per iteration
        if simd_width == 0 || luma_width < 8 {
            return 0;
        }

        let coeff_cb_r = _mm_set1_epi16(-38);
        let coeff_cb_g = _mm_set1_epi16(-74);
        let coeff_cb_b = _mm_set1_epi16(112);
        let coeff_cr_r = _mm_set1_epi16(112);
        let coeff_cr_g = _mm_set1_epi16(-94);
        let coeff_cr_b = _mm_set1_epi16(-18);
        let rounding = _mm_set1_epi16(128);
        let bias_128 = _mm_set1_epi16(128);
        let zero = _mm_setzero_si128();
        let channel_mask = _mm_set1_epi32(0xFF);

        let mut ccol = 0usize;
        while ccol < simd_width {
            let luma_col = ccol * 2;
            if luma_col + 8 > luma_width {
                break;
            }

            // Load 8 pixels from row0 and row1.
            let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
            let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
            let px0_lo = _mm_loadu_si128(ptr0.cast());
            let px0_hi = _mm_loadu_si128(ptr0.add(16).cast());
            let px1_lo = _mm_loadu_si128(ptr1.cast());
            let px1_hi = _mm_loadu_si128(ptr1.add(16).cast());

            // ── 2×2 average for R channel ──
            let r0_lo = _mm_and_si128(px0_lo, channel_mask);
            let r0_hi = _mm_and_si128(px0_hi, channel_mask);
            let r1_lo = _mm_and_si128(px1_lo, channel_mask);
            let r1_hi = _mm_and_si128(px1_hi, channel_mask);
            let r_v_lo = _mm_add_epi32(r0_lo, r1_lo);
            let r_v_hi = _mm_add_epi32(r0_hi, r1_hi);
            let r_v = _mm_packs_epi32(r_v_lo, r_v_hi);
            let r_even = _mm_and_si128(r_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let r_odd = _mm_srli_epi32(r_v, 16);
            let r_sum = _mm_add_epi16(r_even, r_odd);
            let r_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(r_sum, zero), _mm_set1_epi16(2)), 2);

            // ── 2×2 average for G channel ──
            let g0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 8), channel_mask);
            let g0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 8), channel_mask);
            let g1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 8), channel_mask);
            let g1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 8), channel_mask);
            let g_v_lo = _mm_add_epi32(g0_lo, g1_lo);
            let g_v_hi = _mm_add_epi32(g0_hi, g1_hi);
            let g_v = _mm_packs_epi32(g_v_lo, g_v_hi);
            let g_even = _mm_and_si128(g_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let g_odd = _mm_srli_epi32(g_v, 16);
            let g_sum = _mm_add_epi16(g_even, g_odd);
            let g_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(g_sum, zero), _mm_set1_epi16(2)), 2);

            // ── 2×2 average for B channel ──
            let b0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 16), channel_mask);
            let b0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 16), channel_mask);
            let b1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 16), channel_mask);
            let b1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 16), channel_mask);
            let b_v_lo = _mm_add_epi32(b0_lo, b1_lo);
            let b_v_hi = _mm_add_epi32(b0_hi, b1_hi);
            let b_v = _mm_packs_epi32(b_v_lo, b_v_hi);
            let b_even = _mm_and_si128(b_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
            let b_odd = _mm_srli_epi32(b_v, 16);
            let b_sum = _mm_add_epi16(b_even, b_odd);
            let b_avg =
                _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(b_sum, zero), _mm_set1_epi16(2)), 2);

            // ── Cb / Cr coefficient multiplies (same as I420 kernel) ──
            let cb_result = _mm_add_epi16(
                _mm_srai_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_add_epi16(
                                _mm_mullo_epi16(coeff_cb_r, r_avg),
                                _mm_mullo_epi16(coeff_cb_g, g_avg),
                            ),
                            _mm_mullo_epi16(coeff_cb_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            let cr_result = _mm_add_epi16(
                _mm_srai_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_add_epi16(
                                _mm_mullo_epi16(coeff_cr_r, r_avg),
                                _mm_mullo_epi16(coeff_cr_g, g_avg),
                            ),
                            _mm_mullo_epi16(coeff_cr_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            // Pack to u8 and interleave: [U0,V0,U1,V1,U2,V2,U3,V3].
            let cb_packed = _mm_packus_epi16(cb_result, zero);
            let cr_packed = _mm_packus_epi16(cr_result, zero);
            let interleaved = _mm_unpacklo_epi8(cb_packed, cr_packed);

            // Store 8 bytes (4 UV pairs) via a single movq instruction.
            _mm_storel_epi64(uv_out.as_mut_ptr().add(ccol * 2).cast(), interleaved);

            ccol += 4;
        }
        ccol
    }

    // ── RGBA8 → NV12 chroma row (AVX2: 8 interleaved UV pairs / iter) ────

    /// Convert one pair of RGBA8 rows to interleaved `[U, V, U, V, …]`
    /// chroma samples for NV12 output, using AVX2.
    ///
    /// Processes 8 chroma pairs (16 luma pixels) per iteration — double the
    /// throughput of the SSE2 variant.  Same 2×2 averaging and BT.601
    /// coefficient maths.
    ///
    /// Returns the number of chroma *pairs* converted (multiple of 8).
    #[target_feature(enable = "avx2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn rgba8_to_chroma_row_nv12_avx2(
        rgba_row0: &[u8],
        rgba_row1: &[u8],
        uv_out: &mut [u8],
        chroma_width: usize,
        luma_width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm256_add_epi16, _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128,
            _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_mullo_epi16, _mm256_packs_epi32,
            _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_set1_epi16, _mm256_set1_epi32,
            _mm256_set_epi16, _mm256_setzero_si256, _mm256_srai_epi16, _mm256_srli_epi32,
            _mm_storeu_si128, _mm_unpacklo_epi32, _mm_unpacklo_epi8,
        };

        let simd_width = chroma_width & !7; // 8 chroma pairs per iteration
        if simd_width == 0 || luma_width < 16 {
            return 0;
        }

        let coeff_cb_r = _mm256_set1_epi16(-38);
        let coeff_cb_g = _mm256_set1_epi16(-74);
        let coeff_cb_b = _mm256_set1_epi16(112);
        let coeff_cr_r = _mm256_set1_epi16(112);
        let coeff_cr_g = _mm256_set1_epi16(-94);
        let coeff_cr_b = _mm256_set1_epi16(-18);
        let rounding = _mm256_set1_epi16(128);
        let bias_128 = _mm256_set1_epi16(128);
        let zero = _mm256_setzero_si256();
        let channel_mask = _mm256_set1_epi32(0xFF);
        let even_mask = _mm256_set_epi16(0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1);

        let mut ccol = 0usize;
        while ccol < simd_width {
            let luma_col = ccol * 2;
            if luma_col + 16 > luma_width {
                break;
            }

            // Load 16 pixels from row0 and row1 (64 bytes each = 4 × 128-bit).
            let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
            let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
            let px0_a = _mm256_loadu_si256(ptr0.cast()); // pixels 0..7
            let px0_b = _mm256_loadu_si256(ptr0.add(32).cast()); // pixels 8..15
            let px1_a = _mm256_loadu_si256(ptr1.cast());
            let px1_b = _mm256_loadu_si256(ptr1.add(32).cast());

            // ── 2×2 average for R channel ──
            let r0_a = _mm256_and_si256(px0_a, channel_mask);
            let r0_b = _mm256_and_si256(px0_b, channel_mask);
            let r1_a = _mm256_and_si256(px1_a, channel_mask);
            let r1_b = _mm256_and_si256(px1_b, channel_mask);
            let r_v_a = _mm256_add_epi32(r0_a, r1_a);
            let r_v_b = _mm256_add_epi32(r0_b, r1_b);
            // _mm256_packs_epi32 operates per 128-bit lane, scrambling
            // elements across the two source registers.  vpermq with
            // control 0xD8 swaps qwords 1 and 2 to restore sequential order.
            let r_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(r_v_a, r_v_b), 0xD8);
            let r_even = _mm256_and_si256(r_v, even_mask);
            let r_odd = _mm256_srli_epi32(r_v, 16);
            let r_sum = _mm256_add_epi16(r_even, r_odd);
            let r_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(r_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── 2×2 average for G channel ──
            let g0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 8), channel_mask);
            let g0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 8), channel_mask);
            let g1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 8), channel_mask);
            let g1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 8), channel_mask);
            let g_v_a = _mm256_add_epi32(g0_a, g1_a);
            let g_v_b = _mm256_add_epi32(g0_b, g1_b);
            let g_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(g_v_a, g_v_b), 0xD8);
            let g_even = _mm256_and_si256(g_v, even_mask);
            let g_odd = _mm256_srli_epi32(g_v, 16);
            let g_sum = _mm256_add_epi16(g_even, g_odd);
            let g_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(g_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── 2×2 average for B channel ──
            let b0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 16), channel_mask);
            let b0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 16), channel_mask);
            let b1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 16), channel_mask);
            let b1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 16), channel_mask);
            let b_v_a = _mm256_add_epi32(b0_a, b1_a);
            let b_v_b = _mm256_add_epi32(b0_b, b1_b);
            let b_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(b_v_a, b_v_b), 0xD8);
            let b_even = _mm256_and_si256(b_v, even_mask);
            let b_odd = _mm256_srli_epi32(b_v, 16);
            let b_sum = _mm256_add_epi16(b_even, b_odd);
            let b_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(b_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── Cb / Cr coefficient multiplies ──
            let cb_result = _mm256_add_epi16(
                _mm256_srai_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_add_epi16(
                                _mm256_mullo_epi16(coeff_cb_r, r_avg),
                                _mm256_mullo_epi16(coeff_cb_g, g_avg),
                            ),
                            _mm256_mullo_epi16(coeff_cb_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            let cr_result = _mm256_add_epi16(
                _mm256_srai_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_add_epi16(
                                _mm256_mullo_epi16(coeff_cr_r, r_avg),
                                _mm256_mullo_epi16(coeff_cr_g, g_avg),
                            ),
                            _mm256_mullo_epi16(coeff_cr_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            // Pack to u8 and interleave U/V for NV12 layout.
            // AVX2 pack operates per 128-bit lane, so we need to de-lane.
            let cb_packed = _mm256_packus_epi16(cb_result, zero);
            let cr_packed = _mm256_packus_epi16(cr_result, zero);
            // cb_packed lanes: lane0=[cb0,cb1,cb2,cb3, 0..0]  lane1=[cb4,cb5,cb6,cb7, 0..0]
            let cb_lo = _mm256_castsi256_si128(cb_packed);
            let cb_hi = _mm256_extracti128_si256(cb_packed, 1);
            let cr_lo = _mm256_castsi256_si128(cr_packed);
            let cr_hi = _mm256_extracti128_si256(cr_packed, 1);
            // Combine the two 4-byte halves into contiguous 8-byte vectors.
            let cb8 = _mm_unpacklo_epi32(cb_lo, cb_hi); // [cb0..cb3, cb4..cb7, 0..0, 0..0]
            let cr8 = _mm_unpacklo_epi32(cr_lo, cr_hi); // [cr0..cr3, cr4..cr7, 0..0, 0..0]
                                                        // Interleave bytes: _mm_unpacklo_epi8 on the dword-combined vectors
                                                        // gives [cb0,cr0,cb1,cr1,…,cb7,cr7] — exactly 16 bytes of UV pairs.
            let interleaved = _mm_unpacklo_epi8(cb8, cr8);

            // Store 16 bytes (8 UV pairs) via a single movdqu instruction.
            _mm_storeu_si128(uv_out.as_mut_ptr().add(ccol * 2).cast(), interleaved);

            ccol += 8;
        }
        ccol
    }

    // ── RGBA8 → I420 chroma row (AVX2: 8 chroma samples / iter) ──────────

    /// Convert one pair of RGBA8 rows to U and V chroma samples using AVX2.
    ///
    /// Processes 8 chroma samples (16 luma pixels) per iteration — double
    /// the throughput of the SSE2 variant.  Same 2×2 averaging and BT.601
    /// coefficient maths.
    ///
    /// Returns the number of chroma samples converted (multiple of 8).
    #[target_feature(enable = "avx2")]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
    pub(super) unsafe fn rgba8_to_chroma_row_avx2(
        rgba_row0: &[u8],
        rgba_row1: &[u8],
        u_out: &mut [u8],
        v_out: &mut [u8],
        chroma_width: usize,
        luma_width: usize,
    ) -> usize {
        use std::arch::x86_64::{
            _mm256_add_epi16, _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128,
            _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_mullo_epi16, _mm256_packs_epi32,
            _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_set1_epi16, _mm256_set1_epi32,
            _mm256_set_epi16, _mm256_setzero_si256, _mm256_srai_epi16, _mm256_srli_epi32,
            _mm_storel_epi64, _mm_unpacklo_epi32,
        };

        let simd_width = chroma_width & !7; // 8 chroma samples per iteration
        if simd_width == 0 || luma_width < 16 {
            return 0;
        }

        let coeff_cb_r = _mm256_set1_epi16(-38);
        let coeff_cb_g = _mm256_set1_epi16(-74);
        let coeff_cb_b = _mm256_set1_epi16(112);
        let coeff_cr_r = _mm256_set1_epi16(112);
        let coeff_cr_g = _mm256_set1_epi16(-94);
        let coeff_cr_b = _mm256_set1_epi16(-18);
        let rounding = _mm256_set1_epi16(128);
        let bias_128 = _mm256_set1_epi16(128);
        let zero = _mm256_setzero_si256();
        let channel_mask = _mm256_set1_epi32(0xFF);
        let even_mask = _mm256_set_epi16(0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1);

        let mut ccol = 0usize;
        while ccol < simd_width {
            let luma_col = ccol * 2;
            if luma_col + 16 > luma_width {
                break;
            }

            let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
            let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
            let px0_a = _mm256_loadu_si256(ptr0.cast());
            let px0_b = _mm256_loadu_si256(ptr0.add(32).cast());
            let px1_a = _mm256_loadu_si256(ptr1.cast());
            let px1_b = _mm256_loadu_si256(ptr1.add(32).cast());

            // ── 2×2 average for R channel ──
            let r0_a = _mm256_and_si256(px0_a, channel_mask);
            let r0_b = _mm256_and_si256(px0_b, channel_mask);
            let r1_a = _mm256_and_si256(px1_a, channel_mask);
            let r1_b = _mm256_and_si256(px1_b, channel_mask);
            let r_v_a = _mm256_add_epi32(r0_a, r1_a);
            let r_v_b = _mm256_add_epi32(r0_b, r1_b);
            // Fix AVX2 lane-crossing: vpermq 0xD8 after cross-source pack.
            let r_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(r_v_a, r_v_b), 0xD8);
            let r_even = _mm256_and_si256(r_v, even_mask);
            let r_odd = _mm256_srli_epi32(r_v, 16);
            let r_sum = _mm256_add_epi16(r_even, r_odd);
            let r_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(r_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── 2×2 average for G channel ──
            let g0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 8), channel_mask);
            let g0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 8), channel_mask);
            let g1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 8), channel_mask);
            let g1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 8), channel_mask);
            let g_v_a = _mm256_add_epi32(g0_a, g1_a);
            let g_v_b = _mm256_add_epi32(g0_b, g1_b);
            let g_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(g_v_a, g_v_b), 0xD8);
            let g_even = _mm256_and_si256(g_v, even_mask);
            let g_odd = _mm256_srli_epi32(g_v, 16);
            let g_sum = _mm256_add_epi16(g_even, g_odd);
            let g_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(g_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── 2×2 average for B channel ──
            let b0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 16), channel_mask);
            let b0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 16), channel_mask);
            let b1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 16), channel_mask);
            let b1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 16), channel_mask);
            let b_v_a = _mm256_add_epi32(b0_a, b1_a);
            let b_v_b = _mm256_add_epi32(b0_b, b1_b);
            let b_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(b_v_a, b_v_b), 0xD8);
            let b_even = _mm256_and_si256(b_v, even_mask);
            let b_odd = _mm256_srli_epi32(b_v, 16);
            let b_sum = _mm256_add_epi16(b_even, b_odd);
            let b_avg = _mm256_srai_epi16(
                _mm256_add_epi16(_mm256_packs_epi32(b_sum, zero), _mm256_set1_epi16(2)),
                2,
            );

            // ── Cb / Cr coefficient multiplies ──
            let cb_result = _mm256_add_epi16(
                _mm256_srai_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_add_epi16(
                                _mm256_mullo_epi16(coeff_cb_r, r_avg),
                                _mm256_mullo_epi16(coeff_cb_g, g_avg),
                            ),
                            _mm256_mullo_epi16(coeff_cb_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            let cr_result = _mm256_add_epi16(
                _mm256_srai_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_add_epi16(
                                _mm256_mullo_epi16(coeff_cr_r, r_avg),
                                _mm256_mullo_epi16(coeff_cr_g, g_avg),
                            ),
                            _mm256_mullo_epi16(coeff_cr_b, b_avg),
                        ),
                        rounding,
                    ),
                    8,
                ),
                bias_128,
            );

            // Pack to u8.  AVX2 packus works per 128-bit lane.
            let cb_packed = _mm256_packus_epi16(cb_result, zero);
            let cr_packed = _mm256_packus_epi16(cr_result, zero);
            // Each has 4 values in low dword of lane 0, 4 values in low dword of lane 1.
            let cb_lo = _mm256_castsi256_si128(cb_packed);
            let cb_hi = _mm256_extracti128_si256(cb_packed, 1);
            let cr_lo = _mm256_castsi256_si128(cr_packed);
            let cr_hi = _mm256_extracti128_si256(cr_packed, 1);
            let cb8 = _mm_unpacklo_epi32(cb_lo, cb_hi); // [cb0..cb3, cb4..cb7]
            let cr8 = _mm_unpacklo_epi32(cr_lo, cr_hi); // [cr0..cr3, cr4..cr7]

            // Store 8 U and 8 V values via single movq instructions.
            _mm_storel_epi64(u_out.as_mut_ptr().add(ccol).cast(), cb8);
            _mm_storel_epi64(v_out.as_mut_ptr().add(ccol).cast(), cr8);

            ccol += 8;
        }
        ccol
    }
}

// ── Pixel format conversion ─────────────────────────────────────────────────

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8, writing into `out`.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Rows are processed in parallel via `rayon`.
///
/// On x86-64 with SSE2 support the inner per-row loop is vectorised to
/// process 8 pixels per iteration, falling back to scalar for tail pixels.
///
/// **Note:** This function assumes a *packed* I420 layout (luma stride = width,
/// chroma stride = ⌈width/2⌉).  If non-packed / aligned layouts are introduced
/// in the future, a stride-aware variant should be added.
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

    // Hoist CPU feature detection once, outside the per-row closure.
    #[cfg(target_arch = "x86_64")]
    let use_sse41 = is_x86_feature_detected!("sse4.1");
    #[cfg(target_arch = "x86_64")]
    let use_sse2 = is_x86_feature_detected!("sse2");

    let convert_row = |row: usize, rgba_row: &mut [u8]| {
        let y_base = row * y_stride;
        let chroma_row = row / 2;
        let u_base = u_offset + chroma_row * chroma_w;
        let v_base = v_offset + chroma_row * chroma_w;

        let mut start_col = 0usize;

        // SIMD fast path: prefer SSE4.1 (native i32 mul) over SSE2.
        #[cfg(target_arch = "x86_64")]
        {
            if use_sse41 {
                start_col = unsafe {
                    simd::i420_to_rgba8_row_sse41(
                        &data[y_base..y_base + w],
                        &data[u_base..u_base + chroma_w],
                        &data[v_base..v_base + chroma_w],
                        rgba_row,
                        w,
                    )
                };
            } else if use_sse2 {
                start_col = unsafe {
                    simd::i420_to_rgba8_row_sse2(
                        &data[y_base..y_base + w],
                        &data[u_base..u_base + chroma_w],
                        &data[v_base..v_base + chroma_w],
                        rgba_row,
                        w,
                    )
                };
            }
        }

        // Scalar tail (or full row on non-x86-64 / without SSE2).
        for col in start_col..w {
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
        let chunk_rows = rayon_chunk_rows(h);
        let chunk_bytes = rgba_row_stride * chunk_rows;
        out[..w * h * 4].par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, rgba_row) in chunk.chunks_mut(rgba_row_stride).enumerate() {
                let row = base_row + j;
                if row >= h {
                    break;
                }
                convert_row(row, rgba_row);
            }
        });
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

/// Convert an NV12 (Y + interleaved UV) buffer to RGBA8, writing into `out`.
///
/// Same BT.601 math as [`i420_to_rgba8_buf`], but reads U and V from a single
/// interleaved UV plane instead of two separate planes.  Uses a dedicated
/// NV12 SSE2 kernel that reads the interleaved UV data in-place — no
/// scratch-buffer deinterleaving or thread-local storage required.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Input `data` must be a packed NV12 buffer: `width * height` luma bytes
/// followed by `⌈width/2⌉ * 2 * ⌈height/2⌉` interleaved UV bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn nv12_to_rgba8_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let uv_stride = chroma_w * 2; // interleaved UV pairs
    let uv_offset = y_stride * h;
    let rgba_row_stride = w * 4;

    // Hoist CPU feature detection once, outside the per-row closure.
    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");
    #[cfg(target_arch = "x86_64")]
    let use_sse41 = is_x86_feature_detected!("sse4.1");
    #[cfg(target_arch = "x86_64")]
    let use_sse2 = is_x86_feature_detected!("sse2");

    let convert_row = |row: usize, rgba_row: &mut [u8]| {
        let y_base = row * y_stride;
        let chroma_row = row / 2;
        let uv_base = uv_offset + chroma_row * uv_stride;

        let mut start_col = 0usize;

        // SIMD fast path: prefer AVX2 > SSE4.1 > SSE2.
        #[cfg(target_arch = "x86_64")]
        {
            if use_avx2 {
                start_col = unsafe {
                    simd::nv12_to_rgba8_row_avx2(
                        &data[y_base..y_base + w],
                        &data[uv_base..uv_base + uv_stride],
                        rgba_row,
                        w,
                    )
                };
                // Handle remaining pixels (up to 7) with SSE4.1.
                if start_col < w && use_sse41 {
                    start_col += unsafe {
                        simd::nv12_to_rgba8_row_sse41(
                            &data[y_base + start_col..y_base + w],
                            &data[uv_base + (start_col / 2) * 2..uv_base + uv_stride],
                            &mut rgba_row[start_col * 4..],
                            w - start_col,
                        )
                    };
                }
            } else if use_sse41 {
                start_col = unsafe {
                    simd::nv12_to_rgba8_row_sse41(
                        &data[y_base..y_base + w],
                        &data[uv_base..uv_base + uv_stride],
                        rgba_row,
                        w,
                    )
                };
            } else if use_sse2 {
                start_col = unsafe {
                    simd::nv12_to_rgba8_row_sse2(
                        &data[y_base..y_base + w],
                        &data[uv_base..uv_base + uv_stride],
                        rgba_row,
                        w,
                    )
                };
            }
        }

        // Scalar tail (or full row on non-x86-64 / without SSE2).
        for col in start_col..w {
            let y_val = i32::from(data[y_base + col]);
            let u_val = i32::from(data[uv_base + (col / 2) * 2]);
            let v_val = i32::from(data[uv_base + (col / 2) * 2 + 1]);

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
        let chunk_rows = rayon_chunk_rows(h);
        let chunk_bytes = rgba_row_stride * chunk_rows;
        out[..w * h * 4].par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, rgba_row) in chunk.chunks_mut(rgba_row_stride).enumerate() {
                let row = base_row + j;
                if row >= h {
                    break;
                }
                convert_row(row, rgba_row);
            }
        });
    } else {
        for (row, rgba_row) in out[..w * h * 4].chunks_mut(rgba_row_stride).take(h).enumerate() {
            convert_row(row, rgba_row);
        }
    }
}

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + 2 * ((w+1)/2) * ((h+1)/2)`.
/// Y, U and V planes are processed in parallel via `rayon`.
///
/// **Note:** This function assumes a *packed* RGBA8 layout (stride = width × 4)
/// and writes a packed I420 output (luma stride = width, chroma stride = ⌈width/2⌉).
/// If non-packed / aligned layouts are introduced in the future, a stride-aware
/// variant should be added.
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

    // Hoist CPU feature detection once, outside the per-row closures.
    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");
    #[cfg(target_arch = "x86_64")]
    let use_sse41 = is_x86_feature_detected!("sse4.1");
    #[cfg(target_arch = "x86_64")]
    let use_sse2 = is_x86_feature_detected!("sse2");

    // Y plane — parallelise by row.
    let convert_y_row = |row: usize, y_row: &mut [u8]| {
        let rgba_base = row * w * 4;
        let mut start_col = 0usize;

        // SIMD fast path: prefer AVX2 > SSE4.1 > SSE2.
        #[cfg(target_arch = "x86_64")]
        {
            if use_avx2 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_avx2(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
                // Handle remaining pixels (up to 7) with SSE4.1/SSE2.
                if start_col < w && use_sse41 {
                    let tail = unsafe {
                        simd::rgba8_to_y_row_sse41(
                            &data[rgba_base + start_col * 4..rgba_base + w * 4],
                            &mut y_row[start_col..],
                            w - start_col,
                        )
                    };
                    start_col += tail;
                }
            } else if use_sse41 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_sse41(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
            } else if use_sse2 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_sse2(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
            }
        }

        for (col, y_out) in y_row.iter_mut().enumerate().take(w).skip(start_col) {
            let off = rgba_base + col * 4;
            let r = i32::from(data[off]);
            let g = i32::from(data[off + 1]);
            let b = i32::from(data[off + 2]);
            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            *y_out = y.clamp(0, 255) as u8;
        }
    };

    if h >= RAYON_ROW_THRESHOLD {
        let chunk_rows = rayon_chunk_rows(h);
        let chunk_bytes = y_stride * chunk_rows;
        y_plane.par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, y_row) in chunk.chunks_mut(y_stride).enumerate() {
                let row = base_row + j;
                if row >= h {
                    break;
                }
                convert_y_row(row, y_row);
            }
        });
    } else {
        for (row, y_row) in y_plane.chunks_mut(y_stride).take(h).enumerate() {
            convert_y_row(row, y_row);
        }
    }

    // U and V planes — parallelise by chroma row.
    let convert_chroma_row = |crow: usize, u_row: &mut [u8], v_row: &mut [u8]| {
        let r0 = crow * 2;
        let mut start_ccol = 0usize;

        // SIMD fast path for chroma subsampling.
        // Prefer AVX2 (8 chroma samples/iter) > SSE2 (4 samples/iter).
        #[cfg(target_arch = "x86_64")]
        {
            if r0 + 1 < h {
                let row0_start = r0 * w * 4;
                let row1_start = (r0 + 1) * w * 4;
                let rgba_row0 = &data[row0_start..row0_start + w * 4];
                let rgba_row1 = &data[row1_start..row1_start + w * 4];

                if use_avx2 {
                    // SAFETY: feature detection guarantees AVX2 is available.
                    start_ccol = unsafe {
                        simd::rgba8_to_chroma_row_avx2(
                            rgba_row0, rgba_row1, u_row, v_row, chroma_w, w,
                        )
                    };
                }
                // SSE2 tail (or full row if AVX2 unavailable).
                if start_ccol < chroma_w && use_sse2 {
                    start_ccol += unsafe {
                        simd::rgba8_to_chroma_row_sse2(
                            &rgba_row0[start_ccol * 2 * 4..],
                            &rgba_row1[start_ccol * 2 * 4..],
                            &mut u_row[start_ccol..],
                            &mut v_row[start_ccol..],
                            chroma_w - start_ccol,
                            w - start_ccol * 2,
                        )
                    };
                }
            }
        }

        for ccol in start_ccol..chroma_w {
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

/// Convert an RGBA8 buffer to NV12 (Y + interleaved UV), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + ⌈w/2⌉ * 2 * ⌈h/2⌉`.
/// Y plane is computed with the same SSE2-accelerated kernel as
/// [`rgba8_to_i420_buf`].  The chroma plane writes interleaved `[U, V]`
/// pairs instead of separate U and V planes.
///
/// **Note:** Assumes packed RGBA8 input (stride = width × 4) and writes a
/// packed NV12 output (luma stride = width, chroma stride = ⌈width/2⌉ × 2).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn rgba8_to_nv12_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    use rayon::prelude::*;

    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let y_size = y_stride * h;
    let uv_stride = chroma_w * 2;

    // Split output into Y plane and UV plane.
    let (y_plane, uv_plane) = out[..y_size + uv_stride * chroma_h].split_at_mut(y_size);

    // Hoist CPU feature detection once, outside the per-row closures.
    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");
    #[cfg(target_arch = "x86_64")]
    let use_sse41 = is_x86_feature_detected!("sse4.1");
    #[cfg(target_arch = "x86_64")]
    let use_sse2 = is_x86_feature_detected!("sse2");

    // Y plane — parallelise by row (prefers AVX2, falls back to SSE4.1/SSE2).
    let convert_y_row = |row: usize, y_row: &mut [u8]| {
        let rgba_base = row * w * 4;
        let mut start_col = 0usize;

        // SIMD fast path: prefer AVX2 > SSE4.1 > SSE2.
        #[cfg(target_arch = "x86_64")]
        {
            if use_avx2 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_avx2(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
                // Handle remaining pixels (up to 7) with SSE4.1/SSE2.
                if start_col < w && use_sse41 {
                    let tail = unsafe {
                        simd::rgba8_to_y_row_sse41(
                            &data[rgba_base + start_col * 4..rgba_base + w * 4],
                            &mut y_row[start_col..],
                            w - start_col,
                        )
                    };
                    start_col += tail;
                }
            } else if use_sse41 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_sse41(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
            } else if use_sse2 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_sse2(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
            }
        }

        for (col, y_out) in y_row.iter_mut().enumerate().take(w).skip(start_col) {
            let off = rgba_base + col * 4;
            let r = i32::from(data[off]);
            let g = i32::from(data[off + 1]);
            let b = i32::from(data[off + 2]);
            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            *y_out = y.clamp(0, 255) as u8;
        }
    };

    if h >= RAYON_ROW_THRESHOLD {
        let chunk_rows = rayon_chunk_rows(h);
        let chunk_bytes = y_stride * chunk_rows;
        y_plane.par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, y_row) in chunk.chunks_mut(y_stride).enumerate() {
                let row = base_row + j;
                if row >= h {
                    break;
                }
                convert_y_row(row, y_row);
            }
        });
    } else {
        for (row, y_row) in y_plane.chunks_mut(y_stride).take(h).enumerate() {
            convert_y_row(row, y_row);
        }
    }

    // UV plane — parallelise by chroma row, write interleaved [U, V] pairs.
    // Uses SSE2 SIMD for the bulk of each row, with a scalar tail.
    let convert_chroma_row = |crow: usize, uv_row: &mut [u8]| {
        let r0 = crow * 2;
        let mut start_ccol = 0usize;

        // SIMD fast path for interleaved NV12 chroma subsampling.
        // Prefer AVX2 (8 chroma pairs/iter) > SSE2 (4 pairs/iter).
        #[cfg(target_arch = "x86_64")]
        {
            if r0 + 1 < h {
                let row0_start = r0 * w * 4;
                let row1_start = (r0 + 1) * w * 4;
                let rgba_row0 = &data[row0_start..row0_start + w * 4];
                let rgba_row1 = &data[row1_start..row1_start + w * 4];

                if use_avx2 {
                    // SAFETY: feature detection guarantees AVX2 is available.
                    start_ccol = unsafe {
                        simd::rgba8_to_chroma_row_nv12_avx2(
                            rgba_row0, rgba_row1, uv_row, chroma_w, w,
                        )
                    };
                }
                // SSE2 tail (or full row if AVX2 unavailable).
                if start_ccol < chroma_w && use_sse2 {
                    start_ccol += unsafe {
                        simd::rgba8_to_chroma_row_nv12_sse2(
                            &rgba_row0[start_ccol * 2 * 4..],
                            &rgba_row1[start_ccol * 2 * 4..],
                            &mut uv_row[start_ccol * 2..],
                            chroma_w - start_ccol,
                            w - start_ccol * 2,
                        )
                    };
                }
            }
        }

        for ccol in start_ccol..chroma_w {
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
            uv_row[ccol * 2] = u.clamp(0, 255) as u8;
            uv_row[ccol * 2 + 1] = v.clamp(0, 255) as u8;
        }
    };

    if chroma_h >= RAYON_ROW_THRESHOLD / 2 {
        let chunk_rows = rayon_chunk_rows(chroma_h);
        let chunk_bytes = uv_stride * chunk_rows;
        uv_plane.par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_crow = chunk_idx * chunk_rows;
            for (j, uv_row) in chunk.chunks_mut(uv_stride).enumerate() {
                let crow = base_crow + j;
                if crow >= chroma_h {
                    break;
                }
                convert_chroma_row(crow, uv_row);
            }
        });
    } else {
        for (crow, uv_row) in uv_plane.chunks_mut(uv_stride).take(chroma_h).enumerate() {
            convert_chroma_row(crow, uv_row);
        }
    }
}
