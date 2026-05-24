// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration types for the video compositor node.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::HashMap;

/// Shape used to clip a composited layer.
///
/// `Rect` (the default) renders the layer as-is within its destination
/// rectangle.  `Circle` clips to an ellipse inscribed in the destination
/// rect — when the rect is square this produces a perfect circle, ideal
/// for Loom-style webcam PIP overlays.
///
/// New variants (e.g. `RoundedRect`, `Hexagon`) can be added in the
/// future.  The field-level `#[serde(default)]` on `LayerConfig` means a
/// missing `crop_shape` key defaults to `Rect`.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CropShape {
    /// No shape clipping — the layer fills its destination rectangle.
    #[default]
    Rect,
    /// Clip to a circle inscribed in the shorter side of the destination
    /// rectangle.  The circle is always a true circle (never an ellipse),
    /// centred within the rect.
    Circle,
}

const fn default_width() -> u32 {
    1280
}

const fn default_height() -> u32 {
    720
}

const fn default_fps() -> u32 {
    30
}

/// Pixel-space rectangle for positioning a layer on the output canvas.
///
/// `x` and `y` are signed to allow off-screen positioning (e.g. for
/// slide-in effects or rotation around the rect centre).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Common spatial and visual properties shared by all overlay types.
///
/// Flattened into each overlay config via `#[serde(flatten)]` so the JSON
/// shape stays identical (fields remain at the top level).
#[derive(Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct OverlayTransform {
    /// Destination rectangle on the output canvas.
    pub rect: Rect,
    /// Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Clockwise rotation in degrees around the rect centre.  Default 0.0.
    #[serde(default)]
    pub rotation_degrees: f32,
    /// Visual stacking order.  Lower values are drawn first (bottom);
    /// higher values are drawn on top.  Default 0.
    #[serde(default = "default_z_index")]
    pub z_index: i32,
    /// Mirror the layer horizontally (flip left ↔ right).  Default `false`.
    #[serde(default)]
    pub mirror_horizontal: bool,
    /// Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
    #[serde(default)]
    pub mirror_vertical: bool,
}

impl Default for OverlayTransform {
    fn default() -> Self {
        Self {
            rect: Rect { x: 0, y: 0, width: 0, height: 0 },
            opacity: default_opacity(),
            rotation_degrees: 0.0,
            z_index: default_z_index(),
            mirror_horizontal: false,
            mirror_vertical: false,
        }
    }
}

/// Configuration for a static image overlay (decoded once at init).
///
/// Note: `deny_unknown_fields` is intentionally omitted here because
/// `#[serde(flatten)]` on `transform` is incompatible with it — serde
/// cannot distinguish "unknown" fields from flattened struct fields.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ImageOverlayConfig {
    /// Stable unique identifier.  Auto-generated (UUID v4) when omitted.
    #[serde(default = "generate_overlay_id")]
    pub id: String,
    /// Server-relative path to an uploaded image asset
    /// (e.g. `samples/images/user/logo.png`).
    pub asset_path: String,
    /// Spatial and visual properties (rect, opacity, rotation, z_index).
    #[serde(flatten)]
    pub transform: OverlayTransform,
}

/// Configuration for a text overlay (rasterized once per `UpdateParams`).
///
/// Note: `deny_unknown_fields` is intentionally omitted here because
/// `#[serde(flatten)]` on `transform` is incompatible with it — serde
/// cannot distinguish "unknown" fields from flattened struct fields.
#[derive(Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct TextOverlayConfig {
    /// Stable unique identifier.  Auto-generated (UUID v4) when omitted.
    #[serde(default = "generate_overlay_id")]
    pub id: String,
    /// The text string to render.
    pub text: String,
    /// Spatial and visual properties (rect, opacity, rotation, z_index).
    #[serde(flatten)]
    pub transform: OverlayTransform,
    /// RGBA colour, e.g. `[255, 255, 255, 255]`.
    #[serde(default = "default_text_color")]
    pub color: [u8; 4],
    /// Font size in pixels.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// Font identifier: a font asset path under `samples/fonts/`, e.g.
    /// `"samples/fonts/system/DejaVuSans.ttf"` or
    /// `"samples/fonts/system/Inter.ttf"`.
    ///
    /// Font assets are TTF/OTF files managed via the `/api/v1/assets/fonts`
    /// REST API and stored under `samples/fonts/{system,user}/`.
    ///
    /// When omitted, the default system font (DejaVu Sans) is used.
    #[serde(default)]
    pub font_name: Option<String>,
    /// Enable word wrapping within the overlay's bounding rectangle.
    ///
    /// When `true`, text is wrapped at the width specified by
    /// `transform.rect.width`.  When `false` (the default), text only
    /// breaks on explicit newlines — matching the historical behaviour.
    #[serde(default)]
    pub word_wrap: bool,
}

pub(crate) const fn default_opacity() -> f32 {
    1.0
}

pub(crate) const fn default_z_index() -> i32 {
    0
}

/// Default crop zoom (1.0 = no zoom, full source frame visible).
pub(crate) const fn default_crop_zoom() -> f32 {
    1.0
}

/// Default crop centre (0.5 = centred on both axes).
pub(crate) const fn default_crop_center() -> f32 {
    0.5
}

const fn default_text_color() -> [u8; 4] {
    [255, 255, 255, 255]
}

const fn default_font_size() -> u32 {
    24
}

/// Generate a random UUID v4 string for overlay identity.
///
/// Used as the serde default for `TextOverlayConfig::id` and
/// `ImageOverlayConfig::id`.  Callers that send repeated `UpdateParams`
/// should include an explicit `id` to ensure the image-overlay decode
/// cache can match across updates; omitting it causes a fresh UUID on
/// every deserialization, defeating the cache.
fn generate_overlay_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

const fn default_aspect_fit() -> bool {
    true
}

/// Layer configuration for a single compositing input.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct LayerConfig {
    /// Destination rectangle on the output canvas. If `None`, the input is
    /// scaled to fill the entire canvas.
    pub rect: Option<Rect>,
    /// When `true` (the default), the source is fitted within the
    /// destination rect while preserving its native aspect ratio
    /// (letterbox / pillarbox).  Set to `false` to stretch the source
    /// to fill the rect exactly.
    #[serde(default = "default_aspect_fit")]
    pub aspect_fit: bool,
    /// Opacity (0.0 .. 1.0). Default 1.0.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Visual stacking order.  Lower values are drawn first (bottom);
    /// higher values are drawn on top.  Ties are broken by slot index
    /// (pin insertion order).  Default 0.
    #[serde(default = "default_z_index")]
    pub z_index: i32,
    /// Clockwise rotation in degrees.  Default 0.0 (no rotation).
    /// The layer is rotated around its destination rect centre.
    #[serde(default)]
    pub rotation_degrees: f32,
    /// Mirror the layer horizontally (flip left ↔ right).  Default `false`.
    #[serde(default)]
    pub mirror_horizontal: bool,
    /// Mirror the layer vertically (flip top ↔ bottom).  Default `false`.
    #[serde(default)]
    pub mirror_vertical: bool,
    /// Zoom factor for virtual PTZ crop (1.0 = full source, 2.0 = 2× zoom
    /// showing the central 50% of the source).  Default 1.0.
    #[serde(default = "default_crop_zoom")]
    pub crop_zoom: f32,
    /// Normalized horizontal pan position for the crop window
    /// (0.0 = left edge, 0.5 = centred, 1.0 = right edge).  Only has a
    /// visible effect when `crop_zoom > 1.0`.  Default 0.5.
    #[serde(default = "default_crop_center")]
    pub crop_x: f32,
    /// Normalized vertical tilt position for the crop window
    /// (0.0 = top edge, 0.5 = centred, 1.0 = bottom edge).  Only has a
    /// visible effect when `crop_zoom > 1.0`.  Default 0.5.
    #[serde(default = "default_crop_center")]
    pub crop_y: f32,
    /// Shape clipping applied to the layer.  Default `Rect` (no clipping).
    /// Set to `Circle` for Loom-style circular webcam PIP overlays.
    #[serde(default)]
    pub crop_shape: CropShape,
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self {
            rect: None,
            aspect_fit: default_aspect_fit(),
            opacity: default_opacity(),
            z_index: default_z_index(),
            rotation_degrees: 0.0,
            mirror_horizontal: false,
            mirror_vertical: false,
            crop_zoom: default_crop_zoom(),
            crop_x: default_crop_center(),
            crop_y: default_crop_center(),
            crop_shape: CropShape::default(),
        }
    }
}

/// Configuration for the video compositor node.
///
/// The compositor supports an arbitrary number of dynamic video inputs
/// (created at runtime via `PinManagementMessage`) plus static image/text
/// overlays configured here.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
#[serde(default, deny_unknown_fields)]
pub struct CompositorConfig {
    /// Output canvas width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output canvas height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Output frame rate.  The compositor ticks at this fixed rate
    /// regardless of input frame rates, compositing with the latest
    /// available frame from each input.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Number of input pins to pre-create.
    /// Required for stateless/oneshot pipelines where pins must exist before
    /// graph building. Optional for dynamic pipelines where pins are created
    /// on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.
    pub num_inputs: Option<usize>,
    /// Per-layer configuration, keyed by pin name (e.g. `"in_0"`).
    /// Layers without an entry here are scaled to fill the canvas.
    #[serde(default)]
    pub layers: HashMap<String, LayerConfig>,
    /// Static image overlays (decoded once during init).
    #[serde(default)]
    pub image_overlays: Vec<ImageOverlayConfig>,
    /// Text overlays (rasterized once per `UpdateParams`).
    #[serde(default)]
    pub text_overlays: Vec<TextOverlayConfig>,
    /// Optional output pixel format conversion.  When set to `"nv12"` or
    /// `"i420"`, the compositor converts its RGBA8 canvas to the target
    /// format on the compositing thread while data is still cache-hot.
    /// Default: `None` (output RGBA8).
    #[serde(default)]
    pub output_format: Option<String>,
    /// GPU compositing preference.  Default `None` (treated as `"auto"`).
    /// - `"auto"` (default): probe for GPU at startup; use it when scene
    ///   complexity warrants (multi-layer, high-res, effects)
    /// - `"gpu"`: force GPU compositing for every frame (warn and fall back
    ///   to CPU if unavailable)
    /// - `"cpu"`: force CPU compositing (ignore GPU even if available)
    ///
    /// When unset or `"auto"`, the compositor initialises the GPU at startup
    /// and uses a per-frame heuristic to decide whether each frame benefits
    /// from GPU acceleration.  Simple single-layer scenes use the faster CPU
    /// memcpy path.  Set to `"cpu"` to explicitly disable GPU acceleration.
    #[serde(default)]
    pub gpu_mode: Option<String>,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            num_inputs: None,
            layers: HashMap::new(),
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
            output_format: None,
            gpu_mode: None,
        }
    }
}

// Emitted via the view data channel so the frontend can render layers /
// overlays at server-computed positions.  These structs carry ONLY values
// that the client cannot derive from config alone — primarily aspect-fit
// positions and text measurements.  Config-echo fields (opacity, rotation,
// z_index, mirror, crop) are intentionally excluded so that stale
// view-data echoes never overwrite the client's authoritative local state.

/// Server-computed geometry for a single video layer.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ResolvedLayer {
    /// Pin name (e.g. `"in_0"`).
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Source frame width (from the input slot's latest frame).
    /// The client uses this to compute aspect-fit locally for zero-latency
    /// feedback on auto-PiP layers.
    /// `None` when no frame has been received yet for this input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_width: Option<u32>,
    /// Source frame height (from the input slot's latest frame).
    /// `None` when no frame has been received yet for this input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_height: Option<u32>,
}

/// Server-computed geometry for a single overlay (text or image).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ResolvedOverlay {
    /// Stable overlay identifier (matches the config `id` field).
    pub id: String,
    pub x: i32,
    pub y: i32,
    /// Width after text measurement / image aspect-fit (may differ from
    /// the config rect when content doesn't fill it exactly).
    pub width: u32,
    /// Height after text measurement / image aspect-fit.
    pub height: u32,
    /// Actual text width measured by the font engine (text overlays only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_text_width: Option<u32>,
    /// Actual text height measured by the font engine (text overlays only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_text_height: Option<u32>,
}

/// The complete server-computed compositor layout, serialized as view data.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct CompositorLayout {
    pub canvas_width: u32,
    pub canvas_height: u32,
    #[cfg_attr(feature = "codegen", ts(as = "Vec<ResolvedLayer>"))]
    pub layers: SmallVec<[ResolvedLayer; 8]>,
    #[cfg_attr(feature = "codegen", ts(as = "Vec<ResolvedOverlay>"))]
    pub text_overlays: SmallVec<[ResolvedOverlay; 8]>,
    #[cfg_attr(feature = "codegen", ts(as = "Vec<ResolvedOverlay>"))]
    pub image_overlays: SmallVec<[ResolvedOverlay; 8]>,
}

/// Check that an opacity value is a finite number in `[0.0, 1.0]`.
fn validate_opacity(value: f32, label: &str) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} opacity must be in [0.0, 1.0]"));
    }
    Ok(())
}

/// Check that a rotation value is finite.
fn validate_rotation(value: f32, label: &str) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{label} rotation_degrees must be a finite number"));
    }
    Ok(())
}

/// Check that crop/zoom parameters are valid.
fn validate_crop(crop_zoom: f32, crop_x: f32, crop_y: f32, label: &str) -> Result<(), String> {
    if !crop_zoom.is_finite() || crop_zoom < 1.0 {
        return Err(format!("{label} crop_zoom must be >= 1.0"));
    }
    if !crop_x.is_finite() || !(0.0..=1.0).contains(&crop_x) {
        return Err(format!("{label} crop_x must be in [0.0, 1.0]"));
    }
    if !crop_y.is_finite() || !(0.0..=1.0).contains(&crop_y) {
        return Err(format!("{label} crop_y must be in [0.0, 1.0]"));
    }
    Ok(())
}

/// Default maximum canvas dimension (8K UHD).
const DEFAULT_MAX_CANVAS_DIMENSION: u32 = 7680;

/// Default maximum font size in pixels.
const DEFAULT_MAX_FONT_SIZE: u32 = 4096;

/// Default maximum text overlay string length in bytes.
const DEFAULT_MAX_TEXT_LENGTH: usize = 10_000;

/// Server-level limits for the compositor.
///
/// Configured via `skit.toml` under the `[compositor]` section.
/// These are injected at node registration time and cannot be
/// overridden by per-node config or `UpdateParams`.
#[derive(Debug, Clone)]
pub struct GlobalCompositorConfig {
    /// Maximum allowed canvas dimension (width or height) in pixels.
    pub max_canvas_dimension: u32,
    /// Maximum allowed font size for text overlays in pixels.
    pub max_font_size: u32,
    /// Maximum allowed text overlay string length in bytes.
    ///
    /// A 10 000-byte string is far more than any reasonable overlay would
    /// ever display.  Capping the input prevents runaway glyph measurement /
    /// rasterization and the corresponding memory spike.
    pub max_text_length: usize,
}

impl streamkit_core::NodeConstraint for GlobalCompositorConfig {
    fn constraint_name() -> &'static str {
        "video::compositor"
    }
}

impl Default for GlobalCompositorConfig {
    fn default() -> Self {
        Self {
            max_canvas_dimension: DEFAULT_MAX_CANVAS_DIMENSION,
            max_font_size: DEFAULT_MAX_FONT_SIZE,
            max_text_length: DEFAULT_MAX_TEXT_LENGTH,
        }
    }
}

impl CompositorConfig {
    /// Validate compositor parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if width/height are zero or if opacity values
    /// are out of range.
    pub fn validate(&self, limits: &GlobalCompositorConfig) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("Canvas width and height must be > 0".to_string());
        }
        if self.width > limits.max_canvas_dimension || self.height > limits.max_canvas_dimension {
            return Err(format!(
                "Canvas dimensions {}x{} exceed maximum {}x{}",
                self.width, self.height, limits.max_canvas_dimension, limits.max_canvas_dimension
            ));
        }
        if self.fps == 0 {
            return Err("Output fps must be > 0".to_string());
        }
        for (name, layer) in &self.layers {
            validate_opacity(layer.opacity, &format!("Layer '{name}'"))?;
            validate_rotation(layer.rotation_degrees, &format!("Layer '{name}'"))?;
            validate_crop(layer.crop_zoom, layer.crop_x, layer.crop_y, &format!("Layer '{name}'"))?;
        }
        for img in &self.image_overlays {
            let label = format!("Image overlay '{}'", img.id);
            validate_opacity(img.transform.opacity, &label)?;
            validate_rotation(img.transform.rotation_degrees, &label)?;
        }
        for txt in &self.text_overlays {
            let label = format!("Text overlay '{}'", txt.id);
            validate_opacity(txt.transform.opacity, &label)?;
            validate_rotation(txt.transform.rotation_degrees, &label)?;
            if txt.font_size > limits.max_font_size {
                return Err(format!(
                    "{label} font_size {} exceeds maximum {}",
                    txt.font_size, limits.max_font_size
                ));
            }
            if txt.text.len() > limits.max_text_length {
                return Err(format!(
                    "{label} text length {} exceeds maximum {}",
                    txt.text.len(),
                    limits.max_text_length
                ));
            }
        }
        if let Some(ref fmt) = self.output_format {
            crate::video::parse_pixel_format(fmt)
                .map_err(|e| format!("Invalid output_format: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn default_limits() -> GlobalCompositorConfig {
        GlobalCompositorConfig::default()
    }

    fn valid_config() -> CompositorConfig {
        CompositorConfig::default()
    }

    fn make_text_overlay(text: &str, font_size: u32, opacity: f32) -> TextOverlayConfig {
        TextOverlayConfig {
            id: "t1".into(),
            text: text.into(),
            transform: OverlayTransform { opacity, ..OverlayTransform::default() },
            color: default_text_color(),
            font_size,
            font_name: None,
            word_wrap: false,
        }
    }

    fn make_image_overlay(opacity: f32, rotation: f32) -> ImageOverlayConfig {
        ImageOverlayConfig {
            id: "i1".into(),
            asset_path: "test.png".into(),
            transform: OverlayTransform {
                opacity,
                rotation_degrees: rotation,
                ..OverlayTransform::default()
            },
        }
    }

    // --- Default impls ---

    #[test]
    fn compositor_config_defaults() {
        let cfg = CompositorConfig::default();
        assert_eq!(cfg.width, 1280);
        assert_eq!(cfg.height, 720);
        assert_eq!(cfg.fps, 30);
        assert!(cfg.num_inputs.is_none());
        assert!(cfg.layers.is_empty());
        assert!(cfg.image_overlays.is_empty());
        assert!(cfg.text_overlays.is_empty());
        assert!(cfg.output_format.is_none());
        assert!(cfg.gpu_mode.is_none());
    }

    #[test]
    fn layer_config_defaults() {
        let layer = LayerConfig::default();
        assert!(layer.rect.is_none());
        assert!(layer.aspect_fit);
        assert!((layer.opacity - 1.0).abs() < f32::EPSILON);
        assert_eq!(layer.z_index, 0);
        assert!((layer.rotation_degrees).abs() < f32::EPSILON);
        assert!(!layer.mirror_horizontal);
        assert!(!layer.mirror_vertical);
        assert!((layer.crop_zoom - 1.0).abs() < f32::EPSILON);
        assert!((layer.crop_x - 0.5).abs() < f32::EPSILON);
        assert!((layer.crop_y - 0.5).abs() < f32::EPSILON);
        assert_eq!(layer.crop_shape, CropShape::Rect);
    }

    #[test]
    fn overlay_transform_defaults() {
        let t = OverlayTransform::default();
        assert!((t.opacity - 1.0).abs() < f32::EPSILON);
        assert!((t.rotation_degrees).abs() < f32::EPSILON);
        assert_eq!(t.z_index, 0);
        assert!(!t.mirror_horizontal);
        assert!(!t.mirror_vertical);
    }

    #[test]
    fn global_compositor_config_defaults() {
        let g = GlobalCompositorConfig::default();
        assert_eq!(g.max_canvas_dimension, 7680);
        assert_eq!(g.max_font_size, 4096);
        assert_eq!(g.max_text_length, 10_000);
    }

    // --- validate: valid config ---

    #[test]
    fn valid_default_config_passes() {
        assert!(valid_config().validate(&default_limits()).is_ok());
    }

    // --- validate: canvas dimensions ---

    #[test]
    fn zero_width_rejected() {
        let mut cfg = valid_config();
        cfg.width = 0;
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn zero_height_rejected() {
        let mut cfg = valid_config();
        cfg.height = 0;
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn exceeding_max_dimension_rejected() {
        let limits = GlobalCompositorConfig { max_canvas_dimension: 1920, ..default_limits() };
        let mut cfg = valid_config();
        cfg.width = 1921;
        assert!(cfg.validate(&limits).is_err());
    }

    #[test]
    fn max_dimension_exact_passes() {
        let limits = GlobalCompositorConfig { max_canvas_dimension: 1920, ..default_limits() };
        let mut cfg = valid_config();
        cfg.width = 1920;
        cfg.height = 1080;
        assert!(cfg.validate(&limits).is_ok());
    }

    // --- validate: fps ---

    #[test]
    fn zero_fps_rejected() {
        let mut cfg = valid_config();
        cfg.fps = 0;
        assert!(cfg.validate(&default_limits()).is_err());
    }

    // --- validate_opacity ---

    #[test]
    fn opacity_valid_boundaries() {
        assert!(validate_opacity(0.0, "test").is_ok());
        assert!(validate_opacity(1.0, "test").is_ok());
        assert!(validate_opacity(0.5, "test").is_ok());
    }

    #[test]
    fn opacity_nan_rejected() {
        assert!(validate_opacity(f32::NAN, "test").is_err());
    }

    #[test]
    fn opacity_below_zero_rejected() {
        assert!(validate_opacity(-0.1, "test").is_err());
    }

    #[test]
    fn opacity_above_one_rejected() {
        assert!(validate_opacity(1.1, "test").is_err());
    }

    // --- validate_rotation ---

    #[test]
    fn rotation_valid_values() {
        assert!(validate_rotation(0.0, "test").is_ok());
        assert!(validate_rotation(360.0, "test").is_ok());
        assert!(validate_rotation(-180.0, "test").is_ok());
    }

    #[test]
    fn rotation_nan_rejected() {
        assert!(validate_rotation(f32::NAN, "test").is_err());
    }

    #[test]
    fn rotation_inf_rejected() {
        assert!(validate_rotation(f32::INFINITY, "test").is_err());
    }

    // --- validate_crop ---

    #[test]
    fn crop_valid_values() {
        assert!(validate_crop(1.0, 0.0, 0.0, "test").is_ok());
        assert!(validate_crop(2.0, 0.5, 0.5, "test").is_ok());
        assert!(validate_crop(1.0, 1.0, 1.0, "test").is_ok());
    }

    #[test]
    fn crop_zoom_below_one_rejected() {
        assert!(validate_crop(0.9, 0.5, 0.5, "test").is_err());
    }

    #[test]
    fn crop_x_out_of_range_rejected() {
        assert!(validate_crop(1.0, -0.1, 0.5, "test").is_err());
        assert!(validate_crop(1.0, 1.1, 0.5, "test").is_err());
    }

    #[test]
    fn crop_y_out_of_range_rejected() {
        assert!(validate_crop(1.0, 0.5, -0.1, "test").is_err());
        assert!(validate_crop(1.0, 0.5, 1.1, "test").is_err());
    }

    // --- validate: layer opacity/rotation/crop ---

    #[test]
    fn layer_invalid_opacity_rejected() {
        let mut cfg = valid_config();
        cfg.layers.insert("in_0".into(), LayerConfig { opacity: 1.5, ..LayerConfig::default() });
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn layer_invalid_rotation_rejected() {
        let mut cfg = valid_config();
        cfg.layers.insert(
            "in_0".into(),
            LayerConfig { rotation_degrees: f32::NAN, ..LayerConfig::default() },
        );
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn layer_invalid_crop_rejected() {
        let mut cfg = valid_config();
        cfg.layers.insert("in_0".into(), LayerConfig { crop_zoom: 0.5, ..LayerConfig::default() });
        assert!(cfg.validate(&default_limits()).is_err());
    }

    // --- validate: overlay opacity/rotation ---

    #[test]
    fn image_overlay_invalid_opacity_rejected() {
        let mut cfg = valid_config();
        cfg.image_overlays.push(make_image_overlay(1.5, 0.0));
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn image_overlay_invalid_rotation_rejected() {
        let mut cfg = valid_config();
        cfg.image_overlays.push(make_image_overlay(1.0, f32::INFINITY));
        assert!(cfg.validate(&default_limits()).is_err());
    }

    #[test]
    fn text_overlay_invalid_opacity_rejected() {
        let mut cfg = valid_config();
        cfg.text_overlays.push(make_text_overlay("hello", 24, f32::NAN));
        assert!(cfg.validate(&default_limits()).is_err());
    }

    // --- validate: font_size / text length ---

    #[test]
    fn font_size_exceeding_max_rejected() {
        let limits = GlobalCompositorConfig { max_font_size: 100, ..default_limits() };
        let mut cfg = valid_config();
        cfg.text_overlays.push(make_text_overlay("hello", 101, 1.0));
        assert!(cfg.validate(&limits).is_err());
    }

    #[test]
    fn font_size_at_max_passes() {
        let limits = GlobalCompositorConfig { max_font_size: 100, ..default_limits() };
        let mut cfg = valid_config();
        cfg.text_overlays.push(make_text_overlay("hello", 100, 1.0));
        assert!(cfg.validate(&limits).is_ok());
    }

    #[test]
    fn text_length_exceeding_max_rejected() {
        let limits = GlobalCompositorConfig { max_text_length: 10, ..default_limits() };
        let mut cfg = valid_config();
        cfg.text_overlays.push(make_text_overlay("this text is longer than ten bytes", 24, 1.0));
        assert!(cfg.validate(&limits).is_err());
    }

    // --- validate: output_format ---

    #[test]
    fn valid_output_format_passes() {
        let mut cfg = valid_config();
        cfg.output_format = Some("nv12".into());
        assert!(cfg.validate(&default_limits()).is_ok());
    }

    #[test]
    fn invalid_output_format_rejected() {
        let mut cfg = valid_config();
        cfg.output_format = Some("invalid_format".into());
        assert!(cfg.validate(&default_limits()).is_err());
    }
}
