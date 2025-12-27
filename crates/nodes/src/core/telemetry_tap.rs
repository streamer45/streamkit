// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Telemetry tap node for observing packets and emitting telemetry events.
//!
//! This node acts as a passthrough that observes packets and converts them to
//! telemetry events, enabling visibility into pipeline data flow without modifying
//! the packets themselves.
//!
//! ## Design
//!
//! - **Passthrough is primary**: Packets flow through unmodified
//! - **Side-effect emission**: Converts packets to telemetry bus events
//! - **Does the heavy lifting**: VAD and Whisper nodes don't need changes
//! - **No redaction**: That happens server-side
//!
//! ## Configuration
//!
//! ```yaml
//! - id: tap
//!   kind: core::telemetry_tap
//!   params:
//!     # Which packet types to convert to telemetry
//!     packet_types: ["Transcription", "Custom"]  # default
//!     # Filter Custom packets by event_type pattern
//!     event_type_filter: ["vad.*", "stt.*"]  # empty = all
//!     # Rate limit per event type
//!     max_events_per_sec: 100  # default
//!     # Audio sampling (if Audio is in packet_types)
//!     audio_sample_interval_ms: 1000  # emit aggregate every 1s
//! ```

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::time::Instant;
use streamkit_core::telemetry::TelemetryEmitter;
use streamkit_core::types::{Packet, PacketType, TranscriptionData};
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

const VAD_EVENT_TYPE_ID: &str = "plugin::native::vad/vad-event@1";

/// Configuration for the telemetry tap node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TelemetryTapConfig {
    /// Which packet types to convert to telemetry.
    /// Default: `["Transcription", "Custom"]`
    #[serde(default = "default_packet_types")]
    pub packet_types: Vec<String>,

    /// Filter Custom packets by event_type pattern (glob-style).
    /// Empty list means all Custom packets are included.
    #[serde(default)]
    pub event_type_filter: Vec<String>,

    /// Maximum events per second per event type.
    #[serde(default = "default_max_events_per_sec")]
    pub max_events_per_sec: u32,

    /// Audio sampling interval in milliseconds (for Audio packets).
    /// Set to 0 to disable audio level events.
    #[serde(default = "default_audio_sample_interval_ms")]
    pub audio_sample_interval_ms: u64,
}

fn default_packet_types() -> Vec<String> {
    vec!["Transcription".to_string(), "Custom".to_string()]
}

const fn default_max_events_per_sec() -> u32 {
    100
}

const fn default_audio_sample_interval_ms() -> u64 {
    1000
}

impl Default for TelemetryTapConfig {
    fn default() -> Self {
        Self {
            packet_types: default_packet_types(),
            event_type_filter: Vec::new(),
            max_events_per_sec: default_max_events_per_sec(),
            audio_sample_interval_ms: default_audio_sample_interval_ms(),
        }
    }
}

/// Telemetry tap node that observes packets and emits telemetry events.
#[derive(Default)]
pub struct TelemetryTapNode {
    config: TelemetryTapConfig,
}

impl TelemetryTapNode {
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

    /// Create a new telemetry tap node with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration parameters are invalid JSON.
    pub fn new(params: Option<serde_json::Value>) -> Result<Self, StreamKitError> {
        let config: TelemetryTapConfig = if let Some(params) = params {
            serde_json::from_value(params)
                .map_err(|e| StreamKitError::Configuration(format!("Invalid config: {e}")))?
        } else {
            TelemetryTapConfig::default()
        };

        Ok(Self { config })
    }

    /// Check if a packet type should be tapped.
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

    /// Check if a Custom packet's event_type matches the filter.
    fn matches_event_type_filter(&self, event_type: &str) -> bool {
        if self.config.event_type_filter.is_empty() {
            return true;
        }

        self.config.event_type_filter.iter().any(|pattern| {
            // Simple glob matching: "vad.*" matches "vad.start", "vad.end", etc.
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

    /// Convert a TranscriptionData to telemetry event data.
    fn transcription_to_telemetry(transcription: &TranscriptionData) -> JsonValue {
        let segments: Vec<JsonValue> = transcription
            .segments
            .iter()
            .map(|seg| {
                serde_json::json!({
                    "text": seg.text,
                    "start_time_ms": seg.start_time_ms,
                    "end_time_ms": seg.end_time_ms,
                    "confidence": seg.confidence,
                })
            })
            .collect();

        serde_json::json!({
            "text_preview": Self::truncate_preview(&transcription.text, 100),
            "segment_count": segments.len(),
            "segments": segments,
        })
    }

    /// Calculate RMS (root mean square) level for audio samples.
    #[allow(clippy::cast_precision_loss)] // Audio sample count precision loss is acceptable
    fn calculate_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
        (sum_squares / samples.len() as f32).sqrt()
    }

    /// Calculate peak level for audio samples.
    fn calculate_peak(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max)
    }
}

#[async_trait]
impl ProcessorNode for TelemetryTapNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Create telemetry emitter
        let mut telemetry = TelemetryEmitter::new(
            node_name.clone(),
            context.session_id.clone(),
            context.telemetry_tx.clone(),
        );

        // Configure rate limiting
        telemetry.set_rate_limit(self.config.max_events_per_sec);

        tracing::info!(
            node = %node_name,
            packet_types = ?self.config.packet_types,
            event_type_filter = ?self.config.event_type_filter,
            max_events_per_sec = self.config.max_events_per_sec,
            "TelemetryTapNode starting"
        );

        let mut input_rx = context.take_input("in")?;

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Audio level aggregation state
        let mut audio_samples_acc: Vec<f32> = Vec::new();
        let mut last_audio_emit = Instant::now();
        let audio_interval = std::time::Duration::from_millis(self.config.audio_sample_interval_ms);

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            // Check if we should tap this packet type
            if self.should_tap_packet_type(&packet) {
                match &packet {
                    Packet::Transcription(transcription) => {
                        let data = Self::transcription_to_telemetry(transcription);
                        telemetry.emit("stt.result", data);
                    },
                    Packet::Custom(custom) => {
                        // Extract event_type from custom packet data
                        let event_type = custom
                            .data
                            .get("event_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("custom.unknown");

                        let telemetry_event_type = if custom.type_id == VAD_EVENT_TYPE_ID
                            && !event_type.starts_with("vad.")
                        {
                            format!("vad.{event_type}")
                        } else {
                            event_type.to_string()
                        };

                        if self.matches_event_type_filter(&telemetry_event_type) {
                            // Forward the custom packet data as telemetry, preserving source type_id.
                            let mut data = custom.data.clone();
                            if let Some(obj) = data.as_object_mut() {
                                obj.insert(
                                    "source_type_id".to_string(),
                                    JsonValue::String(custom.type_id.clone()),
                                );
                                if telemetry_event_type != event_type {
                                    obj.insert(
                                        "source_event_type".to_string(),
                                        JsonValue::String(event_type.to_string()),
                                    );
                                }
                            }

                            telemetry.emit(&telemetry_event_type, data);
                        }
                    },
                    Packet::Audio(frame) => {
                        // Aggregate audio samples for periodic level reporting
                        if self.config.audio_sample_interval_ms > 0 {
                            audio_samples_acc.extend_from_slice(&frame.samples);

                            if last_audio_emit.elapsed() >= audio_interval {
                                let rms = Self::calculate_rms(&audio_samples_acc);
                                let peak = Self::calculate_peak(&audio_samples_acc);

                                telemetry.emit(
                                    "audio.level",
                                    serde_json::json!({
                                        "rms": rms,
                                        "peak": peak,
                                        "sample_count": audio_samples_acc.len(),
                                        "sample_rate": frame.sample_rate,
                                        "channels": frame.channels,
                                    }),
                                );

                                audio_samples_acc.clear();
                                last_audio_emit = Instant::now();
                            }
                        }
                    },
                    Packet::Text(text) => {
                        let preview = Self::truncate_preview(text, 100);
                        telemetry.emit(
                            "text.received",
                            serde_json::json!({
                                "text_preview": preview,
                                "length": text.len(),
                            }),
                        );
                    },
                    Packet::Binary { data, metadata, .. } => {
                        telemetry.emit(
                            "binary.received",
                            serde_json::json!({
                                "size_bytes": data.len(),
                                "has_metadata": metadata.is_some(),
                            }),
                        );
                    },
                }
            }

            // Periodically emit health events
            telemetry.maybe_emit_health();

            // Forward the packet unchanged
            if context.output_sender.send("out", packet).await.is_err() {
                tracing::debug!("Output channel closed, stopping node");
                break;
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!(node = %node_name, "TelemetryTapNode shutting down");
        Ok(())
    }
}

/// Factory function for creating the telemetry tap node.
///
/// # Errors
///
/// Returns an error if the configuration parameters are invalid JSON.
pub fn create_telemetry_tap(
    params: Option<&serde_json::Value>,
) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
    Ok(Box::new(TelemetryTapNode::new(params.cloned())?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use streamkit_core::types::AudioFrame;

    #[test]
    fn test_config_defaults() {
        let config = TelemetryTapConfig::default();
        assert_eq!(config.packet_types, vec!["Transcription", "Custom"]);
        assert!(config.event_type_filter.is_empty());
        assert_eq!(config.max_events_per_sec, 100);
        assert_eq!(config.audio_sample_interval_ms, 1000);
    }

    #[test]
    fn test_event_type_filter_matching() {
        let config = TelemetryTapConfig {
            event_type_filter: vec!["vad.*".to_string(), "stt.result".to_string()],
            ..Default::default()
        };
        let node = TelemetryTapNode { config };

        assert!(node.matches_event_type_filter("vad.start"));
        assert!(node.matches_event_type_filter("vad.end"));
        assert!(node.matches_event_type_filter("stt.result"));
        assert!(!node.matches_event_type_filter("llm.response"));
    }

    #[test]
    fn test_empty_filter_matches_all() {
        let config = TelemetryTapConfig::default();
        let node = TelemetryTapNode { config };

        assert!(node.matches_event_type_filter("anything"));
        assert!(node.matches_event_type_filter("vad.start"));
    }

    #[test]
    fn test_rms_calculation() {
        let samples = vec![0.5, -0.5, 0.5, -0.5];
        let rms = TelemetryTapNode::calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 0.001);

        let empty: Vec<f32> = vec![];
        assert_eq!(TelemetryTapNode::calculate_rms(&empty), 0.0);
    }

    #[test]
    fn test_peak_calculation() {
        let samples = vec![0.1, -0.8, 0.3, -0.2];
        let peak = TelemetryTapNode::calculate_peak(&samples);
        assert_eq!(peak, 0.8);
    }

    #[test]
    fn test_packet_type_filtering() {
        let config = TelemetryTapConfig {
            packet_types: vec!["Audio".to_string(), "Custom".to_string()],
            ..Default::default()
        };
        let node = TelemetryTapNode { config };

        // Create test packets
        let audio = Packet::Audio(AudioFrame::new(48000, 2, vec![0.0; 100]));
        let text = Packet::Text("test".into());

        assert!(node.should_tap_packet_type(&audio));
        assert!(!node.should_tap_packet_type(&text));
    }
}
