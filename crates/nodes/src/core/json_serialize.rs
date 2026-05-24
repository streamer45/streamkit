// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};

#[derive(Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JsonSerializeConfig {
    /// Enable pretty-printing (formatted with indentation)
    #[serde(default)]
    pub pretty: bool,
    /// Add newline after each JSON object (for NDJSON format)
    #[serde(default)]
    pub newline_delimited: bool,
}

pub struct JsonSerialize {
    pretty: bool,
    newline_delimited: bool,
}

impl JsonSerialize {
    /// # Errors
    /// Returns `Err` if config parsing fails.
    pub fn new(params: Option<&serde_json::Value>) -> Result<Self, StreamKitError> {
        let config: JsonSerializeConfig = config_helpers::parse_config_optional(params)?;

        Ok(Self { pretty: config.pretty, newline_delimited: config.newline_delimited })
    }

    pub fn input_pins() -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    pub fn output_pins() -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }
}

#[async_trait]
impl ProcessorNode for JsonSerialize {
    fn input_pins(&self) -> Vec<InputPin> {
        Self::input_pins()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        Self::output_pins()
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_id = context.output_sender.node_name().to_string();
        state_helpers::emit_running(&context.state_tx, &node_id);

        let mut input = context.take_input("in")?;

        while let Some(packet) = context.recv_with_cancellation(&mut input).await {
            let mut json_bytes = if self.pretty {
                serde_json::to_vec_pretty(&packet)
            } else {
                serde_json::to_vec(&packet)
            }
            .map_err(|e| {
                StreamKitError::Runtime(format!("Failed to serialize packet to JSON: {e}"))
            })?;

            if self.newline_delimited {
                json_bytes.push(b'\n');
            }

            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary {
                        data: Bytes::from(json_bytes),
                        content_type: Some(Cow::Borrowed("application/json")),
                        metadata: None,
                    },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed, stopping node");
                break;
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_id, "input_closed");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{assert_state_running, assert_state_stopped, create_test_context};
    use std::collections::HashMap;
    use streamkit_core::types::PacketType;
    use tokio::sync::mpsc;

    #[test]
    fn new_default_config() {
        let node = JsonSerialize::new(None).unwrap();
        assert!(!node.pretty);
        assert!(!node.newline_delimited);
    }

    #[test]
    fn new_pretty_enabled() {
        let params = serde_json::json!({"pretty": true});
        let node = JsonSerialize::new(Some(&params)).unwrap();
        assert!(node.pretty);
        assert!(!node.newline_delimited);
    }

    #[test]
    fn new_newline_delimited_enabled() {
        let params = serde_json::json!({"newline_delimited": true});
        let node = JsonSerialize::new(Some(&params)).unwrap();
        assert!(!node.pretty);
        assert!(node.newline_delimited);
    }

    #[test]
    fn new_ignores_unknown_fields_and_uses_defaults() {
        let params = serde_json::json!({"unknown_field": 42});
        let node = JsonSerialize::new(Some(&params)).unwrap();
        assert!(!node.pretty);
        assert!(!node.newline_delimited);
    }

    #[test]
    fn input_pins_shape() {
        let pins = JsonSerialize::input_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "in");
        assert_eq!(pins[0].accepts_types, vec![PacketType::Any]);
        assert_eq!(pins[0].cardinality, PinCardinality::One);
    }

    #[test]
    fn output_pins_shape() {
        let pins = JsonSerialize::output_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "out");
        assert_eq!(pins[0].produces_type, PacketType::Binary);
        assert_eq!(pins[0].cardinality, PinCardinality::Broadcast);
    }

    #[tokio::test]
    async fn run_serializes_text_to_json_binary() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = JsonSerialize::new(None).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("hello".into())).await.unwrap();
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(packets.len(), 1);

        match &packets[0] {
            Packet::Binary { data, content_type, .. } => {
                assert_eq!(content_type.as_deref(), Some("application/json"));
                let parsed: serde_json::Value = serde_json::from_slice(data).unwrap();
                assert!(parsed.get("Text").is_some());
            },
            other => panic!("Expected Binary packet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_pretty_output_contains_indentation() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let params = serde_json::json!({"pretty": true});
        let node = JsonSerialize::new(Some(&params)).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("hi".into())).await.unwrap();
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        let data = match &packets[0] {
            Packet::Binary { data, .. } => data,
            other => panic!("Expected Binary, got {other:?}"),
        };
        let text = std::str::from_utf8(data).unwrap();
        assert!(text.contains('\n'), "pretty output should contain newlines");
        assert!(text.contains("  "), "pretty output should contain indentation");
    }

    #[tokio::test]
    async fn run_newline_delimited_appends_newline() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let params = serde_json::json!({"newline_delimited": true});
        let node = JsonSerialize::new(Some(&params)).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("x".into())).await.unwrap();
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        let data = match &packets[0] {
            Packet::Binary { data, .. } => data,
            other => panic!("Expected Binary, got {other:?}"),
        };
        assert_eq!(*data.last().unwrap(), b'\n');
    }

    #[tokio::test]
    async fn run_compact_output_has_no_trailing_newline() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = JsonSerialize::new(None).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("x".into())).await.unwrap();
        drop(input_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        let data = match &packets[0] {
            Packet::Binary { data, .. } => data,
            other => panic!("Expected Binary, got {other:?}"),
        };
        assert_ne!(*data.last().unwrap(), b'\n');
    }
}
