// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Resolve bounded metric labels from trusted request headers.
//!
//! The values are constrained to operator-configured allowlists so
//! client-supplied headers can never inflate metric cardinality.

use axum::http::HeaderMap;
use opentelemetry::KeyValue;

use crate::config::RequestLabelConfig;

/// Label keys emitted by the built-in request instruments (HTTP middleware and
/// oneshot histogram). Configured request labels must not collide with these,
/// even after Prometheus sanitizes the key.
pub const STATUS_KEY: &str = "status";
pub const HTTP_METHOD_KEY: &str = "http.method";
pub const HTTP_ROUTE_KEY: &str = "http.route";
pub const HTTP_STATUS_CODE_KEY: &str = "http.status_code";

/// Single source of truth for the built-in keys, referenced both at the emit
/// sites and by config validation so the reserved set cannot drift.
pub const RESERVED_LABEL_KEYS: [&str; 4] =
    [STATUS_KEY, HTTP_METHOD_KEY, HTTP_ROUTE_KEY, HTTP_STATUS_CODE_KEY];

/// Bounded request labels resolved once per request and stashed in request
/// extensions so downstream handlers can reuse them without re-parsing headers.
#[derive(Clone)]
pub struct ResolvedRequestLabels(pub Vec<KeyValue>);

/// Canonical normalization for label values and allowlist entries: trim and
/// ASCII-lowercase. Shared so the per-request path and config load stay in step.
pub fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Constrain a header value to an allowlist, falling back when it is absent or
/// unrecognized. The incoming value is normalized (trim + lowercase); `allowed`
/// entries are expected to be pre-normalized at config load. An empty value
/// never matches, so a stray empty allowlist entry can't emit an empty label.
fn classify(value: Option<&str>, allowed: &[String], fallback: &str) -> String {
    match value.map(normalize) {
        Some(v) if !v.is_empty() && allowed.contains(&v) => v,
        _ => fallback.to_string(),
    }
}

/// Resolve configured request labels into bounded metric key-values.
pub fn resolve_request_labels(labels: &[RequestLabelConfig], headers: &HeaderMap) -> Vec<KeyValue> {
    labels
        .iter()
        .map(|label| {
            let value = headers.get(label.header.as_str()).and_then(|v| v.to_str().ok());
            KeyValue::new(label.name.clone(), classify(value, &label.allowed, &label.fallback))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn label(name: &str, header: &str, allowed: &[&str]) -> RequestLabelConfig {
        RequestLabelConfig {
            name: name.to_string(),
            header: header.to_string(),
            allowed: allowed.iter().map(|s| (*s).to_string()).collect(),
            fallback: "other".to_string(),
        }
    }

    #[test]
    fn classify_allows_listed_values() {
        let allowed = vec!["tts".to_string(), "stt".to_string()];
        assert_eq!(classify(Some("tts"), &allowed, "other"), "tts");
        assert_eq!(classify(Some("stt"), &allowed, "other"), "stt");
    }

    #[test]
    fn classify_normalizes_case_and_whitespace() {
        let allowed = vec!["tts".to_string()];
        assert_eq!(classify(Some("  TTS  "), &allowed, "other"), "tts");
    }

    #[test]
    fn classify_unknown_empty_and_absent_fall_back() {
        let allowed = vec!["tts".to_string()];
        assert_eq!(classify(Some("kokoro"), &allowed, "other"), "other");
        assert_eq!(classify(Some(""), &allowed, "other"), "other");
        assert_eq!(classify(None, &allowed, "other"), "other");
    }

    #[test]
    fn classify_empty_allowlist_always_falls_back() {
        assert_eq!(classify(Some("tts"), &[], "other"), "other");
    }

    #[test]
    fn classify_empty_allowlist_entry_never_emits_empty_value() {
        let allowed = vec![String::new()];
        assert_eq!(classify(Some(""), &allowed, "other"), "other");
        assert_eq!(classify(Some("   "), &allowed, "other"), "other");
    }

    #[test]
    fn resolve_emits_one_keyvalue_per_label() {
        let labels = vec![label("service", "X-StreamKit-Service", &["tts", "stt"])];
        let mut headers = HeaderMap::new();
        headers.insert("X-StreamKit-Service", HeaderValue::from_static("STT"));

        let resolved = resolve_request_labels(&labels, &headers);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].key.as_str(), "service");
        assert_eq!(resolved[0].value.as_str(), "stt");
    }

    #[test]
    fn resolve_falls_back_when_header_missing() {
        let labels = vec![label("service", "X-StreamKit-Service", &["tts", "stt"])];
        let resolved = resolve_request_labels(&labels, &HeaderMap::new());
        assert_eq!(resolved[0].value.as_str(), "other");
    }
}
