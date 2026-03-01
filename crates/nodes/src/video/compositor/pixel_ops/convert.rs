// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Colour-space conversion between RGBA8 and YUV 4:2:0 formats (I420, NV12).
//!
//! All conversions use BT.601 coefficients and row-level parallelism via
//! `rayon` when the image is large enough.  On x86-64, SIMD-accelerated
//! kernels are dispatched at runtime via `is_x86_feature_detected!()`.

use super::{rayon_chunk_rows, RAYON_ROW_THRESHOLD};

#[cfg(target_arch = "x86_64")]
use super::simd;

// ── Shared rayon parallelization helper ─────────────────────────────────────

/// Process `total_rows` of a buffer in parallel (or sequentially for small
/// images), invoking `process_row(row_index, row_slice)` for each row.
///
/// This eliminates the ~20-line rayon boilerplate that was previously
/// duplicated across every public conversion function.
fn parallel_rows(
    buf: &mut [u8],
    row_stride: usize,
    total_rows: usize,
    process_row: impl Fn(usize, &mut [u8]) + Send + Sync,
) {
    use rayon::prelude::*;

    if total_rows >= RAYON_ROW_THRESHOLD {
        let chunk_rows = rayon_chunk_rows(total_rows);
        let chunk_bytes = row_stride * chunk_rows;
        buf.par_chunks_mut(chunk_bytes).enumerate().for_each(|(chunk_idx, chunk)| {
            let base_row = chunk_idx * chunk_rows;
            for (j, row) in chunk.chunks_mut(row_stride).enumerate() {
                let row_idx = base_row + j;
                if row_idx >= total_rows {
                    break;
                }
                process_row(row_idx, row);
            }
        });
    } else {
        for (row_idx, row) in buf.chunks_mut(row_stride).take(total_rows).enumerate() {
            process_row(row_idx, row);
        }
    }
}

// ── Shared Y-plane conversion helper ────────────────────────────────────────

/// Convert the Y (luma) plane from RGBA8 source data using BT.601 coefficients.
///
/// This helper is shared by [`rgba8_to_i420_buf`] and [`rgba8_to_nv12_buf`],
/// which previously contained identical Y-plane conversion closures.
///
/// The SIMD dispatch cascade (AVX2 → SSE4.1 → SSE2 → scalar) is handled
/// internally.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn convert_y_plane(
    data: &[u8],
    w: usize,
    h: usize,
    y_plane: &mut [u8],
    y_stride: usize,
    #[cfg(target_arch = "x86_64")] use_avx2: bool,
    #[cfg(target_arch = "x86_64")] use_sse41: bool,
    #[cfg(target_arch = "x86_64")] use_sse2: bool,
) {
    let convert_y_row = |row: usize, y_row: &mut [u8]| {
        let rgba_base = row * w * 4;
        let mut start_col = 0usize;

        #[cfg(target_arch = "x86_64")]
        {
            if use_avx2 {
                start_col = unsafe {
                    simd::rgba8_to_y_row_avx2(&data[rgba_base..rgba_base + w * 4], y_row, w)
                };
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

    parallel_rows(y_plane, y_stride, h, convert_y_row);
}

// ── I420 → RGBA8 ────────────────────────────────────────────────────────────

/// Convert an I420 (YUV 4:2:0 planar) buffer to RGBA8, writing into `out`.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Rows are processed in parallel via `rayon`.
///
/// On x86-64 with SSE2 support the inner per-row loop is vectorised to
/// process 8 pixels per iteration, falling back to scalar for tail pixels.
///
/// **Note:** This function assumes a *packed* I420 layout (luma stride = width,
/// chroma stride = ceil(width/2)).  If non-packed / aligned layouts are introduced
/// in the future, a stride-aware variant should be added.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn i420_to_rgba8_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let u_offset = y_stride * h;
    let v_offset = u_offset + chroma_w * chroma_h;
    let rgba_row_stride = w * 4;

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

    parallel_rows(&mut out[..w * h * 4], rgba_row_stride, h, convert_row);
}

// ── NV12 → RGBA8 ────────────────────────────────────────────────────────────

/// Convert an NV12 (Y + interleaved UV) buffer to RGBA8, writing into `out`.
///
/// Same BT.601 math as [`i420_to_rgba8_buf`], but reads U and V from a single
/// interleaved UV plane instead of two separate planes.  Uses a dedicated
/// NV12 SSE2 kernel that reads the interleaved UV data in-place — no
/// scratch-buffer deinterleaving or thread-local storage required.
///
/// The caller must ensure `out` has length >= `width * height * 4`.
/// Input `data` must be a packed NV12 buffer: `width * height` luma bytes
/// followed by `ceil(width/2) * 2 * ceil(height/2)` interleaved UV bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn nv12_to_rgba8_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
    let w = width as usize;
    let h = height as usize;
    let y_stride = w;
    let chroma_w = w.div_ceil(2);
    let uv_stride = chroma_w * 2; // interleaved UV pairs
    let uv_offset = y_stride * h;
    let rgba_row_stride = w * 4;

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

    parallel_rows(&mut out[..w * h * 4], rgba_row_stride, h, convert_row);
}

// ── RGBA8 → I420 ────────────────────────────────────────────────────────────

/// Convert an RGBA8 buffer to I420 (YUV 4:2:0 planar), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + 2 * ((w+1)/2) * ((h+1)/2)`.
/// Y, U and V planes are processed in parallel via `rayon`.
///
/// **Note:** This function assumes a *packed* RGBA8 layout (stride = width * 4)
/// and writes a packed I420 output (luma stride = width, chroma stride = ceil(width/2)).
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

    // Y plane — shared helper handles both parallelism and SIMD dispatch.
    convert_y_plane(
        data,
        w,
        h,
        y_plane,
        y_stride,
        #[cfg(target_arch = "x86_64")]
        use_avx2,
        #[cfg(target_arch = "x86_64")]
        use_sse41,
        #[cfg(target_arch = "x86_64")]
        use_sse2,
    );

    // U and V planes — parallelise by chroma row.
    let convert_chroma_row = |crow: usize, u_row: &mut [u8], v_row: &mut [u8]| {
        let r0 = crow * 2;
        let mut start_ccol = 0usize;

        #[cfg(target_arch = "x86_64")]
        {
            if r0 + 1 < h {
                let row0_start = r0 * w * 4;
                let row1_start = (r0 + 1) * w * 4;
                let rgba_row0 = &data[row0_start..row0_start + w * 4];
                let rgba_row1 = &data[row1_start..row1_start + w * 4];

                if use_avx2 {
                    start_ccol = unsafe {
                        simd::rgba8_to_chroma_row_avx2(
                            rgba_row0, rgba_row1, u_row, v_row, chroma_w, w,
                        )
                    };
                }
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

// ── RGBA8 → NV12 ────────────────────────────────────────────────────────────

/// Convert an RGBA8 buffer to NV12 (Y + interleaved UV), writing into `out`.
///
/// The caller must ensure `out` has length >= `w * h + ceil(w/2) * 2 * ceil(h/2)`.
/// Y plane is computed with the same SIMD-accelerated kernel as
/// [`rgba8_to_i420_buf`].  The chroma plane writes interleaved `[U, V]`
/// pairs instead of separate U and V planes.
///
/// **Note:** Assumes packed RGBA8 input (stride = width * 4) and writes a
/// packed NV12 output (luma stride = width, chroma stride = ceil(width/2) * 2).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::many_single_char_names)]
pub fn rgba8_to_nv12_buf(data: &[u8], width: u32, height: u32, out: &mut [u8]) {
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

    // Y plane — shared helper handles both parallelism and SIMD dispatch.
    convert_y_plane(
        data,
        w,
        h,
        y_plane,
        y_stride,
        #[cfg(target_arch = "x86_64")]
        use_avx2,
        #[cfg(target_arch = "x86_64")]
        use_sse41,
        #[cfg(target_arch = "x86_64")]
        use_sse2,
    );

    // UV plane — parallelise by chroma row, write interleaved [U, V] pairs.
    let convert_chroma_row = |crow: usize, uv_row: &mut [u8]| {
        let r0 = crow * 2;
        let mut start_ccol = 0usize;

        #[cfg(target_arch = "x86_64")]
        {
            if r0 + 1 < h {
                let row0_start = r0 * w * 4;
                let row1_start = (r0 + 1) * w * 4;
                let rgba_row0 = &data[row0_start..row0_start + w * 4];
                let rgba_row1 = &data[row1_start..row1_start + w * 4];

                if use_avx2 {
                    start_ccol = unsafe {
                        simd::rgba8_to_chroma_row_nv12_avx2(
                            rgba_row0, rgba_row1, uv_row, chroma_w, w,
                        )
                    };
                }
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

    parallel_rows(uv_plane, uv_stride, chroma_h, |crow, uv_row| {
        convert_chroma_row(crow, uv_row);
    });
}
