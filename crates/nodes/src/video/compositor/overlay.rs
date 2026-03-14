// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Overlay decoding and rasterization for the video compositor.

use super::config::{ImageOverlayConfig, Rect, TextOverlayConfig};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};
use streamkit_core::StreamKitError;

// ── Decoded overlay bitmap ──────────────────────────────────────────────────

/// A pre-decoded RGBA8 bitmap overlay ready for per-frame blitting.
///
/// Created once at init time (image overlays) or on each `UpdateParams`
/// (text overlays).  Carried in `Arc` so cloning into per-frame work items
/// is a reference-count bump.
#[derive(Clone)]
pub struct DecodedOverlay {
    /// Stable identifier carried through from the config.
    pub id: String,
    pub rgba_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub rect: Rect,
    pub opacity: f32,
    /// Clockwise rotation in degrees around the rect centre.
    pub rotation_degrees: f32,
    /// Visual stacking order for unified z-sorting with video layers.
    pub z_index: i32,
    /// Mirror horizontally (flip left ↔ right).
    pub mirror_horizontal: bool,
    /// Mirror vertically (flip top ↔ bottom).
    pub mirror_vertical: bool,
    /// Actual text width measured by the font engine (text overlays only).
    pub measured_text_width: Option<u32>,
    /// Actual text height measured by the font engine (text overlays only).
    pub measured_text_height: Option<u32>,
}

/// Decode a base64-encoded image (PNG/JPEG) into an RGBA8 bitmap.
///
/// The decoded image is pre-scaled (bilinear filter) to fit within the
/// config's target rect while preserving aspect ratio, so the per-frame
/// blit hits the identity-scale fast path (direct memcpy).  The returned
/// [`DecodedOverlay::rect`] is adjusted to centre the fitted image within
/// the originally requested area.
///
/// # Errors
///
/// Returns an error if the base64 data is invalid or the image cannot be
/// decoded.
pub fn decode_image_overlay(
    config: &ImageOverlayConfig,
    max_dimension: u32,
) -> Result<DecodedOverlay, StreamKitError> {
    use image::GenericImageView;

    use base64::Engine;
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(&config.data_base64).map_err(|e| {
            StreamKitError::Configuration(format!("Invalid base64 in image overlay: {e}"))
        })?;

    let img = image::load_from_memory(&bytes).map_err(|e| {
        StreamKitError::Configuration(format!("Failed to decode image overlay: {e}"))
    })?;

    let (w, h) = img.dimensions();
    if w > max_dimension || h > max_dimension {
        return Err(StreamKitError::Configuration(format!(
            "Decoded image dimensions {w}x{h} exceed the maximum allowed \
             dimension ({max_dimension})",
        )));
    }

    let rgba = img.to_rgba8();

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
        let mut rect = config.transform.rect;
        rect.x += (target_w.cast_signed() - fit_w.cast_signed()) / 2;
        rect.y += (target_h.cast_signed() - fit_h.cast_signed()) / 2;
        rect.width = fit_w;
        rect.height = fit_h;

        Ok(DecodedOverlay {
            id: config.id.clone(),
            rgba_data: scaled,
            width: fit_w,
            height: fit_h,
            rect,
            opacity: config.transform.opacity,
            rotation_degrees: config.transform.rotation_degrees,
            z_index: config.transform.z_index,
            mirror_horizontal: config.transform.mirror_horizontal,
            mirror_vertical: config.transform.mirror_vertical,
            measured_text_width: None,
            measured_text_height: None,
        })
    } else {
        Ok(DecodedOverlay {
            id: config.id.clone(),
            rgba_data: rgba.into_raw(),
            width: w,
            height: h,
            rect: config.transform.rect,
            opacity: config.transform.opacity,
            rotation_degrees: config.transform.rotation_degrees,
            z_index: config.transform.z_index,
            mirror_horizontal: config.transform.mirror_horizontal,
            mirror_vertical: config.transform.mirror_vertical,
            measured_text_width: None,
            measured_text_height: None,
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

// ── Bundled font data ────────────────────────────────────────────────────────

use crate::video::fonts;

// ── Parsed-font cache ───────────────────────────────────────────────────────

/// Cache key identifying a font source.
///
/// Bundled fonts are keyed by their static name.  User-provided `font_path`
/// sources use the filesystem path string.  Inline base64 data is keyed by
/// a hash of the base64 string so the cache map does not retain what may be
/// a several-hundred-KiB string per font.
#[derive(Clone, Hash, Eq, PartialEq)]
enum FontKey {
    /// A font from the compile-time bundled set (keyed by name).
    Bundled(&'static str),
    /// A user-provided font loaded from a filesystem path.
    Path(String),
    /// Inline base64-encoded font data (keyed by content hash).
    InlineHash(u64),
}

/// Maximum number of distinct fonts kept in [`FONT_CACHE`].
///
/// The 6 bundled DejaVu fonts plus a generous allowance for user-provided
/// fonts via `font_path` or `font_data_base64`.  When the limit is reached,
/// the oldest non-bundled entry is evicted (see [`load_font`]).
const FONT_CACHE_MAX_ENTRIES: usize = 64;

/// Process-wide cache of parsed `fontdue::Font` objects.
///
/// `fontdue::Font::from_bytes` parses the full TTF/OTF table set and is
/// expensive (~3.5 s cumulative in profiling when overlay parameters update
/// frequently).  Caching the parsed result keyed by font identity means the
/// parse happens once per distinct font for the lifetime of the process;
/// subsequent `load_font` calls for the same source are an `Arc::clone`.
///
/// Bounded to [`FONT_CACHE_MAX_ENTRIES`] to prevent unbounded growth when
/// users repeatedly send `UpdateParams` with unique inline font data.
/// The lock is held only for the map lookup / insert, never across
/// the parse itself.
static FONT_CACHE: LazyLock<Mutex<HashMap<FontKey, Arc<fontdue::Font>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lazy loader for raw font bytes.  Constructed cheaply by
/// [`resolve_font_source`] so that file I/O and base64 decoding are deferred
/// until after a cache miss is confirmed.
type FontBytesLoader<'a> = Box<dyn FnOnce() -> Result<Vec<u8>, String> + 'a>;

/// Resolve a [`TextOverlayConfig`]'s font-source fields to a [`FontKey`] and a
/// lazy byte loader, following the same precedence as [`load_font`]:
/// `font_data_base64` > `font_name` > `font_path` > bundled default.
///
/// Bundled fonts (via `font_name` or the default) are compiled into the binary
/// and always available — no filesystem dependency.  `font_path` still supports
/// loading arbitrary external fonts from the filesystem.
///
/// Returning a boxed closure lets the caller skip base64 decode / file I/O
/// entirely on a cache hit.
fn resolve_font_source(config: &TextOverlayConfig) -> (FontKey, FontBytesLoader<'_>) {
    if let Some(ref b64) = config.font_data_base64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b64.hash(&mut h);
        let key = FontKey::InlineHash(h.finish());
        let loader = move || {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Invalid base64 in font_data_base64: {e}"))
        };
        return (key, Box::new(loader));
    }

    if let Some(ref name) = config.font_name {
        if let Some(data) = fonts::bundled_font_by_name(name) {
            let bundled = fonts::BUNDLED_FONTS
                .iter()
                .find(|f| f.name == name.as_str())
                .map_or("dejavu-sans", |f| f.name);
            let key = FontKey::Bundled(bundled);
            let loader = move || Ok(data.to_vec());
            return (key, Box::new(loader));
        }
        // Unknown font name — fall back to the default with a warning rather
        // than erroring out, so overlays remain readable when legacy or
        // unrecognised names are passed (e.g. after removing Liberation/FreeFont).
        tracing::warn!(
            "Unknown font name '{name}', falling back to default (dejavu-sans). \
             Available: {}",
            fonts::bundled_font_names()
        );
        let key = FontKey::Bundled("dejavu-sans");
        let loader = || Ok(fonts::DEFAULT_FONT_DATA.to_vec());
        return (key, Box::new(loader));
    }

    if let Some(ref path) = config.font_path {
        let key = FontKey::Path(path.clone());
        let path = path.clone();
        let loader = move || {
            std::fs::read(&path).map_err(|e| format!("Failed to read font file '{path}': {e}"))
        };
        return (key, Box::new(loader));
    }

    // Default: embedded DejaVu Sans.
    let key = FontKey::Bundled("dejavu-sans");
    let loader = || Ok(fonts::DEFAULT_FONT_DATA.to_vec());
    (key, Box::new(loader))
}

/// Load font data, trying (in order):
/// 1. `font_data_base64` (inline base64-encoded TTF/OTF)
/// 2. `font_name` (named font from the bundled set)
/// 3. `font_path` (filesystem path for external/custom fonts)
/// 4. Bundled default (DejaVu Sans, embedded at compile time)
///
/// Parsed fonts are cached in [`FONT_CACHE`] keyed by the resolved source
/// identity, so repeated calls for the same font are an `Arc::clone` rather
/// than a fresh parse.
fn load_font(config: &TextOverlayConfig) -> Result<Arc<fontdue::Font>, String> {
    let (key, load_bytes) = resolve_font_source(config);

    // Fast path: cache hit.  Lock scope limited to the lookup.
    if let Ok(cache) = FONT_CACHE.lock() {
        if let Some(font) = cache.get(&key) {
            return Ok(Arc::clone(font));
        }
    }

    // Miss: do the expensive work (I/O + parse) *outside* the lock.
    let font_bytes = load_bytes()?;
    let font = Arc::new(
        fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .map_err(|e| format!("Failed to parse font: {e}"))?,
    );

    // Insert.  If the mutex is poisoned we simply skip caching — the caller
    // still gets a valid font, just without the memoisation benefit.
    if let Ok(mut cache) = FONT_CACHE.lock() {
        // Evict a non-bundled entry if we've hit the capacity limit.
        // Bundled fonts are never evicted since they are always available
        // and essentially free (static data, no I/O).
        if cache.len() >= FONT_CACHE_MAX_ENTRIES && !cache.contains_key(&key) {
            if let Some(evict_key) =
                cache.keys().find(|k| !matches!(k, FontKey::Bundled(_))).cloned()
            {
                cache.remove(&evict_key);
            }
        }
        cache.entry(key).or_insert_with(|| Arc::clone(&font));
    }

    Ok(font)
}

/// Rasterize a text string into an RGBA8 bitmap at the exact measured
/// text dimensions, clamped to `max_dimension` on each axis.
///
/// Uses `fontdue` for real font glyph rendering with support for explicit
/// newlines (`\n`).  Falls back to solid-rectangle placeholders when font
/// loading fails so the node keeps running.
///
/// The returned bitmap is sized to the measured text extent (not the
/// config rect), so downstream blitting renders the full string without
/// clipping or excess transparent padding.  However, neither axis will
/// exceed `max_dimension`, and if the config rect specifies non-zero
/// width/height those act as additional upper bounds.
///
/// `max_text_length` caps the input string (at a valid UTF-8 boundary)
/// before measurement/rasterization to prevent runaway glyph processing.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn rasterize_text_overlay(
    config: &TextOverlayConfig,
    max_dimension: u32,
    max_text_length: usize,
) -> DecodedOverlay {
    // Attempt to load the font; fall back to rectangle placeholders on error.
    let font = match load_font(config) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("Font loading failed, using placeholder rectangles: {e}");
            None
        },
    };

    // Truncate excessively long overlay strings to prevent unbounded
    // bitmap allocations.  The truncated text is what gets measured and
    // rasterized; the original config is not mutated.
    let text: &str = if config.text.len() > max_text_length {
        tracing::warn!(
            "Text overlay '{}' truncated from {} to {max_text_length} bytes",
            config.id,
            config.text.len(),
        );
        // Find the nearest char boundary at or before the limit so we
        // don't split a multi-byte UTF-8 sequence.
        let mut end = max_text_length;
        while !config.text.is_char_boundary(end) {
            end -= 1;
        }
        &config.text[..end]
    } else {
        &config.text
    };

    let font_size = config.font_size.max(1) as f32;
    // No word wrapping — text only breaks on explicit newlines.
    // Passing 0 tells wrap_text_lines to split on '\n' only.
    let wrap_width = 0;

    // Measure actual text dimensions so the bitmap is large enough to hold
    // the full rendered string without clipping.  When a wrap width is set
    // the text is word-wrapped and may span multiple lines.
    let (measured_w, measured_h) = font.as_ref().map_or_else(
        || {
            // Fallback estimate for placeholder rectangles.
            let glyph_w = config.font_size.max(1) * 3 / 5;
            let est_w = glyph_w * text.chars().count() as u32;
            let est_h = (font_size * 1.4).ceil() as u32;
            (est_w, est_h)
        },
        |f| crate::video::measure_text_wrapped(f, font_size, text, wrap_width),
    );

    // Use the measured text dimensions, but cap them so the bitmap never
    // exceeds the server's max_canvas_dimension.  When the config rect
    // specifies non-zero width/height, those act as additional upper bounds
    // to prevent the overlay from expanding beyond its intended area.
    let mut w = measured_w.max(1).min(max_dimension);
    let mut h = measured_h.max(1).min(max_dimension);
    if config.transform.rect.width > 0 {
        w = w.min(config.transform.rect.width);
    }
    if config.transform.rect.height > 0 {
        h = h.min(config.transform.rect.height);
    }

    let total_bytes = (w as usize) * (h as usize) * 4;
    let mut rgba_data = vec![0u8; total_bytes];

    if let Some(font) = font {
        // ── Real font rendering via shared utility (multi-line aware) ────
        crate::video::blit_text_wrapped(
            &mut rgba_data,
            w,
            h,
            &font,
            config.font_size.max(1) as f32,
            text,
            0,
            0,
            config.color,
            wrap_width,
        );
    } else {
        // ── Fallback: filled rectangle per glyph (placeholder) ──────────
        let [cr, cg, cb, ca] = config.color;
        let stride = w as usize * 4;
        let glyph_w = (config.font_size.max(1) * 3 / 5) as usize;
        let glyph_h = config.font_size.max(1) as usize;

        for (i, _ch) in text.chars().enumerate() {
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
        id: config.id.clone(),
        rgba_data,
        width: w,
        height: h,
        rect: {
            // Use the (clamped) bitmap dimensions so the blit matches the
            // allocated buffer.  The dimensions are already bounded by
            // max_dimension and the config rect above.
            let mut r = config.transform.rect;
            r.width = w;
            r.height = h;
            r
        },
        opacity: config.transform.opacity,
        rotation_degrees: config.transform.rotation_degrees,
        z_index: config.transform.z_index,
        mirror_horizontal: config.transform.mirror_horizontal,
        mirror_vertical: config.transform.mirror_vertical,
        measured_text_width: Some(measured_w),
        measured_text_height: Some(measured_h),
    }
}
