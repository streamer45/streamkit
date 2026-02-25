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

    if let Some(font) = font {
        // ── Real font rendering via shared utility ───────────────────────
        crate::video::blit_text_rgba(
            &mut rgba_data,
            w,
            h,
            &font,
            config.font_size.max(1) as f32,
            &config.text,
            0,
            0,
            config.color,
        );
    } else {
        // ── Fallback: filled rectangle per glyph (placeholder) ──────────
        let [cr, cg, cb, ca] = config.color;
        let stride = w as usize * 4;
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
