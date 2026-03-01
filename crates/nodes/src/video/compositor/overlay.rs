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

    let target_w = config.rect.width;
    let target_h = config.rect.height;

    // Pre-scale the decoded image to the target rect dimensions so that
    // the per-frame `blit_overlay` → `scale_blit_rgba` call hits the
    // identity-scale fast path (direct memcpy) instead of doing
    // nearest-neighbor scaling every frame.
    if target_w > 0 && target_h > 0 && (w != target_w || h != target_h) {
        let raw = rgba.into_raw();
        let scaled = prescale_rgba(&raw, w, h, target_w, target_h);
        Ok(DecodedOverlay {
            rgba_data: scaled,
            width: target_w,
            height: target_h,
            rect: config.rect.clone(),
            opacity: config.opacity,
        })
    } else {
        Ok(DecodedOverlay {
            rgba_data: rgba.into_raw(),
            width: w,
            height: h,
            rect: config.rect.clone(),
            opacity: config.opacity,
        })
    }
}

/// Nearest-neighbor scale an RGBA8 buffer from `(sw, sh)` to `(dw, dh)`.
/// Used once at config time so the per-frame blit is a 1:1 copy.
fn prescale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let sw = sw as usize;
    let sh = sh as usize;
    let dw = dw as usize;
    let dh = dh as usize;
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        let sy = dy * sh / dh;
        for dx in 0..dw {
            let sx = dx * sw / dw;
            let si = (sy * sw + sx) * 4;
            let di = (dy * dw + dx) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
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
