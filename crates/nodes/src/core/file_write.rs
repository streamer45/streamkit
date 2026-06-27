// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileWriteConfig {
    /// Path to the file to write, relative to `[server].asset_root`.
    /// Absolute paths and `..` components are rejected.
    pub path: String,
    /// Size of buffer before writing to disk (default: 8192 bytes)
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
}

const fn default_chunk_size() -> usize {
    8192
}

pub struct FileWriteNode {
    config: FileWriteConfig,
}

impl FileWriteNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: FileWriteConfig = if params.is_none() {
                FileWriteConfig { path: "/dev/null".to_string(), chunk_size: default_chunk_size() }
            } else {
                config_helpers::parse_config_required(params)?
            };
            Ok(Box::new(Self { config }))
        })
    }
}

#[async_trait]
impl ProcessorNode for FileWriteNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let resolved_path = streamkit_core::path_helpers::resolve_new_asset_path(
            &self.config.path,
            &context.asset_root,
        )
        .map_err(StreamKitError::Runtime)?;
        let mut file = tokio::fs::File::create(&resolved_path).await.map_err(|e| {
            StreamKitError::Runtime(format!(
                "Failed to create file '{}': {e}",
                resolved_path.display()
            ))
        })?;

        tracing::info!(
            "FileWriteNode opened file for writing: {} (chunk_size: {})",
            self.config.path,
            self.config.chunk_size
        );

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let mut packet_count = 0u64;
        let mut total_bytes = 0u64;
        let mut reason = "input_closed".to_string();
        let mut buffer = Vec::with_capacity(self.config.chunk_size);
        let mut chunks_written = 0u64;

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            if let Packet::Binary { data, .. } = packet {
                stats_tracker.received();
                packet_count += 1;
                total_bytes += data.len() as u64;

                buffer.extend_from_slice(&data);

                if buffer.len() >= self.config.chunk_size {
                    if let Err(e) = file.write_all(&buffer).await {
                        stats_tracker.errored();
                        stats_tracker.force_send();
                        state_helpers::emit_failed(
                            &context.state_tx,
                            &node_name,
                            format!("Write error: {e}"),
                        );
                        return Err(StreamKitError::Runtime(format!(
                            "Failed to write to file: {e}"
                        )));
                    }
                    chunks_written += 1;
                    buffer.clear();
                }

                stats_tracker.sent();
                stats_tracker.maybe_send();
            } else {
                tracing::warn!("FileWriteNode received non-Binary packet, ignoring");
                stats_tracker.discarded();
            }
        }

        if !buffer.is_empty() {
            if let Err(e) = file.write_all(&buffer).await {
                stats_tracker.errored();
                stats_tracker.force_send();
                state_helpers::emit_failed(
                    &context.state_tx,
                    &node_name,
                    format!("Write error: {e}"),
                );
                return Err(StreamKitError::Runtime(format!("Failed to write to file: {e}")));
            }
            chunks_written += 1;
        }

        // Flush and close the file
        if let Err(e) = file.flush().await {
            tracing::error!("Failed to flush file: {}", e);
            stats_tracker.errored();
            reason = format!("flush_failed: {e}");
        }

        stats_tracker.force_send();
        tracing::info!(
            "FileWriteNode finished writing {} packets ({} bytes, {} chunks) to {}",
            packet_count,
            total_bytes,
            chunks_written,
            self.config.path
        );

        state_helpers::emit_stopped(&context.state_tx, &node_name, reason);
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
    async fn test_file_write_node() {
        // Create a temporary output file path
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("output.bin");

        // Create test context
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let (mock_sender, _packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_file_write".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs,
            input_types: HashMap::new(),
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
            video_pool: None,
            pipeline_mode: streamkit_core::PipelineMode::Dynamic,
            view_data_tx: None,
            engine_control_tx: None,
            asset_root: temp_dir.path().to_path_buf(),
        };

        // Create and run node — use relative path so it resolves under asset_root
        let config =
            FileWriteConfig { path: "output.bin".to_string(), chunk_size: default_chunk_size() };
        let node = Box::new(FileWriteNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for initializing state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Initializing));

        // Wait for running state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Running));

        // Send test data in chunks
        let test_data = b"Hello, StreamKit! This is a test file.";
        for chunk in test_data.chunks(10) {
            input_tx
                .send(Packet::Binary {
                    data: bytes::Bytes::copy_from_slice(chunk),
                    content_type: None,
                    metadata: None,
                })
                .await
                .unwrap();
        }

        // Close input
        drop(input_tx);

        // Wait for stopped state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Stopped { .. }));

        // Wait for node to complete
        node_handle.await.unwrap().unwrap();

        // Verify file contents
        let written_data = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(written_data, test_data);
    }

    #[tokio::test]
    async fn test_file_write_node_with_chunking() {
        // Create a temporary output file path
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("chunked_output.bin");

        // Create test context
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let (mock_sender, _packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_file_write_chunked".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs,
            input_types: HashMap::new(),
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
            video_pool: None,
            pipeline_mode: streamkit_core::PipelineMode::Dynamic,
            view_data_tx: None,
            engine_control_tx: None,
            asset_root: temp_dir.path().to_path_buf(),
        };

        // Create and run node with small chunk size for testing
        let config = FileWriteConfig {
            path: "chunked_output.bin".to_string(),
            chunk_size: 20, // Small chunks for testing
        };
        let node = Box::new(FileWriteNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for initializing state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Initializing));

        // Wait for running state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Running));

        // Send test data in small packets (each packet is 5 bytes)
        // With chunk_size=20, we expect buffering to happen
        let test_data = b"HelloWorldTestDataStreamKit!!!!!";
        for chunk in test_data.chunks(5) {
            input_tx
                .send(Packet::Binary {
                    data: bytes::Bytes::copy_from_slice(chunk),
                    content_type: None,
                    metadata: None,
                })
                .await
                .unwrap();
        }

        // Close input
        drop(input_tx);

        // Wait for stopped state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Stopped { .. }));

        // Wait for node to complete
        node_handle.await.unwrap().unwrap();

        // Verify file contents match original data
        let written_data = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(written_data, test_data);
    }
}
