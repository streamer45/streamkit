// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Resolve bounded metric attributes from a pipeline's declared `attributes`.
//!
//! The pipeline declares key/value attributes; the operator config governs
//! which keys are allowed and how each is bounded, so a user-submitted pipeline
//! can never inflate metric cardinality.

use std::collections::BTreeMap;

use opentelemetry::KeyValue;
use streamkit_engine::ResolvedAttributes;

use crate::config::MetricsAttributePolicy;

/// Label keys emitted by the engine's node/pipeline instruments.
///
/// Attributes are merged onto exactly these metrics' label sets, so a configured
/// attribute key must not collide with one of these (even after Prometheus
/// sanitizes it).
pub const STATUS_KEY: &str = "status";
pub const NODE_ID_KEY: &str = "node_id";
pub const NODE_KIND_KEY: &str = "node_kind";
pub const STATE_KEY: &str = "state";
pub const PIN_NAME_KEY: &str = "pin_name";

/// Label keys emitted by the HTTP request-metrics middleware. Attributes are
/// never merged onto `http.server.*`, so these are not reserved against
/// attribute keys.
pub const HTTP_METHOD_KEY: &str = "http.method";
pub const HTTP_ROUTE_KEY: &str = "http.route";
pub const HTTP_STATUS_CODE_KEY: &str = "http.status_code";

/// Single source of truth for the built-in node/pipeline metric keys,
/// referenced both at the emit sites and by config validation so the reserved
/// set cannot drift.
pub const RESERVED_LABEL_KEYS: [&str; 5] =
    [STATUS_KEY, NODE_ID_KEY, NODE_KIND_KEY, STATE_KEY, PIN_NAME_KEY];

/// Canonical normalization for attribute values and policy entries: trim and
/// ASCII-lowercase. Shared so resolution and config load stay in step.
pub fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Constrain a declared value against its policy. An empty normalized value
/// (or one outside an `allowed` allowlist) collapses to `fallback`, so a label
/// can never be emitted empty. `allowed` entries are pre-normalized at config
/// load; passthrough policies (`allowed` empty) accept any non-empty value.
fn classify(value: &str, policy: &MetricsAttributePolicy) -> String {
    let v = normalize(value);
    if v.is_empty() {
        return policy.fallback.clone();
    }
    if policy.is_passthrough() || policy.allowed.contains(&v) {
        v
    } else {
        policy.fallback.clone()
    }
}

/// Resolve a pipeline's declared attributes against the operator policy.
///
/// Produces bounded metric key-values. Keys absent from the policy are dropped.
/// The declared key is normalized (trim + lowercase) before lookup so it matches
/// the same way declared values do; policy keys are validated lowercase at load.
/// Declared keys that collide after normalization (e.g. `Service` and `service`)
/// are collapsed to one entry so a measurement never carries duplicate keys.
pub fn resolve_attributes(
    declared: Option<&BTreeMap<String, String>>,
    policy: &BTreeMap<String, MetricsAttributePolicy>,
) -> ResolvedAttributes {
    let resolved: BTreeMap<String, String> = declared
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            let key = normalize(key);
            policy.get(&key).map(|p| (key, classify(value, p)))
        })
        .collect();
    let pipeline = resolved.into_iter().map(|(k, v)| KeyValue::new(k, v)).collect();
    ResolvedAttributes { pipeline, per_node: std::collections::HashMap::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allowed: &[&str]) -> MetricsAttributePolicy {
        MetricsAttributePolicy {
            allowed: allowed.iter().map(|s| (*s).to_string()).collect(),
            fallback: "other".to_string(),
        }
    }

    fn declared(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    fn service_policy(allowed: &[&str]) -> BTreeMap<String, MetricsAttributePolicy> {
        let mut map = BTreeMap::new();
        map.insert("service".to_string(), policy(allowed));
        map
    }

    #[test]
    fn classify_allows_listed_values() {
        let p = policy(&["tts", "stt"]);
        assert_eq!(classify("tts", &p), "tts");
        assert_eq!(classify("stt", &p), "stt");
    }

    #[test]
    fn classify_normalizes_case_and_whitespace() {
        let p = policy(&["tts"]);
        assert_eq!(classify("  TTS  ", &p), "tts");
    }

    #[test]
    fn classify_unknown_and_empty_fall_back() {
        let p = policy(&["tts"]);
        assert_eq!(classify("kokoro", &p), "other");
        assert_eq!(classify("", &p), "other");
        assert_eq!(classify("   ", &p), "other");
    }

    #[test]
    fn classify_passthrough_accepts_any_nonempty() {
        let p = policy(&[]);
        assert_eq!(classify("tenant-42", &p), "tenant-42");
        assert_eq!(classify("", &p), "other");
    }

    #[test]
    fn resolve_emits_one_keyvalue_per_allowed_declared_attribute() {
        let resolved = resolve_attributes(
            Some(&declared(&[("service", "STT")])),
            &service_policy(&["tts", "stt"]),
        );
        assert_eq!(resolved.pipeline.len(), 1);
        assert_eq!(resolved.pipeline[0].key.as_str(), "service");
        assert_eq!(resolved.pipeline[0].value.as_str(), "stt");
    }

    #[test]
    fn resolve_clamps_value_outside_allowlist_to_fallback() {
        let resolved = resolve_attributes(
            Some(&declared(&[("service", "kokoro")])),
            &service_policy(&["tts", "stt"]),
        );
        assert_eq!(resolved.pipeline[0].value.as_str(), "other");
    }

    #[test]
    fn resolve_drops_keys_absent_from_policy() {
        let resolved = resolve_attributes(
            Some(&declared(&[("service", "tts"), ("tenant", "acme")])),
            &service_policy(&["tts"]),
        );
        assert_eq!(resolved.pipeline.len(), 1);
        assert_eq!(resolved.pipeline[0].key.as_str(), "service");
    }

    #[test]
    fn resolve_without_attributes_is_empty() {
        let resolved = resolve_attributes(None, &service_policy(&["tts"]));
        assert!(resolved.pipeline.is_empty());
    }

    #[test]
    fn resolve_with_empty_policy_emits_nothing() {
        let resolved = resolve_attributes(Some(&declared(&[("service", "tts")])), &BTreeMap::new());
        assert!(resolved.pipeline.is_empty());
    }

    #[test]
    fn resolve_matches_declared_key_case_insensitively() {
        let resolved = resolve_attributes(
            Some(&declared(&[("Service", "TTS")])),
            &service_policy(&["tts", "stt"]),
        );
        assert_eq!(resolved.pipeline.len(), 1);
        assert_eq!(resolved.pipeline[0].key.as_str(), "service");
        assert_eq!(resolved.pipeline[0].value.as_str(), "tts");
    }

    #[test]
    fn resolve_collapses_keys_colliding_after_normalization() {
        let resolved = resolve_attributes(
            Some(&declared(&[("Service", "tts"), ("service", "stt")])),
            &service_policy(&["tts", "stt"]),
        );
        assert_eq!(resolved.pipeline.len(), 1, "duplicate keys must collapse to one entry");
        assert_eq!(resolved.pipeline[0].key.as_str(), "service");
    }
}
