// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! File read node - Streams raw bytes from a file

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::io::AsyncReadExt;

/// Configuration for the FileReadNode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileReadConfig {
    /// Path to the file to read
    pub path: String,
    /// Size of chunks to read (default: 8192 bytes)
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
}

const fn default_chunk_size() -> usize {
    8192
}

/// A node that reads a file and outputs its contents as Binary packets.
///
/// This node is format-agnostic - it just streams raw bytes.
/// Demuxers and decoders downstream handle format parsing and timing extraction.
pub struct FileReadNode {
    config: FileReadConfig,
}

impl FileReadNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            // For dynamic nodes, allow None to create a default instance for pin inspection
            let config: FileReadConfig = if params.is_none() {
                // Default config for pin inspection only
                FileReadConfig { path: "/dev/null".to_string(), chunk_size: default_chunk_size() }
            } else {
                tracing::debug!("FileReadNode factory received params: {:?}", params);
                config_helpers::parse_config_required(params)?
            };
            tracing::debug!("FileReadNode created with path: {}", config.path);
            Ok(Box::new(Self { config }))
        })
    }
}

#[async_trait]
impl ProcessorNode for FileReadNode {
    fn input_pins(&self) -> Vec<InputPin> {
        // File input nodes have no input pins
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Open the file
        let mut file = tokio::fs::File::open(&self.config.path).await.map_err(|e| {
            StreamKitError::Runtime(format!("Failed to open file '{}': {}", self.config.path, e))
        })?;

        tracing::info!(
            "FileReadNode opened file: {} (chunk_size: {})",
            self.config.path,
            self.config.chunk_size
        );

        // Source nodes emit Ready state and wait for Start signal
        // This prevents packet loss during pipeline startup
        state_helpers::emit_ready(&context.state_tx, &node_name);
        tracing::info!("FileReadNode ready, waiting for start signal");

        // Wait for Start control message
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Start) => {
                    tracing::info!("FileReadNode received start signal");
                    break;
                },
                Some(streamkit_core::control::NodeControlMessage::UpdateParams(_)) => {
                    // Ignore param updates while waiting to start - loop continues naturally
                },
                Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                    tracing::info!("FileReadNode received shutdown before start");
                    return Ok(());
                },
                None => {
                    tracing::warn!("Control channel closed before start signal received");
                    return Ok(());
                },
            }
        }

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let mut chunk_count = 0u64;
        let mut total_bytes = 0u64;
        let mut buffer = vec![0u8; self.config.chunk_size];

        // Read file in chunks
        loop {
            // Check for cancellation before each read
            if let Some(token) = &context.cancellation_token {
                if token.is_cancelled() {
                    tracing::info!("FileRead cancelled after {} chunks", chunk_count);
                    break;
                }
            }

            // Use select! to check both file read AND control messages
            tokio::select! {
                read_result = file.read(&mut buffer) => {
                    match read_result {
                        Ok(0) => {
                            // EOF reached
                            tracing::info!(
                                "FileReadNode reached EOF after {} chunks ({} bytes)",
                                chunk_count,
                                total_bytes
                            );
                            break;
                        }
                        Ok(n) => {
                            chunk_count += 1;
                            total_bytes += n as u64;

                            // Send chunk as Binary packet (no metadata - demuxers will add timing)
                            let chunk = Bytes::copy_from_slice(&buffer[..n]);
                            if context
                                .output_sender
                                .send(
                                    "out",
                                    Packet::Binary {
                                        data: chunk,
                                        content_type: None,
                                        metadata: None,
                                    },
                                )
                                .await
                                .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                break;
                            }

                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                        }
                        Err(e) => {
                            state_helpers::emit_failed(
                                &context.state_tx,
                                &node_name,
                                format!("Read error: {e}"),
                            );
                            return Err(StreamKitError::Runtime(format!(
                                "Failed to read from file: {e}"
                            )));
                        }
                    }
                }
                Some(msg) = context.control_rx.recv() => {
                    match msg {
                        streamkit_core::control::NodeControlMessage::Shutdown => {
                            tracing::info!("FileReadNode received shutdown signal during read");
                            break;
                        }
                        streamkit_core::control::NodeControlMessage::UpdateParams(_)
                        | streamkit_core::control::NodeControlMessage::Start => {
                            // Ignore param updates and start during file read - loop continues naturally
                        }
                    }
                }
            }
        }

        stats_tracker.force_send();
        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use streamkit_core::node::RoutedPacketMessage;
    use streamkit_core::NodeStatsUpdate;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_file_read_node() {
        // Create a temporary test file
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.bin");
        let test_data = b"Hello, StreamKit! This is a test file.";
        tokio::fs::write(&file_path, test_data).await.unwrap();

        // Create test context
        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);
        let (control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_file_read".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs: HashMap::new(),
            control_rx,
            output_sender,
            batch_size: 32,
            state_tx,
            stats_tx: Some(stats_tx),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: None,
            pin_management_rx: None, // Test contexts don't support dynamic pins
            audio_pool: None,
        };

        // Create and run node
        let config = FileReadConfig {
            path: file_path.to_str().unwrap().to_string(),
            chunk_size: 10, // Small chunks for testing
        };
        let node = Box::new(FileReadNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for initializing state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Initializing));

        // Wait for ready state (source nodes wait for start signal)
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Ready));

        // Send start signal to begin reading
        control_tx.send(streamkit_core::control::NodeControlMessage::Start).await.unwrap();

        // Wait for running state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Running));

        // Collect all packets
        let mut collected_data = Vec::new();
        while let Some((_node, _pin, packet)) = packet_rx.recv().await {
            if let Packet::Binary { data, .. } = packet {
                collected_data.extend_from_slice(&data);
            }
        }

        // Wait for stopped state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Stopped { .. }));

        // Wait for node to complete
        node_handle.await.unwrap().unwrap();

        // Verify data matches
        assert_eq!(collected_data, test_data);
    }
}
