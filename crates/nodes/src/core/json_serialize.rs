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
