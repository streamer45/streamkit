// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Telemetry output node
//!
//! Consumes packets and emits telemetry events to the session telemetry bus (WebSocket).
//! This is a terminal node (no outputs) intended for side branches.

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
            Packet::Transcription(_) => "Transcription",
            Packet::Custom(_) => "Custom",
            Packet::Binary { .. } => "Binary",
            Packet::Text(_) => "Text",
        };

        self.config.packet_types.iter().any(|t| t.eq_ignore_ascii_case(type_name))
    }

    fn matches_event_type_filter(&self, event_type: &str) -> bool {
        if self.config.event_type_filter.is_empty() {
            return true;
        }

        self.config.event_type_filter.iter().any(|pattern| {
            if pattern.ends_with(".*") {
                let prefix = &pattern[..pattern.len() - 2];
                event_type.starts_with(prefix)
            } else if pattern.ends_with('*') {
                let prefix = &pattern[..pattern.len() - 1];
                event_type.starts_with(prefix)
            } else {
                event_type == pattern
            }
        })
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
                Packet::Audio(_) => {
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

pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let schema = match serde_json::to_value(schema_for!(TelemetryOutConfig)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize TelemetryOutConfig schema");
            return;
        },
    };

    registry.register_dynamic_with_description(
        "core::telemetry_out",
        create_telemetry_out,
        schema,
        vec!["core".to_string(), "observability".to_string()],
        false,
        "Consumes packets and emits telemetry events to the session bus (WebSocket). \
         This is a terminal node intended for best-effort side branches.",
    );
}
