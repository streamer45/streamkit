// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use streamkit_core::types::PacketType;
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {}

#[derive(Debug, Default)]
pub struct SinkNode;

impl SinkNode {
    /// # Errors
    /// Returns `Err` if config parsing fails.
    pub fn new(params: Option<&serde_json::Value>) -> Result<Self, StreamKitError> {
        let _config: SinkConfig = config_helpers::parse_config_optional(params)?;
        Ok(Self)
    }

    pub fn input_pins() -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }
}

#[async_trait]
impl ProcessorNode for SinkNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Self::input_pins()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_id = context.output_sender.node_name().to_string();
        state_helpers::emit_running(&context.state_tx, &node_id);

        let mut input_rx = context.take_input("in")?;
        while context.recv_with_cancellation(&mut input_rx).await.is_some() {}

        state_helpers::emit_stopped(&context.state_tx, &node_id, "input_closed");
        Ok(())
    }
}

#[allow(clippy::missing_panics_doc)] // Panics only if JsonSchema-derived config fails to serialize (infallible)
pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    #[allow(clippy::expect_used)] // JsonSchema-derived configs are infallible to serialize
    registry.register_dynamic_with_description(
        "core::sink",
        |params| Ok(Box::new(SinkNode::new(params)?)),
        serde_json::to_value(schemars::schema_for!(SinkConfig))
            .expect("SinkConfig schema should serialize to JSON"),
        vec!["core".to_string(), "observability".to_string()],
        false,
        "Accepts packets and discards them. Useful for terminating side-branches \
         (e.g., telemetry taps) without affecting the main pipeline.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_running, assert_state_stopped, create_test_binary_packet, create_test_context,
    };
    use std::collections::HashMap;
    use streamkit_core::types::Packet;
    use tokio::sync::mpsc;

    #[test]
    fn new_default_config() {
        let node = SinkNode::new(None).unwrap();
        assert_eq!(format!("{node:?}"), "SinkNode");
    }

    #[test]
    fn new_ignores_unknown_fields() {
        let node = SinkNode::new(Some(&serde_json::json!({"unknown": true}))).unwrap();
        assert_eq!(format!("{node:?}"), "SinkNode");
    }

    #[test]
    fn input_pins_shape() {
        let pins = SinkNode::input_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "in");
        assert_eq!(pins[0].accepts_types, vec![PacketType::Any]);
        assert_eq!(pins[0].cardinality, PinCardinality::One);
    }

    #[test]
    fn output_pins_empty() {
        let node = SinkNode::new(None).unwrap();
        assert!(node.output_pins().is_empty());
    }

    #[tokio::test]
    async fn run_consumes_packets_and_stops() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, _mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = SinkNode::new(None).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("a".into())).await.unwrap();
        input_tx.send(create_test_binary_packet(vec![1, 2])).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn run_stops_immediately_on_closed_input() {
        let (input_tx, input_rx) = mpsc::channel::<Packet>(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, _mock_sender, mut state_rx) = create_test_context(inputs, 10);

        drop(input_tx);

        let node = SinkNode::new(None).unwrap();
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_running(&mut state_rx).await;
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();
    }
}
