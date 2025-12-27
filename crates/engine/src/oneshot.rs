// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Oneshot pipeline execution for batch processing.
//!
//! This module implements the "oneshot" execution mode where pipelines
//! run once from start to finish, then terminate. Ideal for:
//! - HTTP request processing
//! - File transcoding jobs
//! - Batch audio/video processing
//!
//! ## Stateless Architecture
//!
//! Oneshot pipelines use a stateless architecture: no persistent engine
//! actor, all state is local to the execution. This design minimizes
//! overhead for short-lived processing tasks.
//!
//! ## Current Limitation: Linear Pipelines Only
//!
//! The oneshot runner currently supports only linear graphs (no fan-out/branching).
//! If an output pin has multiple downstream connections, graph wiring fails fast with
//! a configuration error. Fan-out support can be added later by introducing an output
//! router (e.g., per-pin distributors similar to the dynamic engine).

use crate::constants::{
    DEFAULT_BATCH_SIZE, DEFAULT_ONESHOT_IO_CAPACITY, DEFAULT_ONESHOT_MEDIA_CAPACITY,
};
// Note: The constants are used in OneshotEngineConfig::default()
use crate::{graph_builder, Engine};
use bytes::Bytes;
use futures::Stream;
use std::collections::HashMap;
use streamkit_api::Pipeline;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;
use tokio::sync::mpsc;

/// Configuration for oneshot pipeline execution.
#[derive(Debug, Clone)]
pub struct OneshotEngineConfig {
    /// Batch size for packet processing (default: 32)
    pub packet_batch_size: usize,
    /// Buffer size for media channels between nodes (default: 256)
    pub media_channel_capacity: usize,
    /// Buffer size for I/O stream channels (default: 16)
    pub io_channel_capacity: usize,
}

impl Default for OneshotEngineConfig {
    fn default() -> Self {
        Self {
            packet_batch_size: DEFAULT_BATCH_SIZE,
            media_channel_capacity: DEFAULT_ONESHOT_MEDIA_CAPACITY,
            io_channel_capacity: DEFAULT_ONESHOT_IO_CAPACITY,
        }
    }
}

/// The result of a oneshot pipeline execution, containing the output stream and metadata.
pub struct OneshotPipelineResult {
    pub data_stream: mpsc::Receiver<Bytes>,
    pub content_type: String,
}

impl Engine {
    /// Runs a pipeline as a self-contained, one-shot task from a streaming input.
    ///
    /// Supports two modes:
    /// - HTTP streaming mode (`has_http_input=true`): Uses http_input node with media stream
    /// - File-based mode (`has_http_input=false`): Uses file_read nodes reading from disk
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Pipeline compilation fails
    /// - Nodes cannot be created or wired
    /// - The pipeline structure is invalid for oneshot execution
    ///
    /// # Panics
    ///
    /// Panics if the engine's registry lock is poisoned (only possible if a thread panicked
    /// while holding the lock).
    #[allow(clippy::cognitive_complexity)]
    pub async fn run_oneshot_pipeline<S, E>(
        &self,
        definition: Pipeline,
        mut input_stream: S,
        input_content_type: Option<String>,
        has_http_input: bool,
        config: Option<OneshotEngineConfig>,
    ) -> Result<OneshotPipelineResult, StreamKitError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let config = config.unwrap_or_default();

        tracing::info!(
            "Starting oneshot pipeline with {} nodes and {} connections",
            definition.nodes.len(),
            definition.connections.len()
        );

        // expect is documented in #[doc] Panics section above
        #[allow(clippy::expect_used)]
        let registry = {
            let guard = self
                .registry
                .read()
                .expect("Engine registry poisoned while preparing oneshot pipeline");
            guard.clone()
        };

        // --- 1. Find the special namespaced input and output nodes ---
        let mut input_node_id: Option<String> = None;
        let mut output_node_id: Option<String> = None;
        let mut source_node_ids: Vec<String> = Vec::new();

        for (name, def) in &definition.nodes {
            tracing::debug!("Found node '{}' of type '{}'", name, def.kind);
            if def.kind == "streamkit::http_input" {
                input_node_id = Some(name.clone());
            }
            if def.kind == "streamkit::http_output" {
                output_node_id = Some(name.clone());
            }
            if def.kind == "core::file_reader" {
                source_node_ids.push(name.clone());
            }
        }

        // Validate based on mode
        if has_http_input {
            // HTTP streaming mode: require http_input
            if input_node_id.is_none() {
                tracing::error!("Pipeline validation failed: missing streamkit::http_input node");
                return Err(StreamKitError::Configuration(
                    "Pipeline must contain one 'streamkit::http_input' node.".to_string(),
                ));
            }
            tracing::info!(
                "HTTP streaming mode: input='{}', output='{}'",
                // Safe unwrap: just validated input_node_id.is_some() above
                {
                    #[allow(clippy::unwrap_used)]
                    input_node_id.as_ref().unwrap()
                },
                output_node_id.as_ref().map_or("unknown", |s| s.as_str())
            );
        } else {
            // File-based mode: ensure we have source nodes
            if source_node_ids.is_empty() {
                tracing::error!("Pipeline validation failed: no file_reader nodes found");
                return Err(StreamKitError::Configuration(
                    "File-based pipelines must contain at least one 'core::file_reader' node."
                        .to_string(),
                ));
            }
            tracing::info!(
                "File-based mode: {} source node(s), output='{}'",
                source_node_ids.len(),
                output_node_id.as_ref().map_or("unknown", |s| s.as_str())
            );
        }

        let output_node_id = output_node_id.ok_or_else(|| {
            tracing::error!("Pipeline validation failed: missing streamkit::http_output node");
            StreamKitError::Configuration(
                "Pipeline must contain one 'streamkit::http_output' node.".to_string(),
            )
        })?;

        // --- 2. Create channels for the I/O streams and cancellation token ---
        let (input_stream_tx, input_stream_rx) = mpsc::channel(config.io_channel_capacity);
        let (output_stream_tx, output_stream_rx) = mpsc::channel(config.io_channel_capacity);
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        tracing::debug!("Created I/O stream channels and cancellation token");

        // --- 3. Validate that http_output is connected ---
        let final_node_id = definition
            .connections
            .iter()
            .find(|c| c.to_node == output_node_id)
            .map(|c| &c.from_node)
            .ok_or_else(|| {
                tracing::error!(
                    "Pipeline validation failed: streamkit::http_output node '{}' is not connected",
                    output_node_id
                );
                StreamKitError::Configuration(
                    "streamkit::http_output node is not connected".to_string(),
                )
            })?;

        tracing::debug!("Final output node identified: '{}'", final_node_id);

        // Get final node definition - this should exist since it's referenced in a connection
        let final_node_def = definition.nodes.get(final_node_id).ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "Final node '{final_node_id}' referenced in connection but not found in pipeline definition"
            ))
        })?;
        tracing::debug!("Creating final node instance of type '{}'", final_node_def.kind);

        // Get the static content-type from the final node before we move it
        let static_content_type = {
            let temp_instance =
                registry.create_node(&final_node_def.kind, final_node_def.params.as_ref())?;
            temp_instance.content_type()
        };

        // --- 4. Instantiate all nodes for the pipeline ---
        let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();

        // Manually create the special input node (only in HTTP streaming mode)
        if has_http_input {
            // Safe unwrap: validated input_node_id.is_some() when has_http_input is true
            #[allow(clippy::unwrap_used)]
            let input_id = input_node_id.as_ref().unwrap();
            tracing::debug!("Creating special input node '{}'", input_id);
            let input_node = Box::new(streamkit_nodes::core::bytes_input::BytesInputNode::new(
                input_stream_rx,
                input_content_type.clone(),
            ));
            nodes.insert(input_id.clone(), input_node);
        }

        tracing::debug!("Creating special output node '{}'", output_node_id);
        // Get output node definition - this should exist since output_node_id was found in pipeline
        let output_node_def = definition.nodes.get(&output_node_id).ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "Output node '{output_node_id}' not found in pipeline definition"
            ))
        })?;
        let output_node = streamkit_nodes::core::bytes_output::BytesOutputNode::new_with_config(
            output_stream_tx,
            output_node_def.params.as_ref(),
        )?;
        // Capture the configured content type before moving the node
        let configured_content_type = output_node.configured_content_type();
        nodes.insert(output_node_id.clone(), Box::new(output_node));

        // Create the final node for insertion into the pipeline
        tracing::debug!("Adding final node '{}' to pipeline", final_node_id);
        let final_node_instance =
            registry.create_node(&final_node_def.kind, final_node_def.params.as_ref())?;
        nodes.insert(final_node_id.clone(), final_node_instance);

        // Create all other standard processing nodes from the main registry.
        for (name, def) in &definition.nodes {
            if !nodes.contains_key(name) {
                tracing::debug!("Creating node '{}' of type '{}'", name, def.kind);
                let node = registry.create_node(&def.kind, def.params.as_ref()).map_err(|e| {
                    tracing::error!(
                        "Failed to create node '{}' of type '{}': {}",
                        name,
                        def.kind,
                        e
                    );
                    e
                })?;
                nodes.insert(name.clone(), node);
                tracing::debug!("Successfully created node '{}'", name);
            }
        }

        tracing::info!("Created {} nodes total", nodes.len());

        // --- 5. Use the shared helper to wire up and spawn the graph ---
        tracing::info!("Wiring up and spawning pipeline graph");

        let node_kinds: HashMap<String, String> =
            definition.nodes.iter().map(|(name, def)| (name.clone(), def.kind.clone())).collect();

        // Per-pipeline audio buffer pool for hot paths (e.g., Opus decode).
        let audio_pool = std::sync::Arc::new(streamkit_core::FramePool::<f32>::audio_default());

        // Oneshot pipelines don't track state, so pass None for state_tx
        let live_nodes = graph_builder::wire_and_spawn_graph(
            nodes,
            &definition.connections,
            &node_kinds,
            config.packet_batch_size,
            config.media_channel_capacity,
            None, // No state tracking for oneshot pipelines
            Some(cancellation_token.clone()),
            Some(audio_pool),
        )
        .await?;
        tracing::info!("Pipeline graph successfully spawned");

        // --- 5.5. Send Start signals to file_reader nodes ---
        // Note: file_reader nodes need Start signals even in HTTP streaming mode
        // (e.g., for mixing scenarios where you have both http_input and file_reader)
        if !source_node_ids.is_empty() {
            tracing::info!(
                "Sending Start signals to {} file_reader node(s)",
                source_node_ids.len()
            );
            for source_id in &source_node_ids {
                if let Some(node_handle) = live_nodes.get(source_id) {
                    tracing::debug!("Sending Start signal to source node '{}'", source_id);
                    if let Err(e) = node_handle.control_tx.send(NodeControlMessage::Start).await {
                        tracing::error!(
                            "Failed to send Start signal to node '{}': {}",
                            source_id,
                            e
                        );
                    }
                } else {
                    tracing::warn!("Source node '{}' not found in live nodes", source_id);
                }
            }
        }

        // --- 6. Spawn a task to pump the input stream into the graph (HTTP streaming mode only) ---
        if has_http_input {
            tracing::debug!("Starting input stream pump task");
            let input_pump_token = cancellation_token.clone();
            tokio::spawn(async move {
                use futures::StreamExt;
                let mut chunk_count = 0;
                tracing::debug!("Input stream pump starting to read from stream");
                loop {
                    tokio::select! {
                        // Use () instead of _ for unit type to be explicit
                        () = input_pump_token.cancelled() => {
                            tracing::info!("Input stream pump cancelled after {} chunks", chunk_count);
                            break;
                        }
                        chunk_result = input_stream.next() => {
                            match chunk_result {
                                Some(Ok(chunk)) => {
                                    chunk_count += 1;
                                    if input_stream_tx.send(chunk).await.is_err() {
                                        tracing::warn!("Input node closed before stream ended.");
                                        break;
                                    }
                                }
                                Some(Err(e)) => {
                                    tracing::error!("Error reading from input stream: {}", e);
                                    break;
                                }
                                None => {
                                    tracing::info!("Input stream pump finished after {} chunks", chunk_count);
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }

        // --- 7. Determine content-type for the response ---
        tracing::debug!(
            "Content type sources - configured: {:?}, static: {:?}, input: {:?}",
            configured_content_type,
            static_content_type,
            input_content_type
        );

        // Priority: configured (from http_output params) > static (final node) > input > default
        let content_type = configured_content_type
            .or(static_content_type)
            .or(input_content_type)
            .unwrap_or_else(|| "application/octet-stream".to_string());

        tracing::info!("Using content type for response: '{}'", content_type);

        // --- 8. Return the result struct ---
        Ok(OneshotPipelineResult { data_stream: output_stream_rx, content_type })
    }
}
