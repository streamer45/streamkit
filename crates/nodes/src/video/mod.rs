// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video nodes and registration.

use streamkit_core::types::PixelFormat;
use streamkit_core::{NodeRegistry, StreamKitError};

/// Default video frame duration in microseconds (~30 fps).
///
/// Used as a fallback when incoming packets carry no duration metadata.
/// Shared across WebM muxing and MoQ transport.
pub const DEFAULT_VIDEO_FRAME_DURATION_US: u64 = 33_333;

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

#[cfg(feature = "vp9")]
pub mod vp9;

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

    let (ref_metrics, _) = font.rasterize('A', font_size);
    let baseline_y = ref_metrics.height as f32;

    let mut total_width: f32 = 0.0;
    let mut max_top: i32 = 0; // highest pixel above origin_y (always >= 0)
    let mut max_bottom: i32 = 0; // lowest pixel below origin_y

    for ch in text.chars() {
        let (metrics, _) = font.rasterize(ch, font_size);

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

/// Registers all available video nodes with the engine's registry.
#[allow(clippy::missing_const_for_fn)]
pub fn register_video_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "colorbars")]
    colorbars::register_colorbars_nodes(registry);

    #[cfg(feature = "compositor")]
    compositor::register_compositor_nodes(registry);

    #[cfg(feature = "vp9")]
    vp9::register_vp9_nodes(registry);
}
