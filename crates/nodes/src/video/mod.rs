// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Video submodules use unsafe for SIMD intrinsics (pixel_ops) and libvpx FFI (vp9).
// All unsafe blocks carry safety comments. The crate-level lint is `deny`; this
// module-level `allow` limits the exemption to video code only.
#![allow(unsafe_code)]

//! Video nodes and registration.

use streamkit_core::constraints::GlobalNodeConstraints;
use streamkit_core::types::PixelFormat;
use streamkit_core::{NodeRegistry, StreamKitError};

/// Default video frame duration in microseconds (~30 fps).
///
/// Used as a fallback when incoming packets carry no duration metadata.
/// Shared across WebM muxing and MoQ transport.
pub const DEFAULT_VIDEO_FRAME_DURATION_US: u64 = 33_333;

// ── Default VP9 codec parameters ─────────────────────────────────────────────
//
// Shared across MoQ catalog creation and WebM muxer codec-private data.

/// VP9 profile 0 (4:2:0, 8-bit).
pub const VP9_PROFILE: u8 = 0;
/// VP9 level 1.0 (low-latency baseline).
pub const VP9_LEVEL: u8 = 10;
/// 8 bits per channel.
pub const VP9_BIT_DEPTH: u8 = 8;
/// 4:2:0 chroma subsampling (value 1 per VPCodecConfigurationRecord).
pub const VP9_CHROMA_SUBSAMPLING: u8 = 1;

// ── Default AV1 codec parameters ─────────────────────────────────────────────
//
// Shared across MoQ catalog creation for AV1 tracks.

/// AV1 Main profile (4:2:0, 8/10-bit).
pub const AV1_PROFILE: u8 = 0;
/// AV1 level 4.0.
pub const AV1_LEVEL: u8 = 8;
/// 8 bits per channel.
pub const AV1_BIT_DEPTH: u8 = 8;
/// AV1 Main tier.
pub const AV1_TIER: char = 'M';

// ── Codec content-type strings ───────────────────────────────────────────────
//
// Shared across encoder nodes, MoQ transport, and container muxers.

/// MIME-style content type for VP9-encoded video packets.
pub const VP9_CONTENT_TYPE: &str = "video/vp9";

/// MIME-style content type for AV1-encoded video packets.
pub const AV1_CONTENT_TYPE: &str = "video/av1";

/// Parse a pixel format string into a [`PixelFormat`].
///
/// Accepts `"i420"`, `"nv12"`, `"rgba8"`, or `"rgba"` (case-insensitive).
///
/// # Errors
///
/// Returns [`StreamKitError::Configuration`] if `s` is not a recognised format name.
pub fn parse_pixel_format(s: &str) -> Result<PixelFormat, StreamKitError> {
    match s.to_lowercase().as_str() {
        "i420" => Ok(PixelFormat::I420),
        "nv12" => Ok(PixelFormat::Nv12),
        "rgba8" | "rgba" => Ok(PixelFormat::Rgba8),
        other => Err(StreamKitError::Configuration(format!(
            "Unsupported pixel format '{other}'. Use 'i420', 'nv12', or 'rgba8'."
        ))),
    }
}

#[cfg(feature = "colorbars")]
pub mod colorbars;

#[cfg(feature = "compositor")]
pub mod compositor;

#[cfg(any(feature = "video", feature = "compositor"))]
pub mod pixel_ops;

#[cfg(feature = "compositor")]
pub mod pixel_convert;

#[cfg(any(feature = "vp9", feature = "av1", feature = "svt_av1"))]
pub(crate) mod encoder_trait;

// ── Shared I420→NV12 conversion helpers ──────────────────────────────────────
//
// Used by both the rav1d decoder (av1.rs) and the C dav1d decoder (dav1d.rs).

/// Raw plane pointers + strides for an I420 picture, abstracting over the
/// different picture types produced by rav1d and C dav1d.
#[cfg(any(feature = "av1", feature = "dav1d"))]
pub(super) struct I420Planes {
    pub y_ptr: *const u8,
    pub u_ptr: *const u8,
    pub v_ptr: *const u8,
    /// Luma stride in bytes (must be positive).
    pub y_stride: isize,
    /// Chroma stride in bytes (must be positive, shared by U and V).
    pub uv_stride: isize,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
}

/// Copy a single plane from a raw pointer with `src_stride` into `dst` with
/// `dst_stride`, copying `width` bytes per row for `height` rows.
///
/// # Safety
///
/// The caller must ensure `src_ptr` points to at least
/// `(height - 1) * src_stride + width` readable bytes.
///
/// # Errors
///
/// Returns an error if `src_stride` is non-positive or the destination slice
/// is too small.
#[cfg(any(feature = "av1", feature = "dav1d"))]
pub(super) fn copy_plane(
    dst: &mut [u8],
    dst_stride: usize,
    src_ptr: *const u8,
    src_stride: isize,
    width: usize,
    height: usize,
) -> Result<(), String> {
    if src_stride <= 0 {
        return Err("Invalid source stride for plane copy".to_string());
    }
    #[allow(clippy::cast_sign_loss)]
    let src_stride = src_stride as usize;

    for row in 0..height {
        // SAFETY: caller guarantees the source buffer is large enough.
        let src_row = unsafe { std::slice::from_raw_parts(src_ptr.add(row * src_stride), width) };
        let dst_start = row * dst_stride;
        let dst_end = dst_start + width;
        if dst_end > dst.len() {
            return Err("plane copy overflow".to_string());
        }
        dst[dst_start..dst_end].copy_from_slice(src_row);
    }

    Ok(())
}

/// Convert I420 planes into an NV12 [`VideoFrame`].
///
/// Copies the Y plane as-is, then interleaves the U and V planes into a
/// single UV plane.  Includes bounds-checking on strides and allocations.
///
/// # Safety
///
/// The raw pointers in `planes` must be valid for the dimensions described.
#[cfg(any(feature = "av1", feature = "dav1d"))]
pub(super) fn i420_to_nv12(
    planes: &I420Planes,
    metadata: Option<streamkit_core::types::PacketMetadata>,
    video_pool: Option<&std::sync::Arc<streamkit_core::VideoFramePool>>,
) -> Result<streamkit_core::types::VideoFrame, String> {
    use streamkit_core::types::{VideoFrame, VideoLayout};
    use streamkit_core::PooledVideoData;

    let width = planes.width;
    let height = planes.height;

    // Output layout is NV12 (Y + interleaved UV).
    let nv12_layout = VideoLayout::packed(width, height, PixelFormat::Nv12);
    let mut data = video_pool.map_or_else(
        || PooledVideoData::from_vec(vec![0u8; nv12_layout.total_bytes()]),
        |pool| pool.get(nv12_layout.total_bytes()),
    );
    let data_slice = data.as_mut_slice();

    let nv12_planes = nv12_layout.planes();
    let y_plane = nv12_planes[0];
    let uv_plane = nv12_planes[1];

    // Copy Y plane.
    copy_plane(
        &mut data_slice[y_plane.offset..y_plane.offset + y_plane.stride * y_plane.height as usize],
        y_plane.stride,
        planes.y_ptr,
        planes.y_stride,
        width as usize,
        height as usize,
    )?;

    // Interleave U + V into NV12's single UV plane.
    let chroma_w = (width as usize).div_ceil(2);
    let chroma_h = uv_plane.height as usize;

    #[allow(clippy::cast_sign_loss)]
    let chroma_stride = planes.uv_stride as usize;

    // Guard against corrupted bitstreams producing unexpected dimensions.
    if chroma_stride < chroma_w {
        return Err(format!("Chroma plane stride ({chroma_stride}) < chroma width ({chroma_w})"));
    }
    // Verify the total range we will access fits within the expected allocation
    // (stride × height).
    debug_assert!(
        chroma_h == 0
            || (chroma_h - 1) * chroma_stride + chroma_w <= chroma_stride.saturating_mul(chroma_h),
        "Chroma plane read would exceed expected allocation"
    );

    for row in 0..chroma_h {
        // SAFETY: We have verified above that stride >= chroma_w, and the
        // decoder allocates at least stride × chroma_h bytes per plane.
        let u_row =
            unsafe { std::slice::from_raw_parts(planes.u_ptr.add(row * chroma_stride), chroma_w) };
        let v_row =
            unsafe { std::slice::from_raw_parts(planes.v_ptr.add(row * chroma_stride), chroma_w) };
        let dst_start = uv_plane.offset + row * uv_plane.stride;
        for col in 0..chroma_w {
            data_slice[dst_start + col * 2] = u_row[col];
            data_slice[dst_start + col * 2 + 1] = v_row[col];
        }
    }

    VideoFrame::from_pooled(width, height, PixelFormat::Nv12, data, metadata)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "vp9")]
pub mod vp9;

#[cfg(feature = "av1")]
pub mod av1;

#[cfg(feature = "svt_av1")]
pub mod svt_av1;
#[cfg(feature = "svt_av1")]
pub mod svt_av1_ffi;

#[cfg(feature = "dav1d")]
pub mod dav1d;
#[cfg(feature = "dav1d")]
pub mod dav1d_ffi;

#[cfg(any(feature = "colorbars", feature = "compositor"))]
pub(crate) mod fonts;

// ── Shared font-rendering helpers ────────────────────────────────────────────

/// Measure the pixel dimensions a single-line text string would occupy when
/// rendered at `font_size`.  Returns `(width, height)`.
///
/// The width is the sum of advance widths.  The height uses the same baseline
/// logic as [`blit_text_rgba`] and adds enough room for descenders.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn measure_text(font: &fontdue::Font, font_size: f32, text: &str) -> (u32, u32) {
    if text.is_empty() {
        return (0, 0);
    }

    let ref_metrics = font.metrics('A', font_size);
    let baseline_y = ref_metrics.height as f32;

    let mut total_width: f32 = 0.0;
    let mut max_top: i32 = 0; // highest pixel above origin_y (always >= 0)
    let mut max_bottom: i32 = 0; // lowest pixel below origin_y

    for ch in text.chars() {
        let metrics = font.metrics(ch, font_size);

        let gy = (baseline_y - metrics.ymin as f32) as i32 - metrics.height as i32;
        let glyph_bottom = gy + metrics.height as i32;

        if gy < max_top {
            max_top = gy;
        }
        if glyph_bottom > max_bottom {
            max_bottom = glyph_bottom;
        }

        total_width += metrics.advance_width;
    }

    let w = total_width.ceil() as u32;
    let h =
        if max_bottom > max_top { (max_bottom - max_top) as u32 } else { font_size.ceil() as u32 };

    (w, h)
}

/// Alpha-blend a single text string into a packed RGBA8 buffer.
///
/// `origin_x` / `origin_y` are the top-left pixel coordinates where the first
/// glyph begins.  `color` is `[R, G, B, A]` — the alpha component modulates
/// coverage so semi-transparent text is supported.
///
/// The function clips to the buffer dimensions and stops early if the cursor
/// advances past `buf_width`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments
)]
pub fn blit_text_rgba(
    buf: &mut [u8],
    buf_width: u32,
    buf_height: u32,
    font: &fontdue::Font,
    font_size: f32,
    text: &str,
    origin_x: i32,
    origin_y: i32,
    color: [u8; 4],
) {
    let [cr, cg, cb, ca] = color;
    let stride = buf_width as usize * 4;

    // Establish baseline from a reference glyph.
    let (ref_metrics, _) = font.rasterize('A', font_size);
    let baseline_y = ref_metrics.height as f32;

    let mut cursor_x: f32 = 0.0;

    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size);

        let gx = origin_x + (cursor_x + metrics.xmin as f32) as i32;
        let gy = origin_y + (baseline_y - metrics.ymin as f32) as i32 - metrics.height as i32;

        for row in 0..metrics.height {
            let dst_y = gy + row as i32;
            if dst_y < 0 || dst_y >= buf_height as i32 {
                continue;
            }
            for col in 0..metrics.width {
                let dst_x = gx + col as i32;
                if dst_x < 0 || dst_x >= buf_width as i32 {
                    continue;
                }
                let coverage = bitmap[row * metrics.width + col];
                if coverage == 0 {
                    continue;
                }

                let alpha = u16::from(ca) * u16::from(coverage) / 255;
                if alpha == 0 {
                    continue;
                }
                let off = dst_y as usize * stride + dst_x as usize * 4;

                if alpha >= 255 {
                    buf[off] = cr;
                    buf[off + 1] = cg;
                    buf[off + 2] = cb;
                    buf[off + 3] = 255;
                } else {
                    let inv = 255 - alpha;
                    let dr = u16::from(buf[off]);
                    let dg = u16::from(buf[off + 1]);
                    let db = u16::from(buf[off + 2]);
                    let da = u16::from(buf[off + 3]);
                    buf[off] = ((u16::from(cr) * alpha + dr * inv + 128) / 255) as u8;
                    buf[off + 1] = ((u16::from(cg) * alpha + dg * inv + 128) / 255) as u8;
                    buf[off + 2] = ((u16::from(cb) * alpha + db * inv + 128) / 255) as u8;
                    buf[off + 3] = (alpha + (da * inv + 128) / 255).min(255) as u8;
                }
            }
        }

        cursor_x += metrics.advance_width;
        if (origin_x as f32 + cursor_x) >= buf_width as f32 {
            break;
        }
    }
}

// ── Multi-line / word-wrap helpers ────────────────────────────────────────

/// Split `text` into wrapped lines that fit within `max_width` pixels.
///
/// Explicit `\n` characters always produce a line break.  Within each
/// paragraph the text is word-wrapped (split on ASCII whitespace) so that
/// no line exceeds `max_width`.  When a single word is wider than
/// `max_width` it is placed on its own line without further splitting.
///
/// If `max_width` is 0 the text is only split on explicit newlines (no
/// word-wrapping).
#[allow(clippy::cast_precision_loss)]
fn wrap_text_lines(
    font: &fontdue::Font,
    font_size: f32,
    text: &str,
    max_width: u32,
) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split('\n').collect();

    if max_width == 0 {
        return paragraphs.iter().map(|s| (*s).to_string()).collect();
    }

    let max_w = max_width as f32;
    let space_advance = {
        let (m, _) = font.rasterize(' ', font_size);
        m.advance_width
    };

    let mut lines = Vec::new();

    for paragraph in paragraphs {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let words: Vec<&str> = paragraph.split_whitespace().collect();
        if words.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        let mut current_width: f32 = 0.0;

        for word in &words {
            let (word_w, _) = measure_text(font, font_size, word);
            let word_w_f = word_w as f32;

            if current_line.is_empty() {
                // First word on the line — always accept it.
                current_line.push_str(word);
                current_width = word_w_f;
            } else if current_width + space_advance + word_w_f <= max_w {
                // Fits on the current line.
                current_line.push(' ');
                current_line.push_str(word);
                current_width += space_advance + word_w_f;
            } else {
                // Doesn't fit — flush current line and start a new one.
                lines.push(std::mem::take(&mut current_line));
                current_line.push_str(word);
                current_width = word_w_f;
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Measure the pixel dimensions of multi-line wrapped text.
///
/// Splits the input on explicit newlines and word-wraps each paragraph to
/// fit within `max_width` pixels (see [`wrap_text_lines`]).  Returns the
/// bounding `(width, height)` of the full block of text.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn measure_text_wrapped(
    font: &fontdue::Font,
    font_size: f32,
    text: &str,
    max_width: u32,
) -> (u32, u32) {
    if text.is_empty() {
        return (0, 0);
    }

    let lines = wrap_text_lines(font, font_size, text, max_width);
    let line_height = line_height_px(font, font_size);

    let mut widest: u32 = 0;
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        let (w, _) = measure_text(font, font_size, line);
        if w > widest {
            widest = w;
        }
    }

    let total_h = (lines.len() as f32 * line_height).ceil() as u32;
    (widest, total_h)
}

/// Blit multi-line wrapped text into a packed RGBA8 buffer.
///
/// The text is split on explicit `\n` and word-wrapped to `max_width`
/// pixels (see [`wrap_text_lines`]).  Each resulting line is rendered via
/// [`blit_text_rgba`] at successive vertical offsets.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments
)]
pub fn blit_text_wrapped(
    buf: &mut [u8],
    buf_width: u32,
    buf_height: u32,
    font: &fontdue::Font,
    font_size: f32,
    text: &str,
    origin_x: i32,
    origin_y: i32,
    color: [u8; 4],
    max_width: u32,
) {
    let lines = wrap_text_lines(font, font_size, text, max_width);
    let line_height = line_height_px(font, font_size);

    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let y = origin_y + (i as f32 * line_height).round() as i32;
        blit_text_rgba(buf, buf_width, buf_height, font, font_size, line, origin_x, y, color);
    }
}

/// Compute the line height (in pixels) for a font at the given size.
///
/// Uses `font_size * 1.2`, matching CSS `line-height: 1.2` used in the
/// UI's `CompositorCanvas`.  The previous implementation used the
/// rasterised glyph height of 'A' (which is smaller than font_size),
/// producing tighter spacing than the CSS preview showed.
#[allow(clippy::cast_precision_loss)]
fn line_height_px(_font: &fontdue::Font, font_size: f32) -> f32 {
    font_size * 1.2
}

/// Registers all available video nodes with the engine's registry.
#[allow(clippy::missing_const_for_fn)]
pub fn register_video_nodes(registry: &mut NodeRegistry, constraints: &GlobalNodeConstraints) {
    let _ = constraints;

    #[cfg(feature = "colorbars")]
    colorbars::register_colorbars_nodes(registry);

    #[cfg(feature = "compositor")]
    compositor::register_compositor_nodes(registry, constraints);

    #[cfg(feature = "compositor")]
    pixel_convert::register_pixel_convert_nodes(registry);

    #[cfg(feature = "vp9")]
    vp9::register_vp9_nodes(registry);

    #[cfg(feature = "av1")]
    av1::register_av1_nodes(registry);

    #[cfg(feature = "svt_av1")]
    svt_av1::register_svt_av1_nodes(registry);

    #[cfg(feature = "dav1d")]
    dav1d::register_dav1d_nodes(registry);
}
