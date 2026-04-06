// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Overlay decoding and rasterization for the video compositor.

use super::config::{ImageOverlayConfig, Rect, TextOverlayConfig};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use streamkit_core::StreamKitError;

// ── Overlay source kind ─────────────────────────────────────────────────────

/// Tracks the source format of a decoded overlay for cache invalidation.
///
/// Raster overlays are pre-scaled once and reused even when dimensions change.
/// Vector overlays carry the parsed SVG tree so they can be cheaply
/// re-rasterized at new target dimensions without re-parsing the XML.
#[derive(Clone)]
pub enum OverlaySourceKind {
    /// Decoded from a raster format (PNG, JPEG, WebP, GIF).
    Raster,
    /// Decoded from an SVG.  Carries the parsed tree for re-rasterization.
    Vector { tree: Arc<resvg::usvg::Tree> },
}

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
    pub rgba_data: Arc<[u8]>,
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
    /// Source format — used by rebuild logic to decide re-rasterization strategy.
    pub source_kind: OverlaySourceKind,
}

/// Validates that an asset path is safe to read.
///
/// # Errors
///
/// Returns an error if the path contains traversal sequences or does not
/// start with `samples/images/`.
pub fn validate_asset_path(path: &str) -> Result<(), StreamKitError> {
    if path.contains("..") || !path.starts_with("samples/images/") {
        return Err(StreamKitError::Configuration(format!(
            "Invalid asset_path: must start with 'samples/images/' and not contain '..': {path}"
        )));
    }
    Ok(())
}

/// Decode an image overlay from raw bytes into an RGBA8 bitmap.
///
/// The caller is responsible for reading the file (e.g. via
/// `tokio::fs::read`) and passing the bytes here.  This keeps the
/// function synchronous and free of blocking I/O.
///
/// The decoded image is pre-scaled (bilinear filter) to fit within the
/// config's target rect while preserving aspect ratio, so the per-frame
/// blit hits the identity-scale fast path (direct memcpy).  The returned
/// [`DecodedOverlay::rect`] is adjusted to centre the fitted image within
/// the originally requested area.
///
/// # Errors
///
/// Returns an error if the image bytes cannot be decoded.
pub fn decode_image_overlay(
    config: &ImageOverlayConfig,
    bytes: &[u8],
    max_dimension: u32,
) -> Result<DecodedOverlay, StreamKitError> {
    use image::GenericImageView;

    if is_svg(bytes, &config.asset_path) {
        return rasterize_svg(config, bytes, max_dimension);
    }

    let img = image::load_from_memory(bytes).map_err(|e| {
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
            rgba_data: Arc::from(scaled),
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
            source_kind: OverlaySourceKind::Raster,
        })
    } else {
        Ok(DecodedOverlay {
            id: config.id.clone(),
            rgba_data: Arc::from(rgba.into_raw()),
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
            source_kind: OverlaySourceKind::Raster,
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

// ── SVG helpers ──────────────────────────────────────────────────────────────

/// Build `usvg::Options` with the image href resolver disabled so that
/// `<image href="file:///etc/shadow"/>` (or any other local/remote reference)
/// inside an uploaded SVG cannot trigger server-side file reads.
fn safe_svg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    }
}

/// Check whether the given bytes represent an SVG file, using both the
/// file extension and a content sniff for `<svg`.
fn is_svg(bytes: &[u8], path: &str) -> bool {
    let p = std::path::Path::new(path);
    if p.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg") || ext.eq_ignore_ascii_case("svgz"))
    {
        return true;
    }
    let prefix = &bytes[..bytes.len().min(256)];
    prefix.windows(4).any(|w| w == b"<svg")
}

/// Convert premultiplied RGBA to straight (un-associated) alpha in-place.
fn unpremultiply_alpha(data: &mut [u8]) {
    for pixel in data.chunks_exact_mut(4) {
        let a = pixel[3];
        if a > 0 && a < 255 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                let a_f = f32::from(a) / 255.0;
                pixel[0] = (f32::from(pixel[0]) / a_f).min(255.0) as u8;
                pixel[1] = (f32::from(pixel[1]) / a_f).min(255.0) as u8;
                pixel[2] = (f32::from(pixel[2]) / a_f).min(255.0) as u8;
            }
        }
    }
}

/// Render a pre-parsed SVG tree to an RGBA8 bitmap at the target rect size.
/// Aspect-ratio-preserving fit, straight-alpha RGBA8 output.
///
/// Returns `(rgba_data, width, height, adjusted_rect)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub(crate) fn rasterize_svg_tree(
    tree: &resvg::usvg::Tree,
    config: &ImageOverlayConfig,
    max_dimension: u32,
) -> Result<(Vec<u8>, u32, u32, Rect), StreamKitError> {
    let svg_size = tree.size();
    let svg_w = svg_size.width();
    let svg_h = svg_size.height();

    let target_w = config.transform.rect.width.max(1);
    let target_h = config.transform.rect.height.max(1);

    // Aspect-ratio-preserving fit within (target_w, target_h), clamped to max_dimension.
    let scale = (target_w as f32 / svg_w).min(target_h as f32 / svg_h);
    let fit_w = ((svg_w * scale).round() as u32).max(1).min(max_dimension);
    let fit_h = ((svg_h * scale).round() as u32).max(1).min(max_dimension);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(fit_w, fit_h).ok_or_else(|| {
        StreamKitError::Configuration(format!(
            "Failed to create pixmap for SVG rasterization ({fit_w}x{fit_h})"
        ))
    })?;

    let transform =
        resvg::tiny_skia::Transform::from_scale(fit_w as f32 / svg_w, fit_h as f32 / svg_h);

    resvg::render(tree, transform, &mut pixmap.as_mut());

    let mut rgba_data = pixmap.take();
    unpremultiply_alpha(&mut rgba_data);

    // Centre-adjust rect (same pattern as decode_image_overlay).
    let mut rect = config.transform.rect;
    rect.x += (target_w.cast_signed() - fit_w.cast_signed()) / 2;
    rect.y += (target_h.cast_signed() - fit_h.cast_signed()) / 2;
    rect.width = fit_w;
    rect.height = fit_h;

    Ok((rgba_data, fit_w, fit_h, rect))
}

/// Rasterize an SVG with aspect-ratio-preserving fit within `config.transform.rect`.
fn rasterize_svg(
    config: &ImageOverlayConfig,
    svg_data: &[u8],
    max_dimension: u32,
) -> Result<DecodedOverlay, StreamKitError> {
    let tree = resvg::usvg::Tree::from_data(svg_data, &safe_svg_options())
        .map_err(|e| StreamKitError::Configuration(format!("Failed to parse SVG: {e}")))?;

    let (rgba_data, w, h, rect) = rasterize_svg_tree(&tree, config, max_dimension)?;
    let tree = Arc::new(tree);

    Ok(DecodedOverlay {
        id: config.id.clone(),
        rgba_data: Arc::from(rgba_data),
        width: w,
        height: h,
        rect,
        opacity: config.transform.opacity,
        rotation_degrees: config.transform.rotation_degrees,
        z_index: config.transform.z_index,
        mirror_horizontal: config.transform.mirror_horizontal,
        mirror_vertical: config.transform.mirror_vertical,
        measured_text_width: None,
        measured_text_height: None,
        source_kind: OverlaySourceKind::Vector { tree },
    })
}

/// Extract viewBox dimensions from raw SVG data.
/// Used by skit asset pipeline without pulling resvg as a direct dependency.
pub fn svg_viewbox_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let tree = resvg::usvg::Tree::from_data(data, &safe_svg_options()).ok()?;
    let size = tree.size();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let w = (size.width().ceil() as u32).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let h = (size.height().ceil() as u32).max(1);
    Some((w, h))
}

/// Test-only re-export of [`unpremultiply_alpha`].
#[cfg(test)]
pub fn unpremultiply_alpha_for_test(data: &mut [u8]) {
    unpremultiply_alpha(data);
}

/// Test-only re-export of [`is_svg`].
#[cfg(test)]
pub fn is_svg_for_test(bytes: &[u8], path: &str) -> bool {
    is_svg(bytes, path)
}

// ── Font helpers ─────────────────────────────────────────────────────────────

use crate::video::fonts;

// ── Parsed-font cache ───────────────────────────────────────────────────────

/// Cache key identifying a font source by its asset path.
///
/// All fonts (system and user) are loaded from disk as assets under
/// `samples/fonts/`.
#[derive(Clone, Hash, Eq, PartialEq)]
struct FontKey(String);

/// Maximum number of distinct fonts kept in [`FONT_CACHE`].
///
/// A generous allowance for system + user font assets via `samples/fonts/`.
/// When the limit is reached, the oldest entry is evicted (see [`load_font`]).
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

/// Remove a font from the process-wide cache by its asset path.
///
/// Called when a font asset is deleted via the REST API so that a
/// subsequent re-upload with the same filename triggers a fresh parse
/// instead of serving stale cached data.
pub fn invalidate_font_cache_entry(path: &str) {
    if let Ok(mut cache) = FONT_CACHE.lock() {
        cache.remove(&FontKey(path.to_owned()));
    }
}

/// Lazy loader for raw font bytes.  Constructed cheaply by
/// [`resolve_font_source`] so that file I/O is deferred until after a
/// cache miss is confirmed.
type FontBytesLoader<'a> = Box<dyn FnOnce() -> Result<Vec<u8>, String> + 'a>;

/// Validates that a font asset path is safe to read.
///
/// # Errors
///
/// Returns an error string if the path contains traversal sequences or does not
/// start with `samples/fonts/`.
fn validate_font_asset_path(path: &str) -> Result<(), String> {
    if path.contains("..") || !path.starts_with("samples/fonts/") {
        return Err(format!(
            "Invalid font asset path: must start with 'samples/fonts/' and not contain '..': {path}"
        ));
    }
    Ok(())
}

/// Resolve a [`TextOverlayConfig`]'s `font_name` field to a [`FontKey`] and a
/// lazy byte loader.
///
/// Resolution order:
/// 1. If `font_name` is a valid font asset path (`samples/fonts/...`) → load
///    from filesystem.
/// 2. Unknown or invalid name → warn and fall back to the default system font
///    (DejaVu Sans at `samples/fonts/system/DejaVuSans.ttf`).
/// 3. `font_name` absent → default system font.
///
/// Returning a boxed closure lets the caller skip file I/O entirely on a cache
/// hit.
fn resolve_font_source(config: &TextOverlayConfig) -> (FontKey, FontBytesLoader<'_>) {
    if let Some(ref name) = config.font_name {
        // Check if it's a font asset path (samples/fonts/...).
        if name.starts_with("samples/fonts/") {
            if let Err(e) = validate_font_asset_path(name) {
                tracing::warn!("{e}, falling back to default system font");
                let key = FontKey(fonts::DEFAULT_FONT_PATH.to_owned());
                let loader = || fonts::load_default_font();
                return (key, Box::new(loader));
            }
            let key = FontKey(name.clone());
            let path = name.clone();
            let loader = move || {
                std::fs::read(&path).map_err(|e| format!("Failed to read font asset '{path}': {e}"))
            };
            return (key, Box::new(loader));
        }

        // Unknown font name — fall back to default with a warning.
        tracing::warn!(
            "Unknown font name '{name}', falling back to default system font (DejaVu Sans). \
             Use a font asset path like 'samples/fonts/system/Inter.ttf'."
        );
        let key = FontKey(fonts::DEFAULT_FONT_PATH.to_owned());
        let loader = || fonts::load_default_font();
        return (key, Box::new(loader));
    }

    // Default: DejaVu Sans system font asset.
    let key = FontKey(fonts::DEFAULT_FONT_PATH.to_owned());
    let loader = || fonts::load_default_font();
    (key, Box::new(loader))
}

/// Load font data from disk:
/// 1. `font_name` as a font asset path (`samples/fonts/...`)
/// 2. Default system font (DejaVu Sans at `samples/fonts/system/DejaVuSans.ttf`)
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
        // Evict an arbitrary entry if we've hit the capacity limit.
        if cache.len() >= FONT_CACHE_MAX_ENTRIES && !cache.contains_key(&key) {
            if let Some(evict_key) = cache.keys().next().cloned() {
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
        rgba_data: Arc::from(rgba_data),
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
        source_kind: OverlaySourceKind::Raster,
    }
}
