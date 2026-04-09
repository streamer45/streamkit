// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Parameter bridge node
//!
//! Accepts packets on its input and converts them into `UpdateParams` control
//! messages sent to a configured sibling node via
//! [`NodeContext::tune_sibling()`].  This enables cross-node control within the
//! pipeline graph — the same mechanism the WebSocket/REST API uses, but
//! initiated from inside the data flow.
//!
//! Three mapping modes are supported:
//!
//! - **Auto** — smart per-packet-type mapping (e.g. `Transcription.text` →
//!   `{ "properties": { "text": "..." } }`).
//! - **Template** — a user-supplied JSON template with `{{ field }}` placeholders
//!   replaced by values extracted from the incoming packet.
//! - **Raw** — forward the packet payload as-is (useful after a `core::script`
//!   node that already produced the desired JSON shape).
//!
//! This is a terminal node (no output pins) and is designed for `best_effort`
//! side branches so it never stalls the main data flow.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

/// How the bridge maps incoming packets to `UpdateParams` JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MappingMode {
    /// Smart per-packet-type mapping.
    ///
    /// `Transcription` and `Text` packets are wrapped in
    /// `{ "properties": { "text": "..." } }` — a shape that targets Slint
    /// plugin nodes out of the box.  `Custom` packets forward their `data`
    /// field as-is (assumed to already be the correct `UpdateParams` shape).
    ///
    /// If you need a different output shape (e.g. targeting a compositor's
    /// `text_overlays`), use `template` mode instead.
    #[default]
    Auto,
    /// User-provided JSON template with `{{ text }}` placeholders.
    Template,
    /// Forward the extracted payload as-is (no transformation).
    Raw,
}

/// Configuration for the `core::param_bridge` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamBridgeConfig {
    /// The `node_id` of the sibling node to send `UpdateParams` to.
    pub target_node: String,

    /// Mapping strategy.
    #[serde(default)]
    pub mode: MappingMode,

    /// JSON template used when `mode` is `template`.
    ///
    /// Placeholders like `{{ text }}` (or `{{text}}`) are replaced with values
    /// extracted from the incoming packet.
    ///
    /// Currently only `{{ text }}` is supported.  Future extensions could add
    /// `{{ language }}`, `{{ confidence }}`, or arbitrary field paths.
    #[serde(default)]
    pub template: Option<JsonValue>,

    /// Optional debounce window in milliseconds.
    ///
    /// When set, rapid `UpdateParams` messages are coalesced: only the most
    /// recent value is sent after the window expires.  This is useful for
    /// targets like subtitles where intermediate transcription segments are
    /// superseded by newer ones.
    #[serde(default)]
    pub debounce_ms: Option<u64>,
}

pub struct ParamBridgeNode {
    config: ParamBridgeConfig,
}

impl ParamBridgeNode {
    /// Creates a new `ParamBridgeNode` from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration parameters cannot be parsed.
    pub fn new(params: Option<&serde_json::Value>) -> Result<Self, StreamKitError> {
        let config: ParamBridgeConfig = if let Some(p) = params {
            serde_json::from_value(p.clone())
                .map_err(|e| StreamKitError::Configuration(format!("Invalid config: {e}")))?
        } else {
            return Err(StreamKitError::Configuration(
                "param_bridge requires at least `target_node` in params".to_string(),
            ));
        };

        if matches!(config.mode, MappingMode::Template) && config.template.is_none() {
            return Err(StreamKitError::Configuration(
                "param_bridge: `template` is required when mode is `template`".to_string(),
            ));
        }

        Ok(Self { config })
    }

    pub fn input_pins() -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }
}

/// Extract the text content from a packet (for auto/template modes).
fn extract_text(packet: &Packet) -> Option<String> {
    match packet {
        Packet::Transcription(t) => Some(t.text.clone()),
        Packet::Text(t) => Some(t.to_string()),
        _ => None,
    }
}

/// Build `UpdateParams` JSON using the auto-mapping strategy.
fn auto_map(packet: &Packet) -> Option<JsonValue> {
    match packet {
        Packet::Transcription(t) => Some(serde_json::json!({ "properties": { "text": t.text } })),
        Packet::Text(t) => Some(serde_json::json!({ "properties": { "text": t.as_ref() } })),
        Packet::Custom(c) => Some(c.data.clone()),
        _ => {
            tracing::debug!(packet_type = %packet_type_label(packet), "param_bridge auto: unsupported packet type, skipping");
            None
        },
    }
}

/// Replace `{{ text }}` (and `{{text}}`) placeholders in a JSON value tree.
///
/// Currently only the `text` placeholder is supported.  To add more fields
/// (e.g. `{{ language }}`, `{{ confidence }}`), extend the replacement list
/// here and extract the additional values in [`extract_text`] or a new
/// dedicated extraction helper.
fn apply_template(template: &JsonValue, text: &str) -> JsonValue {
    match template {
        JsonValue::String(s) => {
            let normalized = s.replace("{{ text }}", "{{text}}");
            JsonValue::String(normalized.replace("{{text}}", text))
        },
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(|v| apply_template(v, text)).collect())
        },
        JsonValue::Object(map) => JsonValue::Object(
            map.iter().map(|(k, v)| (k.clone(), apply_template(v, text))).collect(),
        ),
        other => other.clone(),
    }
}

/// Extract the raw JSON payload from a packet (for raw mode).
fn raw_payload(packet: &Packet) -> Option<JsonValue> {
    match packet {
        Packet::Custom(c) => Some(c.data.clone()),
        Packet::Transcription(t) => serde_json::to_value(t.as_ref()).ok(),
        Packet::Text(t) => Some(serde_json::json!({ "text": t.as_ref() })),
        _ => {
            tracing::debug!(packet_type = %packet_type_label(packet), "param_bridge raw: unsupported packet type, skipping");
            None
        },
    }
}

const fn packet_type_label(packet: &Packet) -> &'static str {
    match packet {
        Packet::Audio(_) => "Audio",
        Packet::Video(_) => "Video",
        Packet::Text(_) => "Text",
        Packet::Transcription(_) => "Transcription",
        Packet::Custom(_) => "Custom",
        Packet::Binary { .. } => "Binary",
    }
}

#[async_trait]
impl ProcessorNode for ParamBridgeNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Self::input_pins()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_id = context.output_sender.node_name().to_string();
        let target = &self.config.target_node;

        state_helpers::emit_initializing(&context.state_tx, &node_id);

        if context.engine_control_tx.is_none() {
            tracing::error!(
                node = %node_id,
                "param_bridge requires engine_control_tx (only available in dynamic pipelines)"
            );
            state_helpers::emit_failed(
                &context.state_tx,
                &node_id,
                "engine_control_tx not available (oneshot pipeline?)",
            );
            return Err(StreamKitError::Runtime(
                "engine_control_tx not available (oneshot pipeline?)".to_string(),
            ));
        }

        let mut input_rx = context.take_input("in")?;
        state_helpers::emit_running(&context.state_tx, &node_id);

        let debounce = self.config.debounce_ms.map(tokio::time::Duration::from_millis);

        tracing::info!(
            node = %node_id,
            target_node = %target,
            mode = ?self.config.mode,
            debounce_ms = ?self.config.debounce_ms,
            "param_bridge started"
        );

        // When debouncing is enabled we store the most recent params and only
        // send after the window elapses without a new packet arriving.
        let mut pending_params: Option<JsonValue> = None;
        let sleep = tokio::time::sleep(tokio::time::Duration::MAX);
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                biased;

                packet = context.recv_with_cancellation(&mut input_rx) => {
                    let Some(packet) = packet else {
                        break;
                    };

                    let params = match &self.config.mode {
                        MappingMode::Auto => auto_map(&packet),
                        MappingMode::Template => {
                            let Some(text) = extract_text(&packet) else {
                                tracing::debug!(packet_type = %packet_type_label(&packet), "param_bridge template: unsupported packet type, skipping");
                                continue;
                            };
                            self.config.template.as_ref().map(|tmpl| apply_template(tmpl, &text))
                        },
                        MappingMode::Raw => raw_payload(&packet),
                    };

                    let Some(params) = params else {
                        continue;
                    };

                    if let Some(d) = debounce {
                        pending_params = Some(params);
                        sleep.as_mut().reset(tokio::time::Instant::now() + d);
                    } else {
                        Self::send_params(&context, &node_id, target, params).await;
                    }
                }

                () = &mut sleep, if pending_params.is_some() => {
                    if let Some(params) = pending_params.take() {
                        Self::send_params(&context, &node_id, target, params).await;
                    }
                    // Reset sleep to far future so it doesn't fire again.
                    sleep.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_secs(86400));
                }
            }
        }

        // Flush any pending debounced params before shutting down.
        if let Some(params) = pending_params.take() {
            Self::send_params(&context, &node_id, target, params).await;
        }

        state_helpers::emit_stopped(&context.state_tx, &node_id, "input_closed");
        tracing::info!(node = %node_id, "param_bridge stopped");
        Ok(())
    }
}

impl ParamBridgeNode {
    async fn send_params(context: &NodeContext, node_id: &str, target: &str, params: JsonValue) {
        tracing::debug!(
            node = %node_id,
            target_node = %target,
            "param_bridge sending UpdateParams"
        );

        if let Err(e) = context.tune_sibling(target, params).await {
            tracing::warn!(
                node = %node_id,
                target_node = %target,
                error = %e,
                "param_bridge failed to send UpdateParams"
            );
        }
    }
}

pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;
    use streamkit_core::registry::StaticPins;

    let schema = match serde_json::to_value(schema_for!(ParamBridgeConfig)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize ParamBridgeConfig schema");
            return;
        },
    };

    registry.register_static_with_description(
        "core::param_bridge",
        |params| Ok(Box::new(ParamBridgeNode::new(params)?)),
        schema,
        StaticPins { inputs: ParamBridgeNode::input_pins(), outputs: vec![] },
        vec!["core".to_string(), "control".to_string()],
        false,
        "Bridges data-plane packets to control-plane UpdateParams messages. \
         Accepts any packet type and sends a mapped UpdateParams to a configured \
         target node, enabling cross-node control within the pipeline graph. \
         Supports auto, template, and raw mapping modes.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use serde_json::json;
    use streamkit_core::types::{
        CustomEncoding, CustomPacketData, TranscriptionData, TranscriptionSegment,
    };

    // ── extract_text ────────────────────────────────────────────────

    #[test]
    fn extract_text_from_transcription() {
        let pkt = Packet::Transcription(Arc::new(TranscriptionData {
            text: "hello world".into(),
            segments: vec![],
            language: None,
            metadata: None,
        }));
        assert_eq!(extract_text(&pkt), Some("hello world".into()));
    }

    #[test]
    fn extract_text_from_text_packet() {
        let pkt = Packet::Text("some text".into());
        assert_eq!(extract_text(&pkt), Some("some text".into()));
    }

    #[test]
    fn extract_text_from_empty_transcription() {
        let pkt = Packet::Transcription(Arc::new(TranscriptionData {
            text: String::new(),
            segments: vec![],
            language: None,
            metadata: None,
        }));
        assert_eq!(extract_text(&pkt), Some(String::new()));
    }

    #[test]
    fn extract_text_returns_none_for_custom() {
        let pkt = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "test".into(),
            encoding: CustomEncoding::Json,
            data: json!({"key": "value"}),
            metadata: None,
        }));
        assert_eq!(extract_text(&pkt), None);
    }

    // ── auto_map ────────────────────────────────────────────────────

    #[test]
    fn auto_map_transcription() {
        let pkt = Packet::Transcription(Arc::new(TranscriptionData {
            text: "hi".into(),
            segments: vec![],
            language: None,
            metadata: None,
        }));
        let result = auto_map(&pkt).unwrap();
        assert_eq!(result, json!({ "properties": { "text": "hi" } }));
    }

    #[test]
    fn auto_map_text() {
        let pkt = Packet::Text("hello".into());
        let result = auto_map(&pkt).unwrap();
        assert_eq!(result, json!({ "properties": { "text": "hello" } }));
    }

    #[test]
    fn auto_map_custom_forwards_data() {
        let data = json!({"props": {"color": "red"}});
        let pkt = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "test".into(),
            encoding: CustomEncoding::Json,
            data: data.clone(),
            metadata: None,
        }));
        assert_eq!(auto_map(&pkt).unwrap(), data);
    }

    #[test]
    fn auto_map_returns_none_for_unsupported() {
        // Binary is unsupported in auto mode.
        let pkt = Packet::Binary { data: bytes::Bytes::new(), content_type: None, metadata: None };
        assert!(auto_map(&pkt).is_none());
    }

    // ── apply_template ──────────────────────────────────────────────

    #[test]
    fn apply_template_string_replacement() {
        let tmpl = json!("prefix: {{ text }}");
        let result = apply_template(&tmpl, "hello");
        assert_eq!(result, json!("prefix: hello"));
    }

    #[test]
    fn apply_template_no_whitespace_placeholder() {
        let tmpl = json!("prefix: {{text}}");
        let result = apply_template(&tmpl, "hello");
        assert_eq!(result, json!("prefix: hello"));
    }

    #[test]
    fn apply_template_nested_object() {
        let tmpl = json!({
            "properties": {
                "text": "{{ text }}",
                "visible": true
            }
        });
        let result = apply_template(&tmpl, "subtitle line");
        assert_eq!(
            result,
            json!({
                "properties": {
                    "text": "subtitle line",
                    "visible": true
                }
            })
        );
    }

    #[test]
    fn apply_template_array() {
        let tmpl = json!(["{{ text }}", "static"]);
        let result = apply_template(&tmpl, "dynamic");
        assert_eq!(result, json!(["dynamic", "static"]));
    }

    #[test]
    fn apply_template_no_placeholder() {
        let tmpl = json!({"key": "no placeholder here"});
        let result = apply_template(&tmpl, "ignored");
        assert_eq!(result, json!({"key": "no placeholder here"}));
    }

    #[test]
    fn apply_template_empty_text() {
        let tmpl = json!("{{ text }}");
        let result = apply_template(&tmpl, "");
        assert_eq!(result, json!(""));
    }

    #[test]
    fn apply_template_preserves_non_string_values() {
        let tmpl = json!({"count": 42, "flag": true, "text": "{{ text }}"});
        let result = apply_template(&tmpl, "hello");
        assert_eq!(result, json!({"count": 42, "flag": true, "text": "hello"}));
    }

    #[test]
    fn apply_template_text_containing_placeholder_literal() {
        // Regression: if substituted text contains "{{text}}", the second
        // replace pass must NOT re-replace it.
        let tmpl = json!("{{ text }}");
        let result = apply_template(&tmpl, "contains {{text}} marker");
        assert_eq!(result, json!("contains {{text}} marker"));
    }

    // ── raw_payload ─────────────────────────────────────────────────

    #[test]
    fn raw_payload_custom() {
        let data = json!({"properties": {"text": "direct"}});
        let pkt = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "test".into(),
            encoding: CustomEncoding::Json,
            data: data.clone(),
            metadata: None,
        }));
        assert_eq!(raw_payload(&pkt).unwrap(), data);
    }

    #[test]
    fn raw_payload_text() {
        let pkt = Packet::Text("raw text".into());
        assert_eq!(raw_payload(&pkt).unwrap(), json!({"text": "raw text"}));
    }

    #[test]
    fn raw_payload_transcription() {
        let pkt = Packet::Transcription(Arc::new(TranscriptionData {
            text: "hello".into(),
            segments: vec![TranscriptionSegment {
                text: "hello".into(),
                start_time_ms: 0,
                end_time_ms: 1000,
                confidence: Some(0.95),
            }],
            language: Some("en".into()),
            metadata: None,
        }));
        let result = raw_payload(&pkt).unwrap();
        assert_eq!(result["text"], "hello");
        assert_eq!(result["language"], "en");
    }

    #[test]
    fn raw_payload_returns_none_for_unsupported() {
        let pkt = Packet::Binary { data: bytes::Bytes::new(), content_type: None, metadata: None };
        assert!(raw_payload(&pkt).is_none());
    }

    // ── ParamBridgeNode::new (config validation) ────────────────────

    #[test]
    fn config_requires_params() {
        assert!(ParamBridgeNode::new(None).is_err());
    }

    #[test]
    fn config_requires_target_node() {
        let params = json!({"mode": "auto"});
        assert!(ParamBridgeNode::new(Some(&params)).is_err());
    }

    #[test]
    fn config_template_mode_requires_template() {
        let params = json!({"target_node": "foo", "mode": "template"});
        assert!(ParamBridgeNode::new(Some(&params)).is_err());
    }

    #[test]
    fn config_template_mode_with_template_ok() {
        let params = json!({
            "target_node": "sub",
            "mode": "template",
            "template": {"properties": {"text": "{{ text }}"}}
        });
        assert!(ParamBridgeNode::new(Some(&params)).is_ok());
    }

    #[test]
    fn config_auto_mode_defaults() {
        let params = json!({"target_node": "target"});
        let node = ParamBridgeNode::new(Some(&params)).unwrap();
        assert!(matches!(node.config.mode, MappingMode::Auto));
    }

    #[test]
    fn config_rejects_unknown_fields() {
        let params = json!({"target_node": "foo", "unknown_field": true});
        assert!(ParamBridgeNode::new(Some(&params)).is_err());
    }

    #[test]
    fn config_debounce_ms() {
        let params = json!({"target_node": "t", "debounce_ms": 100});
        let node = ParamBridgeNode::new(Some(&params)).unwrap();
        assert_eq!(node.config.debounce_ms, Some(100));
    }
}
