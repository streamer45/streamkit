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

/// An input node that reads a stream of byte chunks from a channel
/// and sends them out as `Packet::Binary` packets. This node is special-cased
/// by the stateless runner to represent the HTTP request body.
pub struct BytesInputNode {
    stream_rx: mpsc::Receiver<Bytes>,
    content_type: Option<String>,
}

impl BytesInputNode {
    /// Creates a new BytesInputNode directly with a channel receiver.
    /// This is a safe, compile-time checked way to provide the input stream.
    pub const fn new(stream_rx: mpsc::Receiver<Bytes>, content_type: Option<String>) -> Self {
        Self { stream_rx, content_type }
    }
}

#[async_trait]
impl ProcessorNode for BytesInputNode {
    fn input_pins(&self) -> Vec<InputPin> {
        // This is an input node, so it has no input pins.
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            // This node produces generic binary data, but we use Any
            // to allow flexible connections (e.g., Binary → Text conversion)
            produces_type: PacketType::Any,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("BytesInputNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut chunk_count = 0;
        let mut reason = "completed".to_string();

        // This node's main loop reads from the stream receiver provided at creation.
        // If a cancellation token is provided, we'll also listen for cancellation.
        if let Some(token) = &context.cancellation_token {
            loop {
                tokio::select! {
                    () = token.cancelled() => {
                        reason = "cancelled".to_string();
                        tracing::info!("BytesInputNode cancelled after {} chunks.", chunk_count);
                        break;
                    }
                    chunk = self.stream_rx.recv() => {
                        match chunk {
                            Some(chunk) => {
                                chunk_count += 1;
                                if context
                                    .output_sender
                                    .send(
                                        "out",
                                        Packet::Binary {
                                            data: chunk,
                                            content_type: self.content_type.clone().map(Cow::Owned),
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
                            None => {
                                // Stream finished normally
                                break;
                            }
                        }
                    }
                }
            }
        } else {
            // No cancellation token, use simpler loop
            while let Some(chunk) = self.stream_rx.recv().await {
                chunk_count += 1;
                if context
                    .output_sender
                    .send(
                        "out",
                        Packet::Binary {
                            data: chunk,
                            content_type: self.content_type.clone().map(Cow::Owned),
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
        }

        // The loop exits when the sender is dropped, which happens when the
        // upstream (e.g., the HTTP request body stream) has finished.
        state_helpers::emit_stopped(&context.state_tx, &node_name, reason);
        tracing::info!("BytesInputNode finished sending stream after {} chunks.", chunk_count);
        Ok(())
    }
}
