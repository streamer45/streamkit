// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration types for the video compositor node.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::HashMap;

// ── Configuration ───────────────────────────────────────────────────────────

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
#[derive(Deserialize, Debug, Clone, JsonSchema)]
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
pub struct ImageOverlayConfig {
    /// Base64-encoded image data (PNG or JPEG). Decoded once during
    /// initialization, not per-frame.
    pub data_base64: String,
    /// Spatial and visual properties (rect, opacity, rotation, z_index).
    #[serde(flatten)]
    pub transform: OverlayTransform,
}

/// Configuration for a text overlay (rasterized once per `UpdateParams`).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct TextOverlayConfig {
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

const fn default_text_color() -> [u8; 4] {
    [255, 255, 255, 255]
}

const fn default_font_size() -> u32 {
    24
}

/// Layer configuration for a single compositing input.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
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
        }
    }
}

/// Configuration for the video compositor node.
///
/// The compositor supports an arbitrary number of dynamic video inputs
/// (created at runtime via `PinManagementMessage`) plus static image/text
/// overlays configured here.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
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
pub struct ResolvedLayer {
    /// Pin name (e.g. "in_0").
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
}

/// Server-computed layout for a single overlay (text or image).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ResolvedOverlay {
    pub index: usize,
    pub x: i32,
    pub y: i32,
    /// Resolved width after text wrapping / image aspect-fit.
    pub width: u32,
    /// Resolved height after text wrapping / image aspect-fit.
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
pub struct CompositorLayout {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub layers: SmallVec<[ResolvedLayer; 8]>,
    pub text_overlays: SmallVec<[ResolvedOverlay; 8]>,
    pub image_overlays: SmallVec<[ResolvedOverlay; 8]>,
}

/// Check that an opacity value is a finite number in `[0.0, 1.0]`.
fn validate_opacity(value: f32, label: &str) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} opacity must be in [0.0, 1.0]"));
    }
    Ok(())
}

impl CompositorConfig {
    /// Validate compositor parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if width/height are zero or if opacity values
    /// are out of range.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("Canvas width and height must be > 0".to_string());
        }
        if self.fps == 0 {
            return Err("Output fps must be > 0".to_string());
        }
        for (name, layer) in &self.layers {
            validate_opacity(layer.opacity, &format!("Layer '{name}'"))?;
        }
        for (i, img) in self.image_overlays.iter().enumerate() {
            validate_opacity(img.transform.opacity, &format!("Image overlay {i}"))?;
        }
        for (i, txt) in self.text_overlays.iter().enumerate() {
            validate_opacity(txt.transform.opacity, &format!("Text overlay {i}"))?;
        }
        Ok(())
    }
}
