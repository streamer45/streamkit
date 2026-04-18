// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration for the Servo web renderer plugin.

use serde::Deserialize;

// -- Defaults ----------------------------------------------------------------

/// Maximum allowed dimension (width or height) -- 8K.
/// Guards against config typos that would attempt multi-GB buffer allocations.
pub const MAX_DIMENSION: u32 = 7680;

const fn default_width() -> u32 {
    1280
}

const fn default_height() -> u32 {
    720
}

const fn default_fps() -> u32 {
    30
}

const fn default_frame_count() -> u32 {
    0
}

const fn default_load_timeout_secs() -> u32 {
    30
}

// -- Configuration -----------------------------------------------------------

/// Configuration for the Servo web renderer plugin.
///
/// Renders a web page at the given URL to RGBA8 video frames using the
/// embedded Servo browser engine.
///
/// ## Viewport vs Output resolution
///
/// `viewport_width`/`viewport_height` control how large the browser
/// viewport is (the CSS layout size).  `width`/`height` control the
/// output frame size.  When the viewport is larger than the output,
/// the rendered page is scaled down.  This lets web pages designed for
/// wider screens render fully without cropping.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServoConfig {
    /// URL to render (required).
    #[serde(default)]
    pub url: String,
    /// Output frame width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output frame height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Browser viewport width.  Defaults to `width` if unset or 0.
    /// Set larger than `width` to see more of the page, scaled down.
    #[serde(default)]
    pub viewport_width: u32,
    /// Browser viewport height.  Defaults to `height` if unset or 0.
    /// Set larger than `height` to see more of the page, scaled down.
    #[serde(default)]
    pub viewport_height: u32,
    /// Output frame rate.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Optional CSS to inject into the page after load.
    #[serde(default)]
    pub custom_css: Option<String>,
    /// Total frames to generate.  0 = infinite (real-time pacing).
    #[serde(default = "default_frame_count")]
    pub frame_count: u32,
    /// Maximum seconds to wait for the initial page load.
    #[serde(default = "default_load_timeout_secs")]
    pub load_timeout_secs: u32,
}

impl Default for ServoConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            width: default_width(),
            height: default_height(),
            viewport_width: 0,
            viewport_height: 0,
            fps: default_fps(),
            custom_css: None,
            frame_count: default_frame_count(),
            load_timeout_secs: default_load_timeout_secs(),
        }
    }
}

impl ServoConfig {
    /// Effective viewport width (falls back to output width).
    pub fn effective_viewport_width(&self) -> u32 {
        if self.viewport_width > 0 {
            self.viewport_width
        } else {
            self.width
        }
    }

    /// Effective viewport height (falls back to output height).
    pub fn effective_viewport_height(&self) -> u32 {
        if self.viewport_height > 0 {
            self.viewport_height
        } else {
            self.height
        }
    }

    /// Whether the viewport differs from the output and scaling is needed.
    pub fn needs_scaling(&self) -> bool {
        self.effective_viewport_width() != self.width
            || self.effective_viewport_height() != self.height
    }

    /// Validate configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if dimensions are zero, fps is zero, or the
    /// URL is empty / invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.url.is_empty() {
            return Err("url must not be empty".to_string());
        }
        if self.width == 0 || self.height == 0 {
            return Err("width and height must be > 0".to_string());
        }
        if self.width > MAX_DIMENSION || self.height > MAX_DIMENSION {
            return Err(format!(
                "width and height must be <= {MAX_DIMENSION} (8K), got {}x{}",
                self.width, self.height
            ));
        }
        let vw = self.effective_viewport_width();
        let vh = self.effective_viewport_height();
        if vw > MAX_DIMENSION || vh > MAX_DIMENSION {
            return Err(format!(
                "viewport dimensions must be <= {MAX_DIMENSION} (8K), got {vw}x{vh}",
            ));
        }
        if self.fps == 0 {
            return Err("fps must be > 0".to_string());
        }
        if self.load_timeout_secs == 0 {
            return Err("load_timeout_secs must be > 0".to_string());
        }
        // Validate that the URL is parseable.
        url::Url::parse(&self.url).map_err(|e| format!("invalid url '{}': {e}", self.url))?;
        Ok(())
    }

    /// Merge runtime parameter changes from an `UpdateParams` payload.
    ///
    /// Only the `url` and `custom_css` fields are merged; init-time fields
    /// (`width`, `height`, `fps`, `frame_count`, `viewport_*`) are left
    /// unchanged because they cannot be changed after the rendering context
    /// is created.
    pub fn merge_update(&mut self, update: &Self) {
        if !update.url.is_empty() {
            self.url.clone_from(&update.url);
        }
        if update.custom_css.is_some() {
            self.custom_css.clone_from(&update.custom_css);
        }
    }
}
