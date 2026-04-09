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
    /// Smart per-packet-type mapping:
    /// - `Transcription` → `{ "properties": { "text": "<text>" } }`
    /// - `Text` → `{ "properties": { "text": "<text>" } }`
    /// - `Custom` → forward `custom.data` as-is
    #[default]
    Auto,
    /// User-provided JSON template with `{{ field }}` placeholders.
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
    /// Placeholders like `{{ text }}` are replaced with values extracted
    /// from the incoming packet (currently supports `{{ text }}`).
    #[serde(default)]
    pub template: Option<JsonValue>,
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

/// Replace `{{ text }}` placeholders in a JSON value tree.
fn apply_template(template: &JsonValue, text: &str) -> JsonValue {
    match template {
        JsonValue::String(s) => JsonValue::String(s.replace("{{ text }}", text)),
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
                "engine_control_tx not available",
            );
            return Err(StreamKitError::Runtime(
                "param_bridge requires engine_control_tx (only available in dynamic pipelines)"
                    .to_string(),
            ));
        }

        let mut input_rx = context.take_input("in")?;
        state_helpers::emit_running(&context.state_tx, &node_id);

        tracing::info!(
            node = %node_id,
            target_node = %target,
            mode = ?self.config.mode,
            "param_bridge started"
        );

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            let params = match &self.config.mode {
                MappingMode::Auto => auto_map(&packet),
                MappingMode::Template => {
                    let text = extract_text(&packet).unwrap_or_default();
                    self.config.template.as_ref().map(|tmpl| apply_template(tmpl, &text))
                },
                MappingMode::Raw => raw_payload(&packet),
            };

            let Some(params) = params else {
                continue;
            };

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

        state_helpers::emit_stopped(&context.state_tx, &node_id, "input_closed");
        tracing::info!(node = %node_id, "param_bridge stopped");
        Ok(())
    }
}

pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let schema = match serde_json::to_value(schema_for!(ParamBridgeConfig)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize ParamBridgeConfig schema");
            return;
        },
    };

    registry.register_dynamic_with_description(
        "core::param_bridge",
        |params| Ok(Box::new(ParamBridgeNode::new(params)?)),
        schema,
        vec!["core".to_string(), "control".to_string()],
        false,
        "Bridges data-plane packets to control-plane UpdateParams messages. \
         Accepts any packet type and sends a mapped UpdateParams to a configured \
         target node, enabling cross-node control within the pipeline graph. \
         Supports auto, template, and raw mapping modes.",
    );
}
