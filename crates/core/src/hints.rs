// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Advisory hints sent from downstream consumers back to upstream sources.

/// Advisory hints sent from a downstream consumer back to an upstream
/// source, allowing the source to adapt its output characteristics.
///
/// Hints are non-binding — sources may ignore any or all of them.
/// The channel uses `try_send` so stale hints are dropped if the
/// source is slow to drain.
///
/// Marked `#[non_exhaustive]` so future variants (e.g. `PreferredFps`,
/// `PreferredPixelFormat`) can be added without breaking downstream
/// plugin `match` arms.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UpstreamHint {
    /// The downstream consumer would prefer frames at this resolution.
    /// Sources that support resolution-independent rendering (e.g. Slint)
    /// can re-rasterize at the requested size to avoid scaling artifacts.
    PreferredSize { width: u32, height: u32 },
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn preferred_size_round_trips_json() {
        let hint = UpstreamHint::PreferredSize { width: 1920, height: 1080 };
        let json = serde_json::to_string(&hint).expect("serialize");
        let parsed: UpstreamHint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, UpstreamHint::PreferredSize { width: 1920, height: 1080 });
    }

    #[test]
    fn preferred_size_tagged_json_format() {
        let hint = UpstreamHint::PreferredSize { width: 640, height: 480 };
        let json = serde_json::to_string(&hint).expect("serialize");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["type"], "preferred_size");
        assert_eq!(v["width"], 640);
        assert_eq!(v["height"], 480);
    }

    #[test]
    fn deserialize_from_known_json() {
        let json = r#"{"type":"preferred_size","width":3840,"height":2160}"#;
        let hint: UpstreamHint = serde_json::from_str(json).expect("deserialize");
        assert_eq!(hint, UpstreamHint::PreferredSize { width: 3840, height: 2160 });
    }
}
