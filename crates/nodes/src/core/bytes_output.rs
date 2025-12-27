// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, PinCardinality, ProcessorNode,
    StreamKitError,
};
use tokio::sync::mpsc;

/// Configuration for BytesOutputNode
#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema)]
pub struct BytesOutputConfig {
    /// Optional content type to set for the HTTP response
    /// If not specified, will be auto-detected from Binary packet or fall back to input type
    #[serde(default)]
    pub content_type: Option<String>,
}

/// An output node that receives binary packets and streams their
/// contents back to the stateless runner. This node is special-cased
/// to represent the HTTP response body.
pub struct BytesOutputNode {
    result_tx: mpsc::Sender<Bytes>,
    /// Configured content type (from params)
    configured_content_type: Option<String>,
}

impl BytesOutputNode {
    pub const fn new(result_tx: mpsc::Sender<Bytes>) -> Self {
        Self { result_tx, configured_content_type: None }
    }

    /// Creates a new `BytesOutputNode` with configuration from YAML parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration parameters cannot be parsed.
    pub fn new_with_config(
        result_tx: mpsc::Sender<Bytes>,
        params: Option<&serde_json::Value>,
    ) -> Result<Self, StreamKitError> {
        let config: BytesOutputConfig = config_helpers::parse_config_optional(params)?;
        Ok(Self { result_tx, configured_content_type: config.content_type })
    }

    /// Get the configured content type from parameters
    pub fn configured_content_type(&self) -> Option<String> {
        self.configured_content_type.clone()
    }
}

#[async_trait]
impl ProcessorNode for BytesOutputNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![] // This is an output node.
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("BytesOutputNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut input_rx = context.take_input("in")?;
        let mut packet_count = 0;

        let mut reason = "input_closed".to_string();

        // Loop and forward every binary packet's data to the result stream.
        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            if let Packet::Binary { data, .. } = packet {
                packet_count += 1;

                // Log every 500th packet to track progress (debug-only to avoid hot-path overhead)
                if packet_count % 500 == 0 {
                    tracing::debug!("BytesOutputNode: sent {} packets so far", packet_count);
                }

                if self.result_tx.send(data).await.is_err() {
                    // The receiver was dropped, so the HTTP response has likely closed.
                    // Trigger cancellation to stop all upstream processing immediately.
                    if let Some(token) = &context.cancellation_token {
                        tracing::warn!(
                            "BytesOutputNode receiver closed. Triggering cancellation after {} packets.",
                            packet_count
                        );
                        token.cancel();
                    } else {
                        tracing::warn!(
                            "BytesOutputNode receiver closed. Shutting down after {} packets.",
                            packet_count
                        );
                    }
                    reason = "output_closed".to_string();
                    break;
                }
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, reason);
        tracing::info!("BytesOutputNode finished after {} packets.", packet_count);
        Ok(())
    }
}
