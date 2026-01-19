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
    streams: Vec<BytesInputStream>,
}

struct BytesInputStream {
    pin: String,
    stream_rx: mpsc::Receiver<Bytes>,
    content_type: Option<String>,
}

impl BytesInputNode {
    /// Creates a new BytesInputNode directly with a channel receiver.
    /// This is a safe, compile-time checked way to provide the input stream.
    pub fn new(
        pin: impl Into<String>,
        stream_rx: mpsc::Receiver<Bytes>,
        content_type: Option<String>,
    ) -> Self {
        Self { streams: vec![BytesInputStream { pin: pin.into(), stream_rx, content_type }] }
    }

    /// Creates a BytesInputNode with multiple output pins/streams.
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
        // This is an input node, so it has no input pins.
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        self.streams
            .iter()
            .map(|stream| OutputPin {
                name: stream.pin.clone(),
                // This node produces generic binary data, but we use Any
                // to allow flexible connections (e.g., Binary → Text conversion)
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
