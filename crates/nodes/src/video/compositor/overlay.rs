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

/// Rasterize a text overlay into an RGBA8 bitmap using `tiny-skia`.
pub fn rasterize_text_overlay(config: &TextOverlayConfig) -> DecodedOverlay {
    let w = config.rect.width.max(1);
    let h = config.rect.height.max(1);

    // Create a tiny-skia pixmap for the overlay region.
    // Safety: w and h are both >= 1 due to .max(1) above, so this should never be None.
    // The fallback handles any edge case from tiny-skia dimension limits.
    let mut pixmap = tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| {
        #[allow(clippy::expect_used)]
        tiny_skia::Pixmap::new(1, 1).expect("1x1 pixmap should always succeed")
    });

    // Use a simple built-in glyph renderer: each character is drawn as a
    // filled rectangle of `font_size` height. This is intentionally
    // simplistic for the MVP; a proper font rasterizer can replace this
    // once a font dependency is approved.
    let font_size = config.font_size.max(1);
    let glyph_w = font_size * 3 / 5; // approximate glyph width
    let glyph_h = font_size;

    let color = tiny_skia::Color::from_rgba8(
        config.color[0],
        config.color[1],
        config.color[2],
        config.color[3],
    );

    let mut paint = tiny_skia::Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    let transform = tiny_skia::Transform::identity();

    for (i, _ch) in config.text.chars().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let x = (i as u32) * glyph_w;
        if x + glyph_w > w {
            break; // clip to rect
        }
        // Draw a filled rectangle per glyph as a placeholder.
        #[allow(clippy::cast_precision_loss)]
        if let Some(rect) =
            tiny_skia::Rect::from_xywh(x as f32, 0.0, glyph_w as f32, glyph_h as f32)
        {
            pixmap.fill_rect(rect, &paint, transform, None);
        }
    }

    let rgba_data = pixmap.data().to_vec();

    DecodedOverlay {
        rgba_data,
        width: pixmap.width(),
        height: pixmap.height(),
        rect: config.rect.clone(),
        opacity: config.opacity,
    }
}
