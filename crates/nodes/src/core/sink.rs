// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Sink node
//!
//! Accepts any packets and discards them.
//! Useful as a terminal node for side-branches (e.g. VAD events → telemetry tap → sink),
//! debugging, and graph wiring where a node requires an output connection.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use streamkit_core::types::PacketType;
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SinkConfig {}

#[derive(Debug, Default)]
pub struct SinkNode;

impl SinkNode {
    /// Creates a new sink node.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration parameters cannot be parsed.
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

pub fn register(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let schema = match serde_json::to_value(schema_for!(SinkConfig)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to serialize SinkConfig schema");
            return;
        },
    };

    registry.register_dynamic_with_description(
        "core::sink",
        |params| Ok(Box::new(SinkNode::new(params)?)),
        schema,
        vec!["core".to_string(), "observability".to_string()],
        false,
        "Accepts packets and discards them. Useful for terminating side-branches (e.g., telemetry taps) without affecting the main pipeline.",
    );
}
