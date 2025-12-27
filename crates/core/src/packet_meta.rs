// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use ts_rs::TS;

use crate::types::PacketType;

/// Declarative field rule used by compatibility checks.
/// If `wildcard_value` is present, a field that equals it is treated as "matches anything".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FieldRule {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wildcard_value: Option<serde_json::Value>,
}

/// Simple compatibility strategies supported in v1.
/// This enum is intentionally extensible for future variants (e.g., expressions, ranges).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Compatibility {
    /// Matches anything.
    Any,
    /// Kinds must be identical. Unit variants always match when kinds match.
    Exact,
    /// Kinds must match. Each field must be equal unless either side equals the wildcard_value.
    StructFieldWildcard { fields: Vec<FieldRule> },
}

/// Server-driven metadata for packet types.
/// Lives next to the PacketType definition (in core) and can be exposed to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PacketTypeMeta {
    /// Variant identifier (e.g., "RawAudio", "OpusAudio", "Binary", "Any").
    pub id: String,
    /// Human-friendly default label.
    pub label: String,
    /// Hex color to use in UIs.
    pub color: String,
    /// Optional display template for struct payloads. Placeholders are field names,
    /// optionally with "|*" to indicate wildcard-display (handled on the client).
    /// Example: "Raw Audio ({sample_rate|*}Hz, {channels|*}ch, {sample_format})"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_template: Option<String>,
    /// Compatibility strategy for this type.
    pub compatibility: Compatibility,
}

/// Returns the built-in registry of packet type metadata.
/// This returns a shared, lazily-initialized slice to avoid allocations in hot paths
/// (e.g., runtime validation in the dynamic engine).
pub fn packet_type_registry() -> &'static [PacketTypeMeta] {
    static REGISTRY: OnceLock<Vec<PacketTypeMeta>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            PacketTypeMeta {
                id: "Any".into(),
                label: "Any".into(),
                color: "#96ceb4".into(),
                display_template: None,
                compatibility: Compatibility::Any,
            },
            PacketTypeMeta {
                id: "Binary".into(),
                label: "Binary".into(),
                color: "#45b7d1".into(),
                display_template: None,
                compatibility: Compatibility::Exact,
            },
            PacketTypeMeta {
                id: "Text".into(),
                label: "Text".into(),
                color: "#4ecdc4".into(),
                display_template: None,
                compatibility: Compatibility::Exact,
            },
            PacketTypeMeta {
                id: "OpusAudio".into(),
                label: "Opus Audio".into(),
                color: "#ff6b6b".into(),
                display_template: None,
                compatibility: Compatibility::Exact,
            },
            PacketTypeMeta {
                id: "RawAudio".into(),
                label: "Raw Audio".into(),
                color: "#f39c12".into(),
                display_template: Some(
                    "Raw Audio ({sample_rate|*}Hz, {channels|*}ch, {sample_format})".into(),
                ),
                compatibility: Compatibility::StructFieldWildcard {
                    fields: vec![
                        FieldRule {
                            name: "sample_rate".into(),
                            wildcard_value: Some(serde_json::json!(0)),
                        },
                        FieldRule {
                            name: "channels".into(),
                            wildcard_value: Some(serde_json::json!(0)),
                        },
                        FieldRule { name: "sample_format".into(), wildcard_value: None },
                    ],
                },
            },
            PacketTypeMeta {
                id: "Transcription".into(),
                label: "Transcription".into(),
                color: "#9b59b6".into(),
                display_template: None,
                compatibility: Compatibility::Exact,
            },
            PacketTypeMeta {
                id: "Custom".into(),
                label: "Custom".into(),
                color: "#e67e22".into(),
                display_template: Some("Custom ({type_id})".into()),
                compatibility: Compatibility::StructFieldWildcard {
                    fields: vec![FieldRule { name: "type_id".into(), wildcard_value: None }],
                },
            },
        ]
    })
}

/// Extracts the PacketType variant id and an optional JSON payload for struct variants.
fn to_variant_and_payload(packet_type: &PacketType) -> (String, Option<serde_json::Value>) {
    let json = serde_json::to_value(packet_type).unwrap_or(serde_json::Value::Null);
    match json {
        serde_json::Value::String(unit) => (unit, None),
        serde_json::Value::Object(map) => {
            if map.len() == 1 {
                // SAFETY: We just checked that map has exactly 1 element
                if let Some((k, v)) = map.into_iter().next() {
                    (k, Some(v))
                } else {
                    ("Unknown".to_string(), None)
                }
            } else {
                ("Unknown".to_string(), None)
            }
        },
        _ => ("Unknown".to_string(), None),
    }
}

/// Finds metadata by id.
fn find_meta<'a>(registry: &'a [PacketTypeMeta], id: &str) -> Option<&'a PacketTypeMeta> {
    registry.iter().find(|m| m.id == id)
}

/// Generic, server-side compatibility check used by both stateless and dynamic engines.
/// v1 rules:
/// - Any matches anything
/// - Kinds must match (unless Any)
/// - Exact: always true when kinds match
/// - StructFieldWildcard: all fields must be equal unless either side equals wildcard_value
pub fn can_connect(output: &PacketType, input: &PacketType, registry: &[PacketTypeMeta]) -> bool {
    let (out_id, out_payload) = to_variant_and_payload(output);
    let (in_id, in_payload) = to_variant_and_payload(input);

    if out_id == "Any" || in_id == "Any" {
        return true;
    }
    if out_id != in_id {
        return false;
    }

    let Some(meta) = find_meta(registry, &out_id) else {
        // If we lack metadata, be conservative.
        return false;
    };

    match &meta.compatibility {
        Compatibility::Any | Compatibility::Exact => true,
        Compatibility::StructFieldWildcard { fields } => {
            let (Some(out_obj), Some(in_obj)) = (out_payload.as_ref(), in_payload.as_ref()) else {
                return false;
            };
            let Some(out_map) = out_obj.as_object() else {
                return false;
            };
            let Some(in_map) = in_obj.as_object() else {
                return false;
            };

            fields.iter().all(|f| {
                let Some(av) = out_map.get(&f.name) else {
                    return false;
                };
                let Some(bv) = in_map.get(&f.name) else {
                    return false;
                };

                // If either equals the wildcard, it matches
                if let Some(wild) = &f.wildcard_value {
                    if av == wild || bv == wild {
                        return true;
                    }
                }

                // Otherwise, values must be equal
                av == bv
            })
        },
    }
}

/// Convenience helper to test an output type against multiple input types.
pub fn can_connect_any(
    output: &PacketType,
    inputs: &[PacketType],
    registry: &[PacketTypeMeta],
) -> bool {
    inputs.iter().any(|inp| can_connect(output, inp, registry))
}
