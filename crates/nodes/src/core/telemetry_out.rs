// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use streamkit_core::telemetry::{TelemetryEmitter, TELEMETRY_TYPE_ID};
use streamkit_core::types::{CustomPacketData, Packet, PacketType, TranscriptionData};
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

const VAD_EVENT_TYPE_ID: &str = "plugin::native::vad/vad-event@1";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelemetryOutConfig {
    /// Which packet types to convert to telemetry.
    /// Default: `["Transcription", "Custom"]`
    #[serde(default = "default_packet_types")]
    pub packet_types: Vec<String>,

    /// Filter event types (glob-style prefix patterns like `vad.*`).
    /// Empty list means all events are included.
    #[serde(default)]
    pub event_type_filter: Vec<String>,

    /// Maximum events per second per event type.
    #[serde(default = "default_max_events_per_sec")]
    pub max_events_per_sec: u32,
}

fn default_packet_types() -> Vec<String> {
    vec!["Transcription".to_string(), "Custom".to_string()]
}

const fn default_max_events_per_sec() -> u32 {
    100
}

impl Default for TelemetryOutConfig {
    fn default() -> Self {
        Self {
            packet_types: default_packet_types(),
            event_type_filter: Vec::new(),
            max_events_per_sec: default_max_events_per_sec(),
        }
    }
}

#[derive(Default)]
pub struct TelemetryOutNode {
    config: TelemetryOutConfig,
}

impl TelemetryOutNode {
    /// Create a `TelemetryOutNode` from configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if `params` is present but cannot be deserialized into `TelemetryOutConfig`.
    pub fn new(params: Option<serde_json::Value>) -> Result<Self, StreamKitError> {
        let config: TelemetryOutConfig = if let Some(params) = params {
            serde_json::from_value(params)
                .map_err(|e| StreamKitError::Configuration(format!("Invalid config: {e}")))?
        } else {
            TelemetryOutConfig::default()
        };

        Ok(Self { config })
    }

    fn should_tap_packet_type(&self, packet: &Packet) -> bool {
        let type_name = match packet {
            Packet::Audio(_) => "Audio",
            Packet::Video(_) => "Video",
            Packet::Transcription(_) => "Transcription",
            Packet::Custom(_) => "Custom",
            Packet::Binary { .. } => "Binary",
            Packet::Text(_) => "Text",
        };

        self.config.packet_types.iter().any(|t| t.eq_ignore_ascii_case(type_name))
    }

    fn matches_event_type_filter(&self, event_type: &str) -> bool {
        super::glob_filter::matches_glob_filter(&self.config.event_type_filter, event_type)
    }

    fn truncate_preview(text: &str, max_chars: usize) -> String {
        if max_chars == 0 {
            return String::new();
        }

        let mut chars = text.chars();
        let prefix: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            format!("{prefix}...")
        } else {
            prefix
        }
    }

    fn transcription_to_telemetry(transcription: &TranscriptionData) -> JsonValue {
        serde_json::json!({
            "text_preview": Self::truncate_preview(&transcription.text, 100),
            "text_length": transcription.text.len(),
            "segment_count": transcription.segments.len(),
            "language": transcription.language,
        })
    }

    fn custom_to_event_type(custom: &CustomPacketData) -> String {
        let event_type =
            custom.data.get("event_type").and_then(|v| v.as_str()).unwrap_or("custom.unknown");

        if custom.type_id == TELEMETRY_TYPE_ID {
            return event_type.to_string();
        }

        if custom.type_id == VAD_EVENT_TYPE_ID && !event_type.starts_with("vad.") {
            return format!("vad.{event_type}");
        }

        event_type.to_string()
    }
}

#[async_trait]
impl ProcessorNode for TelemetryOutNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let mut telemetry = TelemetryEmitter::new(
            node_name.clone(),
            context.session_id.clone(),
            context.telemetry_tx.clone(),
        );
        telemetry.set_rate_limit(self.config.max_events_per_sec);

        let mut input_rx = context.take_input("in")?;
        state_helpers::emit_running(&context.state_tx, &node_name);

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            if !self.should_tap_packet_type(&packet) {
                continue;
            }

            match &packet {
                Packet::Transcription(t) => {
                    telemetry.emit("stt.result", Self::transcription_to_telemetry(t));
                },
                Packet::Custom(custom) => {
                    let telemetry_event_type = Self::custom_to_event_type(custom);
                    if !self.matches_event_type_filter(&telemetry_event_type) {
                        continue;
                    }

                    let mut data = custom.data.clone();
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(
                            "source_type_id".to_string(),
                            JsonValue::String(custom.type_id.clone()),
                        );
                    }

                    telemetry.emit(&telemetry_event_type, data);
                },
                Packet::Text(text) => {
                    let preview = Self::truncate_preview(text, 100);
                    telemetry.emit(
                        "text.received",
                        serde_json::json!({ "text_preview": preview, "length": text.len() }),
                    );
                },
                Packet::Binary { data, metadata, .. } => {
                    telemetry.emit(
                        "binary.received",
                        serde_json::json!({ "size_bytes": data.len(), "has_metadata": metadata.is_some() }),
                    );
                },
                Packet::Video(_) | Packet::Audio(_) => {
                    // Intentionally no audio-level telemetry here to avoid noise; use `core::telemetry_tap` if needed.
                },
            }

            telemetry.maybe_emit_health();
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        Ok(())
    }
}

/// Construct a boxed `TelemetryOutNode` from JSON configuration.
///
/// # Errors
///
/// Returns an error if the provided configuration is invalid.
pub fn create_telemetry_out(
    params: Option<&serde_json::Value>,
) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
    Ok(Box::new(TelemetryOutNode::new(params.cloned())?))
}

#[allow(clippy::missing_panics_doc)] // Panics only if JsonSchema-derived config fails to serialize (infallible)
pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    #[allow(clippy::expect_used)] // JsonSchema-derived configs are infallible to serialize
    registry.register_dynamic_with_description(
        "core::telemetry_out",
        create_telemetry_out,
        serde_json::to_value(schemars::schema_for!(TelemetryOutConfig))
            .expect("TelemetryOutConfig schema should serialize to JSON"),
        vec!["core".to_string(), "observability".to_string()],
        false,
        "Consumes packets and emits telemetry events to the session bus (WebSocket). \
         This is a terminal node intended for best-effort side branches.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Tests use unwrap/expect for concise assertions.
mod tests {
    use super::*;
    use std::sync::Arc;
    use streamkit_core::types::{CustomEncoding, TranscriptionSegment};

    #[test]
    fn new_default_config() {
        let node = TelemetryOutNode::new(None).unwrap();
        assert_eq!(node.config.packet_types, vec!["Transcription", "Custom"]);
        assert!(node.config.event_type_filter.is_empty());
        assert_eq!(node.config.max_events_per_sec, 100);
    }

    #[test]
    fn new_custom_config() {
        let params = serde_json::json!({
            "packet_types": ["Text"],
            "event_type_filter": ["vad.*"],
            "max_events_per_sec": 50
        });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert_eq!(node.config.packet_types, vec!["Text"]);
        assert_eq!(node.config.event_type_filter, vec!["vad.*"]);
        assert_eq!(node.config.max_events_per_sec, 50);
    }

    #[test]
    fn new_invalid_config_returns_error() {
        let params = serde_json::json!({ "unknown_field": true });
        assert!(TelemetryOutNode::new(Some(params)).is_err());
    }

    #[test]
    fn create_telemetry_out_no_params() {
        assert!(create_telemetry_out(None).is_ok());
    }

    #[test]
    fn tap_default_transcription() {
        let node = TelemetryOutNode::new(None).unwrap();
        let pkt = Packet::Transcription(Arc::new(TranscriptionData {
            text: "hi".into(),
            segments: vec![],
            language: None,
            metadata: None,
        }));
        assert!(node.should_tap_packet_type(&pkt));
    }

    #[test]
    fn tap_default_custom() {
        let node = TelemetryOutNode::new(None).unwrap();
        let pkt = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "test".into(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({}),
            metadata: None,
        }));
        assert!(node.should_tap_packet_type(&pkt));
    }

    #[test]
    fn tap_default_rejects_text() {
        let node = TelemetryOutNode::new(None).unwrap();
        assert!(!node.should_tap_packet_type(&Packet::Text("hello".into())));
    }

    #[test]
    fn tap_case_insensitive() {
        let params = serde_json::json!({ "packet_types": ["tEXT"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert!(node.should_tap_packet_type(&Packet::Text("hello".into())));
    }

    #[test]
    fn tap_binary_packet() {
        let params = serde_json::json!({ "packet_types": ["Binary"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(b"test"),
            content_type: None,
            metadata: None,
        };
        assert!(node.should_tap_packet_type(&pkt));
    }

    #[test]
    fn filter_empty_matches_all() {
        let node = TelemetryOutNode::new(None).unwrap();
        assert!(node.matches_event_type_filter("anything"));
        assert!(node.matches_event_type_filter("vad.speech_start"));
    }

    #[test]
    fn filter_dot_star_glob() {
        let params = serde_json::json!({ "event_type_filter": ["vad.*"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert!(node.matches_event_type_filter("vad.speech_start"));
        assert!(node.matches_event_type_filter("vad.speech_end"));
        assert!(!node.matches_event_type_filter("vad_something"));
        assert!(!node.matches_event_type_filter("stt.result"));
    }

    #[test]
    fn filter_star_glob() {
        let params = serde_json::json!({ "event_type_filter": ["vad*"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert!(node.matches_event_type_filter("vad.speech_start"));
        assert!(node.matches_event_type_filter("vad_something"));
        assert!(!node.matches_event_type_filter("stt.result"));
    }

    #[test]
    fn filter_exact_match() {
        let params = serde_json::json!({ "event_type_filter": ["stt.result"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert!(node.matches_event_type_filter("stt.result"));
        assert!(!node.matches_event_type_filter("stt.result.extra"));
    }

    #[test]
    fn filter_multiple_patterns() {
        let params = serde_json::json!({ "event_type_filter": ["vad.*", "stt.result"] });
        let node = TelemetryOutNode::new(Some(params)).unwrap();
        assert!(node.matches_event_type_filter("vad.x"));
        assert!(node.matches_event_type_filter("stt.result"));
        assert!(!node.matches_event_type_filter("other.event"));
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(TelemetryOutNode::truncate_preview("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        assert_eq!(TelemetryOutNode::truncate_preview("hello world", 5), "hello...");
    }

    #[test]
    fn truncate_zero_max_returns_empty() {
        assert_eq!(TelemetryOutNode::truncate_preview("anything", 0), "");
    }

    #[test]
    fn truncate_exact_length_no_ellipsis() {
        assert_eq!(TelemetryOutNode::truncate_preview("abcde", 5), "abcde");
    }

    #[test]
    fn transcription_telemetry_structure() {
        let data = TranscriptionData {
            text: "Hello world, this is a test.".into(),
            segments: vec![
                TranscriptionSegment {
                    text: "Hello world,".into(),
                    start_time_ms: 0,
                    end_time_ms: 1000,
                    confidence: Some(0.95),
                },
                TranscriptionSegment {
                    text: "this is a test.".into(),
                    start_time_ms: 1000,
                    end_time_ms: 2000,
                    confidence: None,
                },
            ],
            language: Some("en".into()),
            metadata: None,
        };

        let json = TelemetryOutNode::transcription_to_telemetry(&data);
        assert_eq!(json["text_length"], 28);
        assert_eq!(json["segment_count"], 2);
        assert_eq!(json["language"], "en");
        assert!(json["text_preview"].as_str().unwrap().starts_with("Hello world"));
    }

    #[test]
    fn transcription_telemetry_no_language() {
        let data = TranscriptionData {
            text: "test".into(),
            segments: vec![],
            language: None,
            metadata: None,
        };
        let json = TelemetryOutNode::transcription_to_telemetry(&data);
        assert!(json["language"].is_null());
        assert_eq!(json["segment_count"], 0);
    }

    #[test]
    fn custom_event_type_telemetry_passthrough() {
        let custom = CustomPacketData {
            type_id: TELEMETRY_TYPE_ID.to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({ "event_type": "my.custom.event" }),
            metadata: None,
        };
        assert_eq!(TelemetryOutNode::custom_to_event_type(&custom), "my.custom.event");
    }

    #[test]
    fn custom_event_type_vad_adds_prefix() {
        let custom = CustomPacketData {
            type_id: VAD_EVENT_TYPE_ID.to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({ "event_type": "speech_start" }),
            metadata: None,
        };
        assert_eq!(TelemetryOutNode::custom_to_event_type(&custom), "vad.speech_start");
    }

    #[test]
    fn custom_event_type_vad_already_prefixed() {
        let custom = CustomPacketData {
            type_id: VAD_EVENT_TYPE_ID.to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({ "event_type": "vad.speech_end" }),
            metadata: None,
        };
        assert_eq!(TelemetryOutNode::custom_to_event_type(&custom), "vad.speech_end");
    }

    #[test]
    fn custom_event_type_fallback_unknown() {
        let custom = CustomPacketData {
            type_id: "some::other/type@1".to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({}),
            metadata: None,
        };
        assert_eq!(TelemetryOutNode::custom_to_event_type(&custom), "custom.unknown");
    }

    #[test]
    fn custom_event_type_other_type_with_event_type() {
        let custom = CustomPacketData {
            type_id: "some::other/type@1".to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({ "event_type": "my_event" }),
            metadata: None,
        };
        assert_eq!(TelemetryOutNode::custom_to_event_type(&custom), "my_event");
    }
}
