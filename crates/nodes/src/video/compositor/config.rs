// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration types for the video compositor node.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::HashMap;

// ── Configuration ───────────────────────────────────────────────────────────

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
    /// Clip to an ellipse inscribed in the destination rectangle.
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
#[derive(Deserialize, Debug, Clone, Copy, JsonSchema)]
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
#[derive(Deserialize, Debug, Clone, JsonSchema)]
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
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ImageOverlayConfig {
    /// Stable unique identifier.  Auto-generated (UUID v4) when omitted.
    #[serde(default = "generate_overlay_id")]
    pub id: String,
    /// Base64-encoded image data (PNG or JPEG). Decoded once during
    /// initialization, not per-frame.
    pub data_base64: String,
    /// Spatial and visual properties (rect, opacity, rotation, z_index).
    #[serde(flatten)]
    pub transform: OverlayTransform,
}

/// Configuration for a text overlay (rasterized once per `UpdateParams`).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
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
    /// Optional filesystem path to a TTF/OTF font file.
    /// Use this for external or system-installed fonts not in the bundled set.
    /// When omitted, a bundled default font (DejaVu Sans) is used.
    #[serde(default)]
    pub font_path: Option<String>,
    /// Optional base64-encoded TTF/OTF font data.
    /// Takes precedence over `font_path` when both are provided.
    #[serde(default)]
    pub font_data_base64: Option<String>,
    /// Named font from the bundled set (embedded in the binary at compile
    /// time — guaranteed to work without system font packages).
    /// Takes precedence over `font_path` but not `font_data_base64`.
    /// Available names: "dejavu-sans", "dejavu-sans-bold",
    /// "dejavu-sans-mono", "dejavu-sans-mono-bold",
    /// "dejavu-serif", "dejavu-serif-bold".
    #[serde(default)]
    pub font_name: Option<String>,
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

/// Layer configuration for a single compositing input.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct LayerConfig {
    /// Destination rectangle on the output canvas. If `None`, the input is
    /// scaled to fill the entire canvas.
    pub rect: Option<Rect>,
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
#[serde(default)]
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
        }
    }
}

// ── Server-computed layout types ─────────────────────────────────────────
// These are emitted via the view data channel so the frontend can render
// overlays / layers at server-computed positions (server is source of truth
// in Monitor view).

/// Server-computed layout for a single video layer.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS))]
pub struct ResolvedLayer {
    /// Pin name (e.g. `"in_0"`).
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub opacity: f32,
    pub z_index: i32,
    pub rotation_degrees: f32,
    pub mirror_horizontal: bool,
    pub mirror_vertical: bool,
    /// Crop zoom factor (1.0 = full source).
    pub crop_zoom: f32,
    /// Normalized crop pan X (0.0–1.0).
    pub crop_x: f32,
    /// Normalized crop tilt Y (0.0–1.0).
    pub crop_y: f32,
    /// Shape clipping applied to the layer.
    pub crop_shape: CropShape,
}

/// Server-computed layout for a single overlay (text or image).
#[derive(Serialize, Clone, Debug, PartialEq)]
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
    pub opacity: f32,
    pub z_index: i32,
    pub rotation_degrees: f32,
    pub mirror_horizontal: bool,
    pub mirror_vertical: bool,
    /// Actual text width measured by the font engine (text overlays only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_text_width: Option<u32>,
    /// Actual text height measured by the font engine (text overlays only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_text_height: Option<u32>,
}

/// The complete server-computed compositor layout, serialized as view data.
#[derive(Serialize, Clone, Debug, PartialEq)]
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
        Ok(())
    }
}
