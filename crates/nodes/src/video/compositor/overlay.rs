// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Overlay decoding and rasterization for the video compositor.

use super::config::{ImageOverlayConfig, Rect, TextOverlayConfig};
use streamkit_core::StreamKitError;

// ── Decoded overlay bitmap ──────────────────────────────────────────────────

/// A pre-decoded RGBA bitmap overlay ready for per-frame blitting.
#[derive(Clone)]
pub struct DecodedOverlay {
    pub rgba_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub rect: Rect,
    pub opacity: f32,
}

/// Decode a base64-encoded image (PNG/JPEG) into an RGBA8 bitmap.
pub fn decode_image_overlay(config: &ImageOverlayConfig) -> Result<DecodedOverlay, StreamKitError> {
    use image::GenericImageView;

    use base64::Engine;
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(&config.data_base64).map_err(|e| {
            StreamKitError::Configuration(format!("Invalid base64 in image overlay: {e}"))
        })?;

    let img = image::load_from_memory(&bytes).map_err(|e| {
        StreamKitError::Configuration(format!("Failed to decode image overlay: {e}"))
    })?;

    let rgba = img.to_rgba8();
    let (w, h) = img.dimensions();

    Ok(DecodedOverlay {
        rgba_data: rgba.into_raw(),
        width: w,
        height: h,
        rect: config.rect.clone(),
        opacity: config.opacity,
    })
}

// ── Bundled default font ────────────────────────────────────────────────────

/// Path to the system DejaVu Sans font (commonly available on Linux).
const DEJAVU_SANS_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";

/// Load a font from a filesystem path.
///
/// Use this to pre-load a font once and then call
/// [`rasterize_text_with_font`] per-frame without re-reading the file.
pub fn load_font_from_path(path: &str) -> Result<fontdue::Font, String> {
    let font_bytes =
        std::fs::read(path).map_err(|e| format!("Failed to read font file '{path}': {e}"))?;
    fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
        .map_err(|e| format!("Failed to parse font: {e}"))
}

/// Load font data, trying (in order):
/// 1. `font_data_base64` (inline base64-encoded TTF/OTF)
/// 2. `font_path` (filesystem path)
/// 3. Bundled system default (`DejaVuSans.ttf`)
fn load_font(config: &TextOverlayConfig) -> Result<fontdue::Font, String> {
    let font_bytes: Vec<u8> = if let Some(ref b64) = config.font_data_base64 {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Invalid base64 in font_data_base64: {e}"))?
    } else if let Some(ref path) = config.font_path {
        std::fs::read(path).map_err(|e| format!("Failed to read font file '{path}': {e}"))?
    } else {
        std::fs::read(DEJAVU_SANS_PATH)
            .map_err(|e| format!("Failed to read default font '{DEJAVU_SANS_PATH}': {e}"))?
    };

    fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
        .map_err(|e| format!("Failed to parse font: {e}"))
}

/// Rasterize a text overlay into an RGBA8 bitmap using `fontdue` for real
/// font glyph rendering.  Falls back to solid-rectangle placeholders when
/// font loading fails so the node keeps running.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn rasterize_text_overlay(config: &TextOverlayConfig) -> DecodedOverlay {
    let w = config.rect.width.max(1);
    let h = config.rect.height.max(1);

    // Attempt to load the font; fall back to rectangle placeholders on error.
    let font = match load_font(config) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("Font loading failed, using placeholder rectangles: {e}");
            None
        },
    };

    let total_bytes = (w as usize) * (h as usize) * 4;
    let mut rgba_data = vec![0u8; total_bytes];
    let stride = w as usize * 4;

    let font_size_f = config.font_size.max(1) as f32;
    let [cr, cg, cb, ca] = config.color;

    if let Some(font) = font {
        // ── Real font rendering via fontdue ──────────────────────────────
        let mut cursor_x: f32 = 0.0;

        // Use a capital letter to establish the baseline position.
        let (ref_metrics, _) = font.rasterize('A', font_size_f);
        let baseline_y = ref_metrics.height as f32;

        for ch in config.text.chars() {
            let (metrics, bitmap) = font.rasterize(ch, font_size_f);

            // Glyph origin in the overlay buffer.
            let gx = (cursor_x + metrics.xmin as f32) as i32;
            let gy = (baseline_y - metrics.ymin as f32) as i32 - metrics.height as i32;

            // Blit coverage bitmap into the RGBA overlay buffer.
            for row in 0..metrics.height {
                let dst_y = gy + row as i32;
                if dst_y < 0 || dst_y >= h as i32 {
                    continue;
                }
                for col in 0..metrics.width {
                    let dst_x = gx + col as i32;
                    if dst_x < 0 || dst_x >= w as i32 {
                        continue;
                    }
                    let coverage = bitmap[row * metrics.width + col];
                    if coverage == 0 {
                        continue;
                    }
                    let alpha = u16::from(ca) * u16::from(coverage) / 255;
                    let off = dst_y as usize * stride + dst_x as usize * 4;

                    if alpha >= 255 {
                        rgba_data[off] = cr;
                        rgba_data[off + 1] = cg;
                        rgba_data[off + 2] = cb;
                        rgba_data[off + 3] = 255;
                    } else if alpha > 0 {
                        let inv = 255 - alpha;
                        let dr = u16::from(rgba_data[off]);
                        let dg = u16::from(rgba_data[off + 1]);
                        let db = u16::from(rgba_data[off + 2]);
                        let da = u16::from(rgba_data[off + 3]);
                        rgba_data[off] = ((u16::from(cr) * alpha + dr * inv + 128) / 255) as u8;
                        rgba_data[off + 1] = ((u16::from(cg) * alpha + dg * inv + 128) / 255) as u8;
                        rgba_data[off + 2] = ((u16::from(cb) * alpha + db * inv + 128) / 255) as u8;
                        rgba_data[off + 3] = (alpha + (da * inv + 128) / 255).min(255) as u8;
                    }
                }
            }

            cursor_x += metrics.advance_width;
            if cursor_x >= w as f32 {
                break;
            }
        }
    } else {
        // ── Fallback: filled rectangle per glyph (placeholder) ──────────
        let glyph_w = (config.font_size.max(1) * 3 / 5) as usize;
        let glyph_h = config.font_size.max(1) as usize;

        for (i, _ch) in config.text.chars().enumerate() {
            let x = i * glyph_w;
            if x + glyph_w > w as usize {
                break;
            }
            for row in 0..glyph_h.min(h as usize) {
                for col in x..x + glyph_w {
                    let off = row * stride + col * 4;
                    rgba_data[off] = cr;
                    rgba_data[off + 1] = cg;
                    rgba_data[off + 2] = cb;
                    rgba_data[off + 3] = ca;
                }
            }
        }
    }

    DecodedOverlay {
        rgba_data,
        width: w,
        height: h,
        rect: config.rect.clone(),
        opacity: config.opacity,
    }
}

/// Rasterize a text string into a `DecodedOverlay` using a pre-loaded font.
///
/// Unlike [`rasterize_text_overlay`], this avoids re-reading the font file
/// from disk on every call, making it suitable for per-frame rendering
/// (e.g. the `draw_time` clock overlay).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn rasterize_text_with_font(
    text: &str,
    rect: &Rect,
    color: [u8; 4],
    font_size: u32,
    opacity: f32,
    font: &fontdue::Font,
) -> DecodedOverlay {
    let w = rect.width.max(1);
    let h = rect.height.max(1);

    let total_bytes = (w as usize) * (h as usize) * 4;
    let mut rgba_data = vec![0u8; total_bytes];
    let stride = w as usize * 4;

    let font_size_f = font_size.max(1) as f32;
    let [cr, cg, cb, ca] = color;

    let mut cursor_x: f32 = 0.0;

    // Use a capital letter to establish the baseline position.
    let (ref_metrics, _) = font.rasterize('A', font_size_f);
    let baseline_y = ref_metrics.height as f32;

    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size_f);

        let gx = (cursor_x + metrics.xmin as f32) as i32;
        let gy = (baseline_y - metrics.ymin as f32) as i32 - metrics.height as i32;

        for row in 0..metrics.height {
            let dst_y = gy + row as i32;
            if dst_y < 0 || dst_y >= h as i32 {
                continue;
            }
            for col in 0..metrics.width {
                let dst_x = gx + col as i32;
                if dst_x < 0 || dst_x >= w as i32 {
                    continue;
                }
                let coverage = bitmap[row * metrics.width + col];
                if coverage == 0 {
                    continue;
                }
                let alpha = u16::from(ca) * u16::from(coverage) / 255;
                let off = dst_y as usize * stride + dst_x as usize * 4;

                if alpha >= 255 {
                    rgba_data[off] = cr;
                    rgba_data[off + 1] = cg;
                    rgba_data[off + 2] = cb;
                    rgba_data[off + 3] = 255;
                } else if alpha > 0 {
                    let inv = 255 - alpha;
                    let dr = u16::from(rgba_data[off]);
                    let dg = u16::from(rgba_data[off + 1]);
                    let db = u16::from(rgba_data[off + 2]);
                    let da = u16::from(rgba_data[off + 3]);
                    rgba_data[off] = ((u16::from(cr) * alpha + dr * inv + 128) / 255) as u8;
                    rgba_data[off + 1] = ((u16::from(cg) * alpha + dg * inv + 128) / 255) as u8;
                    rgba_data[off + 2] = ((u16::from(cb) * alpha + db * inv + 128) / 255) as u8;
                    rgba_data[off + 3] = (alpha + (da * inv + 128) / 255).min(255) as u8;
                }
            }
        }

        cursor_x += metrics.advance_width;
        if cursor_x >= w as f32 {
            break;
        }
    }

    DecodedOverlay {
        rgba_data,
        width: w,
        height: h,
        rect: rect.clone(),
        opacity,
    }
}
