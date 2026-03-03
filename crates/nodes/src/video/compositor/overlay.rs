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
    /// Clockwise rotation in degrees around the rect centre.
    pub rotation_degrees: f32,
    /// Visual stacking order for unified z-sorting with video layers.
    pub z_index: i32,
}

/// Decode a base64-encoded image (PNG/JPEG) into an RGBA8 bitmap.
///
/// # Errors
///
/// Returns an error if the base64 data is invalid or the image cannot be decoded.
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

    let target_w = config.transform.rect.width;
    let target_h = config.transform.rect.height;

    // Pre-scale the decoded image to fit within the target rect while
    // preserving the source aspect ratio.  This ensures the per-frame
    // `scale_blit_rgba_rotated` call hits the identity-scale fast path
    // (direct memcpy) and the image is never stretched.
    if target_w > 0 && target_h > 0 && (w != target_w || h != target_h) {
        // Aspect-ratio-preserving fit: scale so the image fits inside
        // the target box without distortion.
        #[allow(clippy::cast_precision_loss)]
        let scale = {
            let sw = w as f32;
            let sh = h as f32;
            (target_w as f32 / sw).min(target_h as f32 / sh)
        };
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let fit_w = ((w as f32 * scale).round() as u32).max(1);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let fit_h = ((h as f32 * scale).round() as u32).max(1);

        let raw = rgba.into_raw();
        let scaled = prescale_rgba(&raw, w, h, fit_w, fit_h);

        // Adjust the rect to match the fitted dimensions so the blit
        // stays on the identity-scale path and the image is centred
        // within the originally requested area.
        let mut rect = config.transform.rect.clone();
        rect.x += (target_w.cast_signed() - fit_w.cast_signed()) / 2;
        rect.y += (target_h.cast_signed() - fit_h.cast_signed()) / 2;
        rect.width = fit_w;
        rect.height = fit_h;

        Ok(DecodedOverlay {
            rgba_data: scaled,
            width: fit_w,
            height: fit_h,
            rect,
            opacity: config.transform.opacity,
            rotation_degrees: config.transform.rotation_degrees,
            z_index: config.transform.z_index,
        })
    } else {
        Ok(DecodedOverlay {
            rgba_data: rgba.into_raw(),
            width: w,
            height: h,
            rect: config.transform.rect.clone(),
            opacity: config.transform.opacity,
            rotation_degrees: config.transform.rotation_degrees,
            z_index: config.transform.z_index,
        })
    }
}

/// Bilinear-filtered scale of an RGBA8 buffer from `(sw, sh)` to `(dw, dh)`.
/// Uses the `image` crate's `resize` with `Triangle` (bilinear) filter for
/// high-quality prescaling — much better than nearest-neighbor for images
/// containing text or fine detail.  Called once at config time so the
/// per-frame blit is a 1:1 copy.
fn prescale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    // Invariant: caller guarantees src.len() == sw * sh * 4.
    #[allow(clippy::expect_used)]
    // from_raw only fails if buffer length != w*h*4; caller guarantees this
    let src_img = image::RgbaImage::from_raw(sw, sh, src.to_vec())
        .expect("prescale_rgba: source dimensions do not match buffer length");
    let resized = image::imageops::resize(&src_img, dw, dh, image::imageops::FilterType::Triangle);
    resized.into_raw()
}

// ── Bundled default font ────────────────────────────────────────────────────

/// Path to the system DejaVu Sans font (commonly available on Linux).
const DEJAVU_SANS_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";

/// Map of named fonts to their filesystem paths.
/// All fonts listed here are open-source and royalty-free, commonly installed
/// on Debian/Ubuntu systems via the `fonts-dejavu-core`, `fonts-liberation`,
/// and `fonts-freefont-ttf` packages.
const KNOWN_FONTS: &[(&str, &str)] = &[
    ("dejavu-sans", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
    ("dejavu-serif", "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"),
    ("dejavu-sans-mono", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
    ("dejavu-sans-bold", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
    ("dejavu-serif-bold", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf"),
    ("dejavu-sans-mono-bold", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"),
    ("liberation-sans", "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"),
    ("liberation-serif", "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf"),
    ("liberation-mono", "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf"),
    ("freesans", "/usr/share/fonts/truetype/freefont/FreeSans.ttf"),
    ("freeserif", "/usr/share/fonts/truetype/freefont/FreeSerif.ttf"),
    ("freemono", "/usr/share/fonts/truetype/freefont/FreeMono.ttf"),
];

/// Load font data, trying (in order):
/// 1. `font_data_base64` (inline base64-encoded TTF/OTF)
/// 2. `font_name` (named font from [`KNOWN_FONTS`] map)
/// 3. `font_path` (filesystem path)
/// 4. Bundled system default (`DejaVuSans.ttf`)
fn load_font(config: &TextOverlayConfig) -> Result<fontdue::Font, String> {
    let font_bytes: Vec<u8> = if let Some(ref b64) = config.font_data_base64 {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Invalid base64 in font_data_base64: {e}"))?
    } else if let Some(ref name) = config.font_name {
        let path =
            KNOWN_FONTS.iter().find(|(n, _)| *n == name.as_str()).map(|(_, p)| *p).ok_or_else(
                || format!("Unknown font name '{name}'. Available: {}", known_font_names()),
            )?;
        std::fs::read(path).map_err(|e| {
            tracing::warn!(
                "Named font '{name}' not found at '{path}': {e}. Is the font package installed?"
            );
            format!("Failed to read named font '{name}' at '{path}': {e}")
        })?
    } else if let Some(ref path) = config.font_path {
        std::fs::read(path).map_err(|e| format!("Failed to read font file '{path}': {e}"))?
    } else {
        std::fs::read(DEJAVU_SANS_PATH)
            .map_err(|e| format!("Failed to read default font '{DEJAVU_SANS_PATH}': {e}"))?
    };

    fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
        .map_err(|e| format!("Failed to parse font: {e}"))
}

/// Comma-separated list of available font names for error messages.
fn known_font_names() -> String {
    KNOWN_FONTS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// Rasterize a text overlay into an RGBA8 bitmap using `fontdue` for real
/// font glyph rendering.  Falls back to solid-rectangle placeholders when
/// font loading fails so the node keeps running.
///
/// The bitmap dimensions are expanded to fit the measured text size so that
/// neither the width nor the height clips the rendered string.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn rasterize_text_overlay(config: &TextOverlayConfig) -> DecodedOverlay {
    // Attempt to load the font; fall back to rectangle placeholders on error.
    let font = match load_font(config) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("Font loading failed, using placeholder rectangles: {e}");
            None
        },
    };

    let font_size = config.font_size.max(1) as f32;

    // Measure actual text dimensions so the bitmap is large enough to hold
    // the full rendered string without clipping.
    let (measured_w, measured_h) = font.as_ref().map_or_else(
        || {
            // Fallback estimate for placeholder rectangles.
            let glyph_w = config.font_size.max(1) * 3 / 5;
            let est_w = glyph_w * config.text.chars().count() as u32;
            let est_h = (font_size * 1.4).ceil() as u32;
            (est_w, est_h)
        },
        |f| crate::video::measure_text(f, font_size, &config.text),
    );

    let w = config.transform.rect.width.max(measured_w).max(1);
    let h = config.transform.rect.height.max(measured_h).max(1);

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
        rect: {
            // Use the expanded dimensions so the blit renders the full bitmap
            // without clipping text that exceeds the original rect.
            let mut r = config.transform.rect.clone();
            r.width = w;
            r.height = h;
            r
        },
        opacity: config.transform.opacity,
        rotation_degrees: config.transform.rotation_degrees,
        z_index: config.transform.z_index,
    }
}
