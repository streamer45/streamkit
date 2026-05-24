// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use std::borrow::Cow;

use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

/// Special-cased by the stateless runner to represent the HTTP request body.
pub struct BytesInputNode {
    streams: Vec<BytesInputStream>,
}

struct BytesInputStream {
    pin: String,
    stream_rx: mpsc::Receiver<Bytes>,
    content_type: Option<String>,
}

impl BytesInputNode {
    pub fn new(
        pin: impl Into<String>,
        stream_rx: mpsc::Receiver<Bytes>,
        content_type: Option<String>,
    ) -> Self {
        Self { streams: vec![BytesInputStream { pin: pin.into(), stream_rx, content_type }] }
    }

    pub fn with_streams(streams: Vec<(String, mpsc::Receiver<Bytes>, Option<String>)>) -> Self {
        let streams = streams
            .into_iter()
            .map(|(pin, stream_rx, content_type)| BytesInputStream { pin, stream_rx, content_type })
            .collect();
        Self { streams }
    }
}

#[async_trait]
impl ProcessorNode for BytesInputNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        self.streams
            .iter()
            .map(|stream| OutputPin {
                name: stream.pin.clone(),
                produces_type: PacketType::Any,
                cardinality: PinCardinality::Broadcast,
            })
            .collect()
    }

    async fn run(mut self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("BytesInputNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut handles = Vec::new();

        for mut stream in self.streams {
            let mut sender = context.output_sender.clone();
            let state_tx = context.state_tx.clone();
            let node = node_name.clone();
            let cancel = context.cancellation_token.clone();
            handles.push(tokio::spawn(async move {
                let mut chunk_count = 0usize;
                let mut reason = "completed".to_string();
                loop {
                    tokio::select! {
                        () = async {
                            if let Some(token) = &cancel {
                                token.cancelled().await;
                            }
                        } => {
                            reason = "cancelled".to_string();
                            tracing::info!("BytesInputNode '{}' stream '{}' cancelled after {} chunks.", node, stream.pin, chunk_count);
                            break;
                        }
                        chunk = stream.stream_rx.recv() => {
                            match chunk {
                                Some(chunk) => {
                                    chunk_count += 1;
                                    if sender
                                        .send(
                                            &stream.pin,
                                            Packet::Binary {
                                                data: chunk,
                                                content_type: stream.content_type.clone().map(Cow::Owned),
                                                metadata: None,
                                            },
                                        )
                                        .await
                                        .is_err()
                                    {
                                        tracing::debug!("Output channel for pin '{}' closed, stopping stream", stream.pin);
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                }
                state_helpers::emit_stopped(&state_tx, &node, reason);
                tracing::info!("BytesInputNode '{}' stream '{}' finished after {} chunks.", node, stream.pin, chunk_count);
            }));
        }

        for handle in handles {
            let _ = handle.await;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use std::collections::HashMap;
    use streamkit_core::ProcessorNode;
    use tokio::sync::mpsc;

    #[test]
    fn new_single_stream_output_pins() {
        let (_tx, rx) = mpsc::channel(10);
        let node = BytesInputNode::new("body", rx, Some("audio/wav".to_string()));
        let pins = node.output_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "body");
        assert_eq!(pins[0].produces_type, PacketType::Any);
        assert_eq!(pins[0].cardinality, PinCardinality::Broadcast);
    }

    #[test]
    fn new_has_no_input_pins() {
        let (_tx, rx) = mpsc::channel(10);
        let node = BytesInputNode::new("out", rx, None);
        assert!(node.input_pins().is_empty());
    }

    #[test]
    fn with_streams_multi_output_pins() {
        let (_tx1, rx1) = mpsc::channel(10);
        let (_tx2, rx2) = mpsc::channel(10);
        let node = BytesInputNode::with_streams(vec![
            ("audio".to_string(), rx1, Some("audio/wav".to_string())),
            ("video".to_string(), rx2, None),
        ]);
        let pins = node.output_pins();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "audio");
        assert_eq!(pins[1].name, "video");
    }

    #[tokio::test]
    async fn run_sends_binary_packets_with_content_type() {
        let (stream_tx, stream_rx) = mpsc::channel(10);
        let node = BytesInputNode::new("out", stream_rx, Some("audio/wav".to_string()));

        let (mut context, mock_sender, mut state_rx) = create_test_context(HashMap::new(), 10);
        context.cancellation_token = Some(tokio_util::sync::CancellationToken::new());
        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        stream_tx.send(Bytes::from_static(b"chunk1")).await.unwrap();
        stream_tx.send(Bytes::from_static(b"chunk2")).await.unwrap();
        drop(stream_tx);

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        let packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(packets.len(), 2);

        for (i, expected) in [b"chunk1".as_slice(), b"chunk2"].iter().enumerate() {
            match &packets[i] {
                Packet::Binary { data, content_type, .. } => {
                    assert_eq!(data.as_ref(), *expected);
                    assert_eq!(content_type.as_deref(), Some("audio/wav"));
                },
                other => panic!("Expected Binary, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn run_cancellation_stops_node() {
        let (stream_tx, stream_rx) = mpsc::channel(10);
        let node = BytesInputNode::new("out", stream_rx, None);

        let (mut context, _mock_sender, mut state_rx) = create_test_context(HashMap::new(), 10);
        let token = tokio_util::sync::CancellationToken::new();
        context.cancellation_token = Some(token.clone());

        let handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        token.cancel();

        assert_state_stopped(&mut state_rx).await;
        handle.await.unwrap().unwrap();

        drop(stream_tx);
    }
}
