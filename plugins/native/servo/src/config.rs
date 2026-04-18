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
    /// Viewport resolution preset (e.g. `"1280x720"`).  When set via
    /// a runtime update, overrides `viewport_width` and `viewport_height`.
    /// This is the preferred way to change the viewport at runtime via
    /// the Stream View controls (select dropdown).
    #[serde(default)]
    pub viewport_resolution: Option<String>,
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
            viewport_resolution: None,
            fps: default_fps(),
            custom_css: None,
            frame_count: default_frame_count(),
            load_timeout_secs: default_load_timeout_secs(),
        }
    }
}

impl ServoConfig {
    /// Parse a `"WxH"` resolution string (e.g. `"1920x1080"`) into `(width, height)`.
    fn parse_resolution(s: &str) -> Option<(u32, u32)> {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() == 2 {
            if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
        }
        None
    }

    /// Apply `viewport_resolution` string (if set) to `viewport_width`/`viewport_height`.
    /// Called at init time so the rendering context is created at the correct size.
    pub fn apply_resolution_preset(&mut self) {
        if let Some(ref res) = self.viewport_resolution {
            if let Some((w, h)) = Self::parse_resolution(res) {
                self.viewport_width = w.min(MAX_DIMENSION);
                self.viewport_height = h.min(MAX_DIMENSION);
            }
        }
    }

    /// Effective viewport width (falls back to output width).
    pub const fn effective_viewport_width(&self) -> u32 {
        if self.viewport_width > 0 {
            self.viewport_width
        } else {
            self.width
        }
    }

    /// Effective viewport height (falls back to output height).
    pub const fn effective_viewport_height(&self) -> u32 {
        if self.viewport_height > 0 {
            self.viewport_height
        } else {
            self.height
        }
    }

    /// Whether the viewport differs from the output and scaling is needed.
    pub const fn needs_scaling(&self) -> bool {
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
    /// Tunable fields: `url`, `custom_css`, `viewport_resolution`.
    /// Init-time fields (`width`, `height`, `fps`, `frame_count`) are left
    /// unchanged.  `viewport_width`/`viewport_height` are updated only
    /// when `viewport_resolution` is provided (parsed from a `"WxH"` string).
    ///
    /// Returns `true` if the viewport dimensions changed (requiring a
    /// rendering context resize).
    pub fn merge_update(&mut self, update: &Self) -> bool {
        if !update.url.is_empty() {
            self.url.clone_from(&update.url);
        }
        if update.custom_css.is_some() {
            self.custom_css.clone_from(&update.custom_css);
        }

        // Handle viewport_resolution preset.
        let mut viewport_changed = false;
        if let Some(ref res) = update.viewport_resolution {
            if let Some((w, h)) = Self::parse_resolution(res) {
                let w = w.min(MAX_DIMENSION);
                let h = h.min(MAX_DIMENSION);
                if w != self.viewport_width || h != self.viewport_height {
                    self.viewport_width = w;
                    self.viewport_height = h;
                    self.viewport_resolution = Some(res.clone());
                    viewport_changed = true;
                }
            }
        }
        viewport_changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> ServoConfig {
        ServoConfig { url: "https://example.com".to_string(), ..ServoConfig::default() }
    }

    // -- validate ---------------------------------------------------------

    #[test]
    fn validate_valid_config() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn validate_empty_url() {
        let cfg = ServoConfig::default();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("url must not be empty"));
    }

    #[test]
    fn validate_invalid_url() {
        let cfg = ServoConfig { url: "not a url".to_string(), ..ServoConfig::default() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("invalid url"));
    }

    #[test]
    fn validate_zero_width() {
        let cfg = ServoConfig { width: 0, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("width and height must be > 0"));
    }

    #[test]
    fn validate_zero_height() {
        let cfg = ServoConfig { height: 0, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("width and height must be > 0"));
    }

    #[test]
    fn validate_exceeds_max_dimension() {
        let cfg = ServoConfig { width: MAX_DIMENSION + 1, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("must be <="));
    }

    #[test]
    fn validate_viewport_exceeds_max_dimension() {
        let cfg = ServoConfig { viewport_width: MAX_DIMENSION + 1, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("viewport dimensions must be <="));
    }

    #[test]
    fn validate_zero_fps() {
        let cfg = ServoConfig { fps: 0, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("fps must be > 0"));
    }

    #[test]
    fn validate_zero_load_timeout() {
        let cfg = ServoConfig { load_timeout_secs: 0, ..valid_config() };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("load_timeout_secs must be > 0"));
    }

    #[test]
    fn validate_at_max_dimension() {
        let cfg = ServoConfig { width: MAX_DIMENSION, height: MAX_DIMENSION, ..valid_config() };
        assert!(cfg.validate().is_ok());
    }

    // -- parse_resolution -------------------------------------------------

    #[test]
    fn parse_resolution_valid() {
        assert_eq!(ServoConfig::parse_resolution("1920x1080"), Some((1920, 1080)));
    }

    #[test]
    fn parse_resolution_small() {
        assert_eq!(ServoConfig::parse_resolution("1x1"), Some((1, 1)));
    }

    #[test]
    fn parse_resolution_missing_separator() {
        assert_eq!(ServoConfig::parse_resolution("1920-1080"), None);
    }

    #[test]
    fn parse_resolution_zero_width() {
        assert_eq!(ServoConfig::parse_resolution("0x1080"), None);
    }

    #[test]
    fn parse_resolution_zero_height() {
        assert_eq!(ServoConfig::parse_resolution("1920x0"), None);
    }

    #[test]
    fn parse_resolution_non_numeric() {
        assert_eq!(ServoConfig::parse_resolution("abcxdef"), None);
    }

    #[test]
    fn parse_resolution_empty() {
        assert_eq!(ServoConfig::parse_resolution(""), None);
    }

    #[test]
    fn parse_resolution_extra_parts() {
        assert_eq!(ServoConfig::parse_resolution("1920x1080x60"), None);
    }

    // -- apply_resolution_preset ------------------------------------------

    #[test]
    fn apply_resolution_preset_sets_viewport() {
        let mut cfg =
            ServoConfig { viewport_resolution: Some("1920x1080".to_string()), ..valid_config() };
        cfg.apply_resolution_preset();
        assert_eq!(cfg.viewport_width, 1920);
        assert_eq!(cfg.viewport_height, 1080);
    }

    #[test]
    fn apply_resolution_preset_clamps_to_max() {
        let mut cfg =
            ServoConfig { viewport_resolution: Some("99999x99999".to_string()), ..valid_config() };
        cfg.apply_resolution_preset();
        assert_eq!(cfg.viewport_width, MAX_DIMENSION);
        assert_eq!(cfg.viewport_height, MAX_DIMENSION);
    }

    #[test]
    fn apply_resolution_preset_no_op_when_none() {
        let mut cfg = valid_config();
        cfg.apply_resolution_preset();
        assert_eq!(cfg.viewport_width, 0);
        assert_eq!(cfg.viewport_height, 0);
    }

    #[test]
    fn apply_resolution_preset_invalid_string_ignored() {
        let mut cfg =
            ServoConfig { viewport_resolution: Some("invalid".to_string()), ..valid_config() };
        cfg.apply_resolution_preset();
        assert_eq!(cfg.viewport_width, 0);
        assert_eq!(cfg.viewport_height, 0);
    }

    // -- effective_viewport_width / effective_viewport_height --------------

    #[test]
    fn effective_viewport_falls_back_to_output() {
        let cfg = valid_config();
        assert_eq!(cfg.effective_viewport_width(), cfg.width);
        assert_eq!(cfg.effective_viewport_height(), cfg.height);
    }

    #[test]
    fn effective_viewport_uses_explicit_value() {
        let cfg = ServoConfig { viewport_width: 1920, viewport_height: 1080, ..valid_config() };
        assert_eq!(cfg.effective_viewport_width(), 1920);
        assert_eq!(cfg.effective_viewport_height(), 1080);
    }

    // -- needs_scaling ----------------------------------------------------

    #[test]
    fn needs_scaling_false_when_same() {
        let cfg = valid_config();
        assert!(!cfg.needs_scaling());
    }

    #[test]
    fn needs_scaling_true_when_viewport_differs() {
        let cfg = ServoConfig { viewport_width: 1920, ..valid_config() };
        assert!(cfg.needs_scaling());
    }

    // -- merge_update -----------------------------------------------------

    #[test]
    fn merge_update_changes_url() {
        let mut cfg = valid_config();
        let update =
            ServoConfig { url: "https://new.example.com".to_string(), ..ServoConfig::default() };
        cfg.merge_update(&update);
        assert_eq!(cfg.url, "https://new.example.com");
    }

    #[test]
    fn merge_update_empty_url_keeps_old() {
        let mut cfg = valid_config();
        let update = ServoConfig::default();
        cfg.merge_update(&update);
        assert_eq!(cfg.url, "https://example.com");
    }

    #[test]
    fn merge_update_changes_custom_css() {
        let mut cfg = valid_config();
        let update = ServoConfig {
            custom_css: Some("body { color: red; }".to_string()),
            ..ServoConfig::default()
        };
        cfg.merge_update(&update);
        assert_eq!(cfg.custom_css.as_deref(), Some("body { color: red; }"));
    }

    #[test]
    fn merge_update_viewport_resolution_returns_true() {
        let mut cfg = ServoConfig { viewport_width: 1280, viewport_height: 720, ..valid_config() };
        let update = ServoConfig {
            viewport_resolution: Some("1920x1080".to_string()),
            ..ServoConfig::default()
        };
        let changed = cfg.merge_update(&update);
        assert!(changed);
        assert_eq!(cfg.viewport_width, 1920);
        assert_eq!(cfg.viewport_height, 1080);
    }

    #[test]
    fn merge_update_same_viewport_returns_false() {
        let mut cfg = ServoConfig { viewport_width: 1920, viewport_height: 1080, ..valid_config() };
        let update = ServoConfig {
            viewport_resolution: Some("1920x1080".to_string()),
            ..ServoConfig::default()
        };
        let changed = cfg.merge_update(&update);
        assert!(!changed);
    }

    #[test]
    fn merge_update_viewport_clamps_to_max() {
        let mut cfg = valid_config();
        let update = ServoConfig {
            viewport_resolution: Some("99999x99999".to_string()),
            ..ServoConfig::default()
        };
        let changed = cfg.merge_update(&update);
        assert!(changed);
        assert_eq!(cfg.viewport_width, MAX_DIMENSION);
        assert_eq!(cfg.viewport_height, MAX_DIMENSION);
    }

    #[test]
    fn merge_update_invalid_viewport_resolution_ignored() {
        let mut cfg = ServoConfig { viewport_width: 1280, viewport_height: 720, ..valid_config() };
        let update = ServoConfig {
            viewport_resolution: Some("invalid".to_string()),
            ..ServoConfig::default()
        };
        let changed = cfg.merge_update(&update);
        assert!(!changed);
        assert_eq!(cfg.viewport_width, 1280);
        assert_eq!(cfg.viewport_height, 720);
    }

    #[test]
    fn merge_update_preserves_init_time_fields() {
        let mut cfg =
            ServoConfig { fps: 60, frame_count: 100, width: 640, height: 480, ..valid_config() };
        let update = ServoConfig {
            fps: 30,
            frame_count: 0,
            width: 1920,
            height: 1080,
            url: "https://new.example.com".to_string(),
            ..ServoConfig::default()
        };
        cfg.merge_update(&update);
        assert_eq!(cfg.fps, 60);
        assert_eq!(cfg.frame_count, 100);
        assert_eq!(cfg.width, 640);
        assert_eq!(cfg.height, 480);
        assert_eq!(cfg.url, "https://new.example.com");
    }
}
