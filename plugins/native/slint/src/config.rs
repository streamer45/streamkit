// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration for the Slint video source plugin.

use std::collections::HashMap;

use serde::Deserialize;

// ── Defaults ────────────────────────────────────────────────────────────────

/// Maximum allowed dimension (width or height) — 8K.
/// Guards against config typos that would attempt multi-GB buffer allocations.
const MAX_DIMENSION: u32 = 7680;

const fn default_width() -> u32 {
    640
}

const fn default_height() -> u32 {
    480
}

const fn default_fps() -> u32 {
    30
}

const fn default_frame_count() -> u32 {
    0
}

const fn default_keyframe_interval() -> u32 {
    90
}

const fn default_static_ui() -> bool {
    false
}

// ── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the Slint UI video source plugin.
///
/// Produces RGBA8 frames by rendering a compiled `.slint` component via the
/// software renderer.  Properties can be set at init and updated at runtime
/// via `UpdateParams`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SlintConfig {
    /// Output frame width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output frame height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Output frame rate.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Path to the `.slint` file.
    #[serde(default)]
    pub slint_file: String,
    /// Name of the exported component to instantiate.  When omitted, the
    /// first exported component in the file is used.
    #[serde(default)]
    pub component: Option<String>,
    /// Key-value map of Slint properties to set on the component instance.
    /// Strings → `SharedString`, numbers → `f64`, booleans → `bool`.
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional list of property snapshots to cycle through over time.
    /// Each entry is a partial property map merged on top of `properties`.
    #[serde(default)]
    pub property_keyframes: Vec<HashMap<String, serde_json::Value>>,
    /// Number of frames between keyframe switches (default: 90 ≈ 3 s at 30 fps).
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval: u32,
    /// Total frames to generate.  0 = infinite (real-time pacing).
    #[serde(default = "default_frame_count")]
    pub frame_count: u32,
    /// When `true`, the rendered frame is cached and reused until properties
    /// change (via `UpdateParams` or keyframe cycling).  Suitable for overlays
    /// with no Slint-internal `Timer` or `animate` directives.  When `false`
    /// (the default), every frame is re-rendered so that Slint timers and
    /// animations advance correctly.
    #[serde(default = "default_static_ui")]
    pub static_ui: bool,
}

impl Default for SlintConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            slint_file: String::new(),
            component: None,
            properties: HashMap::new(),
            property_keyframes: Vec::new(),
            keyframe_interval: default_keyframe_interval(),
            frame_count: default_frame_count(),
            static_ui: default_static_ui(),
        }
    }
}

impl SlintConfig {
    /// Validate configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if dimensions are zero, fps is zero, or the
    /// slint file path is invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("width and height must be > 0".to_string());
        }
        if self.width > MAX_DIMENSION || self.height > MAX_DIMENSION {
            return Err(format!(
                "width and height must be <= {MAX_DIMENSION} (8K), got {}x{}",
                self.width, self.height
            ));
        }
        if self.fps == 0 {
            return Err("fps must be > 0".to_string());
        }
        validate_slint_asset_path(&self.slint_file)
    }

    /// Merge runtime property changes from an `UpdateParams` payload.
    ///
    /// Only `properties` values are merged (via `extend`) so that a partial
    /// JSON like `{"properties": {"home_score": 4}}` updates the named keys
    /// without dropping unmentioned ones.  Init-time fields (`slint_file`,
    /// `component`, `width`, `height`, `fps`, `frame_count`,
    /// `property_keyframes`, `keyframe_interval`) are left unchanged because
    /// serde defaults make it impossible to distinguish "user sent empty" from
    /// "field was absent in the JSON".
    pub fn merge_update(&mut self, update: &Self) {
        self.properties.extend(update.properties.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}

/// Validates that a Slint asset path is safe to read.
///
/// Allows any relative path but forbids directory traversal sequences.
///
/// # Errors
///
/// Returns an error string if the path is empty or contains traversal sequences.
fn validate_slint_asset_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("slint_file must not be empty".to_string());
    }
    if std::path::Path::new(path).is_absolute() {
        return Err(format!("Invalid slint_file: absolute paths are not allowed: {path}"));
    }
    if path.contains("..") {
        return Err(format!("Invalid slint_file: path must not contain '..': {path}"));
    }
    Ok(())
}
