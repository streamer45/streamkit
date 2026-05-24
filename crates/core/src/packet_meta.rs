// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use ts_rs::TS;

use crate::types::PacketType;

/// If `wildcard_value` is present, a field equalling it is treated as "matches anything".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FieldRule {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wildcard_value: Option<serde_json::Value>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PacketTypeMeta {
    pub id: String,
    pub label: String,
    pub color: String,
    /// Placeholders are field names; "|*" means wildcard-display (client-side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_template: Option<String>,
    pub compatibility: Compatibility,
}

/// Lazily-initialized; shared to avoid allocations in hot paths.
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
                id: "RawVideo".into(),
                label: "Raw Video".into(),
                color: "#1abc9c".into(),
                display_template: Some("Raw Video ({width|*}x{height|*}, {pixel_format})".into()),
                compatibility: Compatibility::StructFieldWildcard {
                    fields: vec![
                        FieldRule {
                            name: "width".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                        FieldRule {
                            name: "height".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                        FieldRule { name: "pixel_format".into(), wildcard_value: None },
                    ],
                },
            },
            PacketTypeMeta {
                id: "EncodedAudio".into(),
                label: "Encoded Audio".into(),
                color: "#ff6b6b".into(),
                display_template: Some("Encoded Audio ({codec})".into()),
                compatibility: Compatibility::StructFieldWildcard {
                    fields: vec![
                        FieldRule { name: "codec".into(), wildcard_value: None },
                        FieldRule {
                            name: "codec_private".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                    ],
                },
            },
            PacketTypeMeta {
                id: "EncodedVideo".into(),
                label: "Encoded Video".into(),
                color: "#2980b9".into(),
                display_template: Some("Encoded Video ({codec})".into()),
                compatibility: Compatibility::StructFieldWildcard {
                    fields: vec![
                        FieldRule { name: "codec".into(), wildcard_value: None },
                        FieldRule {
                            name: "bitstream_format".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                        FieldRule {
                            name: "codec_private".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                        FieldRule {
                            name: "profile".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
                        FieldRule {
                            name: "level".into(),
                            wildcard_value: Some(serde_json::Value::Null),
                        },
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

fn find_meta<'a>(registry: &'a [PacketTypeMeta], id: &str) -> Option<&'a PacketTypeMeta> {
    registry.iter().find(|m| m.id == id)
}

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
                let wildcard = f.wildcard_value.as_ref();
                let av = match out_map.get(&f.name) {
                    Some(value) => value,
                    None => match wildcard {
                        Some(value) => value,
                        None => return false,
                    },
                };
                let bv = match in_map.get(&f.name) {
                    Some(value) => value,
                    None => match wildcard {
                        Some(value) => value,
                        None => return false,
                    },
                };

                if let Some(wild) = wildcard {
                    if av == wild || bv == wild {
                        return true;
                    }
                }

                av == bv
            })
        },
    }
}

pub fn can_connect_any(
    output: &PacketType,
    inputs: &[PacketType],
    registry: &[PacketTypeMeta],
) -> bool {
    inputs.iter().any(|inp| can_connect(output, inp, registry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioCodec, EncodedAudioFormat, PixelFormat, RawVideoFormat};

    #[test]
    fn raw_video_wildcard_dimensions() {
        let registry = packet_type_registry();
        let exact = RawVideoFormat {
            width: Some(1920),
            height: Some(1080),
            pixel_format: PixelFormat::I420,
        };
        let wildcard =
            RawVideoFormat { width: None, height: None, pixel_format: PixelFormat::I420 };
        let mismatched = RawVideoFormat {
            width: Some(1280),
            height: Some(720),
            pixel_format: PixelFormat::I420,
        };
        let different_format = RawVideoFormat {
            width: Some(1920),
            height: Some(1080),
            pixel_format: PixelFormat::Rgba8,
        };

        assert!(can_connect(
            &PacketType::RawVideo(exact.clone()),
            &PacketType::RawVideo(exact.clone()),
            registry
        ));
        assert!(can_connect(
            &PacketType::RawVideo(exact.clone()),
            &PacketType::RawVideo(wildcard.clone()),
            registry
        ));
        assert!(can_connect(
            &PacketType::RawVideo(wildcard),
            &PacketType::RawVideo(exact.clone()),
            registry
        ));
        assert!(!can_connect(
            &PacketType::RawVideo(exact.clone()),
            &PacketType::RawVideo(mismatched),
            registry
        ));
        assert!(!can_connect(
            &PacketType::RawVideo(exact),
            &PacketType::RawVideo(different_format),
            registry
        ));
    }

    #[test]
    fn encoded_audio_optional_fields() {
        let registry = packet_type_registry();
        let format = EncodedAudioFormat { codec: AudioCodec::Opus, codec_private: None };

        assert!(can_connect(
            &PacketType::EncodedAudio(format.clone()),
            &PacketType::EncodedAudio(format),
            registry
        ));
    }
}
