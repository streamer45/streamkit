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

#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BytesOutputConfig {
    /// Optional content type to set for the HTTP response
    /// If not specified, will be auto-detected from Binary packet or fall back to input type
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Special-cased by the stateless runner to represent the HTTP response body.
pub struct BytesOutputNode {
    result_tx: mpsc::Sender<Bytes>,
    configured_content_type: Option<String>,
}

impl BytesOutputNode {
    pub const fn new(result_tx: mpsc::Sender<Bytes>) -> Self {
        Self { result_tx, configured_content_type: None }
    }

    /// # Errors
    /// Returns `Err` if config parsing fails.
    pub fn new_with_config(
        result_tx: mpsc::Sender<Bytes>,
        params: Option<&serde_json::Value>,
    ) -> Result<Self, StreamKitError> {
        let config: BytesOutputConfig = config_helpers::parse_config_optional(params)?;
        Ok(Self { result_tx, configured_content_type: config.content_type })
    }

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
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("BytesOutputNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut input_rx = context.take_input("in")?;
        let mut packet_count = 0;

        let mut reason = "input_closed".to_string();

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            if let Packet::Binary { data, .. } = packet {
                packet_count += 1;

                if packet_count % 500 == 0 {
                    tracing::debug!("BytesOutputNode: sent {} packets so far", packet_count);
                }

                if self.result_tx.send(data).await.is_err() {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions use unwrap/expect to fail loudly.
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use std::borrow::Cow;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[test]
    fn new_returns_no_configured_content_type() {
        let (tx, _rx) = mpsc::channel(10);
        let node = BytesOutputNode::new(tx);
        assert!(node.configured_content_type().is_none());
    }

    #[test]
    fn new_with_config_parses_content_type() {
        let (tx, _rx) = mpsc::channel(10);
        let params = serde_json::json!({"content_type": "audio/wav"});
        let node = BytesOutputNode::new_with_config(tx, Some(&params)).unwrap();
        assert_eq!(node.configured_content_type().as_deref(), Some("audio/wav"));
    }

    #[test]
    fn new_with_config_default_has_no_content_type() {
        let (tx, _rx) = mpsc::channel(10);
        let node = BytesOutputNode::new_with_config(tx, None).unwrap();
        assert!(node.configured_content_type().is_none());
    }

    #[test]
    fn new_with_config_ignores_unknown_fields() {
        let (tx, _rx) = mpsc::channel(10);
        let params = serde_json::json!({"unknown": 42});
        let node = BytesOutputNode::new_with_config(tx, Some(&params)).unwrap();
        assert!(node.configured_content_type().is_none());
    }

    #[test]
    fn input_pins_shape() {
        let (tx, _rx) = mpsc::channel(10);
        let node = BytesOutputNode::new(tx);
        let pins = node.input_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "in");
        assert_eq!(pins[0].accepts_types, vec![PacketType::Binary]);
        assert_eq!(pins[0].cardinality, PinCardinality::One);
    }

    #[test]
    fn output_pins_empty() {
        let (tx, _rx) = mpsc::channel(10);
        let node = BytesOutputNode::new(tx);
        assert!(node.output_pins().is_empty());
    }

    #[tokio::test]
    async fn run_forwards_binary_data_to_result_tx() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let node = BytesOutputNode::new(result_tx);

        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (context, _mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let data1 = Bytes::from_static(b"hello");
        let data2 = Bytes::from_static(b"world");
        input_tx
            .send(Packet::Binary {
                data: data1.clone(),
                content_type: Some(Cow::Borrowed("text/plain")),
                metadata: None,
            })
            .await
            .unwrap();
        input_tx
            .send(Packet::Binary { data: data2.clone(), content_type: None, metadata: None })
            .await
            .unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let r1 = result_rx.recv().await.unwrap();
        let r2 = result_rx.recv().await.unwrap();
        assert_eq!(r1, data1);
        assert_eq!(r2, data2);
    }

    #[tokio::test]
    async fn run_receiver_closed_triggers_cancellation() {
        let (result_tx, result_rx) = mpsc::channel(1);
        let node = BytesOutputNode::new(result_tx);

        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);
        let (mut context, _mock_sender, mut state_rx) = create_test_context(inputs, 10);
        let token = tokio_util::sync::CancellationToken::new();
        context.cancellation_token = Some(token.clone());

        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        drop(result_rx);

        input_tx
            .send(Packet::Binary {
                data: Bytes::from_static(b"data"),
                content_type: None,
                metadata: None,
            })
            .await
            .unwrap();

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        assert!(token.is_cancelled());
    }
}
