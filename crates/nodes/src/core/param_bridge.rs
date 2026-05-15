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
use streamkit_core::control::NodeControlMessage;
use streamkit_core::telemetry::TelemetryEmitter;
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

/// Replace `{{ field }}` placeholders in a JSON value tree using values
/// from a context object.
///
/// When a string value consists entirely of a single placeholder
/// (e.g. `"{{ is_speech }}"`) the raw JSON value from the context is
/// substituted — preserving booleans, numbers, and nulls.  When the
/// placeholder appears inside a longer string (e.g.
/// `"Hello {{ name }}"`) the context value is stringified.
///
/// Transcription and Text packets produce a context with a single
/// `text` key.  Custom packets use their full JSON `.data` object.
fn apply_template(template: &JsonValue, ctx: &JsonValue) -> JsonValue {
    match template {
        JsonValue::String(s) => {
            // Fast path: check if the entire string is a single {{ field }}.
            let trimmed = s.trim();
            if let Some(field) = parse_sole_placeholder(trimmed) {
                if let Some(val) = lookup_ctx(ctx, field) {
                    return val.clone();
                }
            }
            // General path: replace all {{ field }} occurrences as strings.
            // We track a cursor to advance past each replacement so that
            // placeholders inside substituted text are never re-scanned
            // (prevents infinite loops when replacement contains `{{ … }}`).
            let mut result = s.clone();
            let mut cursor = 0;
            while cursor < result.len() {
                let Some(start) = result[cursor..].find("{{").map(|i| cursor + i) else {
                    break;
                };
                let Some(end) = result[start..].find("}}") else { break };
                let end = start + end + 2;
                let field = result[start + 2..end - 2].trim();
                let replacement = lookup_ctx(ctx, field).map_or_else(String::new, |v| match v {
                    JsonValue::String(s) => s.clone(),
                    other => other.to_string(),
                });
                let replacement_len = replacement.len();
                result.replace_range(start..end, &replacement);
                cursor = start + replacement_len;
            }
            JsonValue::String(result)
        },
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(|v| apply_template(v, ctx)).collect())
        },
        JsonValue::Object(map) => JsonValue::Object(
            map.iter().map(|(k, v)| (k.clone(), apply_template(v, ctx))).collect(),
        ),
        other => other.clone(),
    }
}

/// If the string is exactly `{{ field }}` (or `{{field}}`), return the
/// field name; otherwise `None`.
fn parse_sole_placeholder(s: &str) -> Option<&str> {
    let s = s.strip_prefix("{{")?;
    let s = s.strip_suffix("}}")?;
    // Ensure there are no nested braces.
    if s.contains("{{") || s.contains("}}") {
        return None;
    }
    Some(s.trim())
}

/// Look up a field name in a JSON context value.
fn lookup_ctx<'a>(ctx: &'a JsonValue, field: &str) -> Option<&'a JsonValue> {
    match ctx {
        JsonValue::Object(map) => map.get(field),
        _ => None,
    }
}

/// Extract the raw JSON payload from a packet (for raw mode).
///
/// **Note:** `Transcription` packets serialize the full `TranscriptionData`
/// struct (including per-segment timing and confidence).  For transcriptions
/// with many segments this can produce a non-trivial JSON tree — prefer
/// `auto` or `template` mode for the subtitle use case.
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

        let telemetry = TelemetryEmitter::new(
            node_id.clone(),
            context.session_id.clone(),
            context.telemetry_tx.clone(),
        );

        // Take control_rx out of context so we can select on it alongside
        // recv_with_cancellation (which borrows context immutably).  The
        // dummy channel is a one-time allocation that is never read —
        // other nodes avoid this because they don't use
        // recv_with_cancellation.
        let mut control_rx = {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            std::mem::replace(&mut context.control_rx, rx)
        };

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

        // When debouncing is enabled we store the most recent params (and the
        // pre-mapping text preview for telemetry) and only send after the
        // window elapses without a new packet arriving.
        let mut pending_params: Option<(JsonValue, Option<String>)> = None;

        // Dedup: skip UpdateParams that are identical to the last-sent value.
        // This avoids redundant Slint re-renders when Whisper emits duplicate
        // segments during VAD boundary refinement.
        let mut last_sent: Option<JsonValue> = None;
        let sleep = tokio::time::sleep(tokio::time::Duration::MAX);
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                biased;

                Some(ctrl) = control_rx.recv() => {
                    match ctrl {
                        NodeControlMessage::Shutdown => {
                            tracing::info!(node = %node_id, "param_bridge received shutdown");
                            break;
                        },
                        NodeControlMessage::UpdateParams(_) | NodeControlMessage::Start => {},
                    }
                }

                packet = context.recv_with_cancellation(&mut input_rx) => {
                    let Some(packet) = packet else {
                        break;
                    };

                    // Extract text preview for telemetry — done before mapping
                    // so it's independent of the target-specific JSON shape.
                    let text_preview = extract_text(&packet);

                    let params = match &self.config.mode {
                        MappingMode::Auto => auto_map(&packet),
                        MappingMode::Template => {
                            // Build a context object for template substitution.
                            // Text-bearing packets get a `{ "text": "..." }`
                            // context; Custom packets use their full JSON data.
                            let ctx = if let Some(ref text) = text_preview {
                                serde_json::json!({ "text": text })
                            } else if let Packet::Custom(c) = &packet {
                                c.data.clone()
                            } else {
                                tracing::debug!(packet_type = %packet_type_label(&packet), "param_bridge template: unsupported packet type, skipping");
                                continue;
                            };
                            self.config.template.as_ref().map(|tmpl| apply_template(tmpl, &ctx))
                        },
                        MappingMode::Raw => raw_payload(&packet),
                    };

                    let Some(params) = params else {
                        continue;
                    };

                    if let Some(d) = debounce {
                        pending_params = Some((params, text_preview));
                        sleep.as_mut().reset(tokio::time::Instant::now() + d);
                    } else {
                        // Dedup: skip if identical to last sent params.
                        if last_sent.as_ref() == Some(&params) {
                            continue;
                        }
                        last_sent = Some(params.clone());
                        Self::send_params(&context, &telemetry, &node_id, target, params, text_preview.as_deref()).await;
                    }
                }

                () = &mut sleep, if pending_params.is_some() => {
                    if let Some((params, text_preview)) = pending_params.take() {
                        // Dedup: skip if identical to last sent params.
                        if last_sent.as_ref() != Some(&params) {
                            last_sent = Some(params.clone());
                            Self::send_params(&context, &telemetry, &node_id, target, params, text_preview.as_deref()).await;
                        }
                    }
                    // Reset sleep to far future so it doesn't fire again.
                    // Cannot use Duration::MAX — Instant + Duration::MAX overflows.
                    sleep.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_hours(8760));
                }
            }
        }

        // Flush any pending debounced params before shutting down.
        if let Some((params, text_preview)) = pending_params.take() {
            if last_sent.as_ref() != Some(&params) {
                Self::send_params(
                    &context,
                    &telemetry,
                    &node_id,
                    target,
                    params,
                    text_preview.as_deref(),
                )
                .await;
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_id, "input_closed");
        tracing::info!(node = %node_id, "param_bridge stopped");
        Ok(())
    }
}

impl ParamBridgeNode {
    async fn send_params(
        context: &NodeContext,
        telemetry: &TelemetryEmitter,
        node_id: &str,
        target: &str,
        params: JsonValue,
        text_preview: Option<&str>,
    ) {
        tracing::debug!(
            node = %node_id,
            target_node = %target,
            "param_bridge sending UpdateParams"
        );

        // Emit telemetry so the stream view can display forwarded text.
        // text_preview is extracted from the packet before mapping, so it
        // works regardless of the target node's expected JSON shape.
        if let Some(text) = text_preview {
            telemetry.emit(
                "stt.result",
                serde_json::json!({
                    "text_preview": text,
                    "target_node": target,
                }),
            );
        }

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

#[allow(clippy::missing_panics_doc)] // Panics only if JsonSchema-derived config fails to serialize (infallible)
pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    use streamkit_core::registry::StaticPins;

    register_static_node!(
        registry,
        "core::param_bridge",
        |params| Ok(Box::new(ParamBridgeNode::new(params)?)),
        ParamBridgeConfig,
        StaticPins { inputs: ParamBridgeNode::input_pins(), outputs: vec![] },
        ["core", "control"],
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

    /// Helper: build a text-only context for template tests.
    fn text_ctx(s: &str) -> JsonValue {
        json!({ "text": s })
    }

    #[test]
    fn apply_template_string_replacement() {
        let tmpl = json!("prefix: {{ text }}");
        let result = apply_template(&tmpl, &text_ctx("hello"));
        assert_eq!(result, json!("prefix: hello"));
    }

    #[test]
    fn apply_template_no_whitespace_placeholder() {
        let tmpl = json!("prefix: {{text}}");
        let result = apply_template(&tmpl, &text_ctx("hello"));
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
        let result = apply_template(&tmpl, &text_ctx("subtitle line"));
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
        let result = apply_template(&tmpl, &text_ctx("dynamic"));
        assert_eq!(result, json!(["dynamic", "static"]));
    }

    #[test]
    fn apply_template_no_placeholder() {
        let tmpl = json!({"key": "no placeholder here"});
        let result = apply_template(&tmpl, &text_ctx("ignored"));
        assert_eq!(result, json!({"key": "no placeholder here"}));
    }

    #[test]
    fn apply_template_empty_text() {
        let tmpl = json!("{{ text }}");
        let result = apply_template(&tmpl, &text_ctx(""));
        assert_eq!(result, json!(""));
    }

    #[test]
    fn apply_template_preserves_non_string_values() {
        let tmpl = json!({"count": 42, "flag": true, "text": "{{ text }}"});
        let result = apply_template(&tmpl, &text_ctx("hello"));
        assert_eq!(result, json!({"count": 42, "flag": true, "text": "hello"}));
    }

    #[test]
    fn apply_template_text_containing_placeholder_literal() {
        // Regression: if substituted text contains "{{text}}", the replacement
        // must NOT re-scan it (would cause infinite loop / double-replace).
        let tmpl = json!("{{ text }}");
        let result = apply_template(&tmpl, &text_ctx("contains {{text}} marker"));
        assert_eq!(result, json!("contains {{text}} marker"));
    }

    #[test]
    fn apply_template_no_infinite_loop_on_replacement_with_placeholder() {
        // Regression: the general replacement path (not sole-placeholder fast
        // path) must advance past each replacement to avoid re-scanning
        // substituted text that itself contains {{ field }} patterns.
        let tmpl = json!("Say: {{ text }}!");
        let result = apply_template(&tmpl, &text_ctx("hello {{text}} world"));
        assert_eq!(result, json!("Say: hello {{text}} world!"));
    }

    #[test]
    fn apply_template_sole_placeholder_preserves_type() {
        // When a placeholder is the entire value, the raw JSON type is kept.
        let ctx = json!({ "is_speech": true, "score": 42 });
        assert_eq!(apply_template(&json!("{{ is_speech }}"), &ctx), json!(true));
        assert_eq!(apply_template(&json!("{{ score }}"), &ctx), json!(42));
    }

    #[test]
    fn apply_template_custom_fields_in_object() {
        let tmpl = json!({ "properties": { "speaking": "{{ is_speech }}" } });
        let ctx = json!({ "is_speech": true });
        assert_eq!(apply_template(&tmpl, &ctx), json!({ "properties": { "speaking": true } }));
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
