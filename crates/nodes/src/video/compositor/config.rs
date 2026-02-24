// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration types for the video compositor node.

use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;

// ── Configuration ───────────────────────────────────────────────────────────

const fn default_width() -> u32 {
    1280
}

const fn default_height() -> u32 {
    720
}

/// Pixel-space rectangle for positioning a layer on the output canvas.
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Configuration for a static image overlay (decoded once at init).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct ImageOverlayConfig {
    /// Base64-encoded image data (PNG or JPEG). Decoded once during
    /// initialization, not per-frame.
    pub data_base64: String,
    /// Destination rectangle on the output canvas.
    pub rect: Rect,
    /// Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

/// Configuration for a text overlay (rasterized once per `UpdateParams`).
#[derive(Deserialize, Debug, Clone, JsonSchema)]
pub struct TextOverlayConfig {
    /// The text string to render.
    pub text: String,
    /// Destination rectangle on the output canvas.
    pub rect: Rect,
    /// RGBA colour, e.g. `[255, 255, 255, 255]`.
    #[serde(default = "default_text_color")]
    pub color: [u8; 4],
    /// Font size in pixels.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
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
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self { rect: None, opacity: default_opacity(), z_index: default_z_index() }
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
    /// Number of input pins to pre-create.
    /// Required for stateless/oneshot pipelines where pins must exist before
    /// graph building. Optional for dynamic pipelines where pins are created
    /// on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.
    pub num_inputs: Option<usize>,
    /// Output pixel format: "rgba8" (default) or "i420".
    /// Use "i420" when feeding a VP9/VP8 encoder downstream.
    #[serde(default = "default_output_pixel_format")]
    pub output_pixel_format: String,
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

fn default_output_pixel_format() -> String {
    "rgba8".to_string()
}

// Re-export the shared parse_pixel_format from the parent video module.
pub(crate) use super::super::parse_pixel_format;

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            num_inputs: None,
            output_pixel_format: default_output_pixel_format(),
            layers: HashMap::new(),
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
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
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("Canvas width and height must be > 0".to_string());
        }
        for (name, layer) in &self.layers {
            if !layer.opacity.is_finite() || layer.opacity < 0.0 || layer.opacity > 1.0 {
                return Err(format!("Layer '{name}' opacity must be in [0.0, 1.0]"));
            }
        }
        for (i, img) in self.image_overlays.iter().enumerate() {
            if !img.opacity.is_finite() || img.opacity < 0.0 || img.opacity > 1.0 {
                return Err(format!("Image overlay {i} opacity must be in [0.0, 1.0]"));
            }
        }
        for (i, txt) in self.text_overlays.iter().enumerate() {
            if !txt.opacity.is_finite() || txt.opacity < 0.0 || txt.opacity > 1.0 {
                return Err(format!("Text overlay {i} opacity must be in [0.0, 1.0]"));
            }
        }
        Ok(())
    }
}
