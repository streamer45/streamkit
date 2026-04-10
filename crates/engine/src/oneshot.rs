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
    DEFAULT_STATE_CHANNEL_CAPACITY,
};
// Note: The constants are used in OneshotEngineConfig::default()
use crate::{graph_builder, Engine};
use bytes::Bytes;
use futures::Stream;
use opentelemetry::{global, KeyValue};
use std::collections::HashMap;
use streamkit_api::Pipeline;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;

/// The detected input mode for a oneshot pipeline.
enum OneshotInputMode {
    /// HTTP streaming: pipeline has `streamkit::http_input` nodes.
    HttpStreaming,
    /// File-based: pipeline has `core::file_reader` nodes (no http_input).
    FileBased,
    /// Generator: pipeline produces its own data (e.g. `video::colorbars`).
    Generator,
}

/// Validates the input mode of a oneshot pipeline.
///
/// Checks that the combination of pipeline nodes and provided input streams
/// is consistent.
fn validate_input_mode<S>(
    has_http_input: bool,
    source_node_ids: &[String],
    http_input_nodes: &[String],
    inputs: &[OneshotInput<S>],
    output_node_id: Option<&String>,
) -> Result<OneshotInputMode, StreamKitError> {
    let output_label = output_node_id.map_or("unknown", String::as_str);

    if has_http_input {
        if inputs.is_empty() {
            tracing::error!(
                "Pipeline validation failed: no input streams provided for http_input nodes"
            );
            return Err(StreamKitError::Configuration(
                "Input streams are required for 'streamkit::http_input' nodes.".to_string(),
            ));
        }
        tracing::info!(
            "HTTP streaming mode: {} http_input node(s), output='{output_label}'",
            http_input_nodes.len(),
        );
        Ok(OneshotInputMode::HttpStreaming)
    } else if !source_node_ids.is_empty() {
        if !inputs.is_empty() {
            tracing::error!(
                "Pipeline validation failed: streams provided but no http_input nodes present"
            );
            return Err(StreamKitError::Configuration(
                "Multipart streams were provided but the pipeline has no 'streamkit::http_input' nodes."
                    .to_string(),
            ));
        }
        tracing::info!(
            "File-based mode: {} source node(s), output='{output_label}'",
            source_node_ids.len(),
        );
        Ok(OneshotInputMode::FileBased)
    } else {
        // Generator mode: pipeline produces its own data (e.g. video::colorbars)
        // No http_input or file_reader required — just needs http_output.
        if !inputs.is_empty() {
            tracing::error!(
                "Pipeline validation failed: streams provided but no http_input nodes present"
            );
            return Err(StreamKitError::Configuration(
                "Multipart streams were provided but the pipeline has no 'streamkit::http_input' nodes."
                    .to_string(),
            ));
        }
        tracing::info!("Generator mode: no input nodes, output='{output_label}'");
        Ok(OneshotInputMode::Generator)
    }
}
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

/// Binding between a multipart field and an `streamkit::http_input` node.
pub struct OneshotInput<S> {
    /// Node id of the `streamkit::http_input` instance to feed.
    pub node_id: String,
    /// Output pin name to send this stream on (typically matches the multipart field).
    pub output_pin: String,
    /// Incoming byte stream for this node.
    pub stream: S,
    /// Optional request content type associated with this stream.
    pub content_type: Option<String>,
    /// Multipart field name (for logging/debugging).
    pub field_name: String,
    /// Whether the pipeline marked this input as required.
    pub required: bool,
    /// Cancellation token to stop reading if the pipeline is cancelled.
    pub cancellation_token: Option<CancellationToken>,
}

impl Engine {
    /// Runs a pipeline as a self-contained, one-shot task from a streaming input.
    ///
    /// Supports two modes:
    /// - HTTP streaming mode (`inputs` non-empty): Uses http_input nodes with media streams
    /// - File-based mode (`inputs` empty): Uses file_read nodes reading from disk
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
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    pub async fn run_oneshot_pipeline<S, E>(
        &self,
        definition: Pipeline,
        inputs: Vec<OneshotInput<S>>,
        config: Option<OneshotEngineConfig>,
        cancellation_token: Option<CancellationToken>,
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

        #[allow(clippy::expect_used)]
        let registry = {
            let guard = self
                .registry
                .read()
                .expect("Engine registry poisoned while preparing oneshot pipeline");
            guard.clone()
        };

        // --- 1. Identify key nodes ---
        let mut output_node_id: Option<String> = None;
        let mut source_node_ids: Vec<String> = Vec::new();
        let mut http_input_nodes: Vec<String> = Vec::new();

        for (name, def) in &definition.nodes {
            tracing::debug!("Found node '{}' of type '{}'", name, def.kind);
            if def.kind == "streamkit::http_input" {
                http_input_nodes.push(name.clone());
            }
            if def.kind == "streamkit::http_output" {
                output_node_id = Some(name.clone());
            }
            if def.kind == "core::file_reader" {
                source_node_ids.push(name.clone());
            }
        }

        let has_http_input = !http_input_nodes.is_empty();

        let _input_mode = validate_input_mode(
            has_http_input,
            &source_node_ids,
            &http_input_nodes,
            &inputs,
            output_node_id.as_ref(),
        )?;

        let output_node_id = output_node_id.ok_or_else(|| {
            tracing::error!("Pipeline validation failed: missing streamkit::http_output node");
            StreamKitError::Configuration(
                "Pipeline must contain one 'streamkit::http_output' node.".to_string(),
            )
        })?;

        // --- 2. I/O channels and cancellation token ---
        let (output_stream_tx, output_stream_rx) = mpsc::channel(config.io_channel_capacity);
        let cancellation_token = cancellation_token.unwrap_or_default();
        tracing::debug!("Created I/O stream channels and cancellation token");

        // --- 2.5. Bind http_input streams ---
        let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
        let mut provided_inputs: HashMap<String, Vec<OneshotInput<S>>> = HashMap::new();
        let mut first_input_content_type: Option<String> = None;

        for input in inputs {
            provided_inputs.entry(input.node_id.clone()).or_default().push(input);
        }

        if has_http_input {
            for node_id in &http_input_nodes {
                let Some(bound_inputs) = provided_inputs.remove(node_id) else {
                    tracing::error!(
                        "Pipeline validation failed: no stream provided for http_input node '{}'",
                        node_id
                    );
                    return Err(StreamKitError::Configuration(format!(
                        "No stream provided for http_input node '{node_id}'"
                    )));
                };

                let mut per_pin_receivers: Vec<(String, mpsc::Receiver<Bytes>, Option<String>)> =
                    Vec::new();

                for input in bound_inputs {
                    if first_input_content_type.is_none() {
                        first_input_content_type.clone_from(&input.content_type);
                    }

                    let (tx, rx) = mpsc::channel(config.io_channel_capacity);
                    per_pin_receivers.push((
                        input.output_pin.clone(),
                        rx,
                        input.content_type.clone(),
                    ));

                    let node_name = node_id.clone();
                    let mut stream = input.stream;
                    let input_stream_tx = tx;
                    let input_pump_token =
                        input.cancellation_token.unwrap_or_else(|| cancellation_token.clone());
                    let output_pin = input.output_pin.clone();

                    tokio::spawn(async move {
                        use futures::StreamExt;
                        let mut chunk_count = 0usize;
                        tracing::debug!(
                            "Input stream pump starting for node '{}', pin '{}'",
                            node_name,
                            output_pin
                        );
                        loop {
                            tokio::select! {
                                () = input_pump_token.cancelled() => {
                                    tracing::info!("Input stream pump for '{}.{}' cancelled after {} chunks", node_name, output_pin, chunk_count);
                                    break;
                                }
                                chunk_result = stream.next() => {
                                    match chunk_result {
                                        Some(Ok(chunk)) => {
                                            chunk_count += 1;
                                            if input_stream_tx.send(chunk).await.is_err() {
                                                tracing::warn!("Input node '{}.{}' closed before stream ended.", node_name, output_pin);
                                                break;
                                            }
                                        }
                                        Some(Err(e)) => {
                                            tracing::error!("Error reading from input stream for '{}.{}': {}", node_name, output_pin, e);
                                            break;
                                        }
                                        None => {
                                            tracing::info!("Input stream pump for '{}.{}' finished after {} chunks", node_name, output_pin, chunk_count);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    });
                }

                tracing::debug!("Creating special input node '{}'", node_id);
                let input_node = streamkit_nodes::core::bytes_input::BytesInputNode::with_streams(
                    per_pin_receivers,
                );
                nodes.insert(node_id.clone(), Box::new(input_node));
            }

            if !provided_inputs.is_empty() {
                let extras = provided_inputs.keys().cloned().collect::<Vec<_>>().join(", ");
                return Err(StreamKitError::Configuration(format!(
                    "Unexpected input streams provided for unknown http_input nodes: {extras}"
                )));
            }
        }

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

        // Walk backwards from the output node through the connection graph to find
        // the first node that declares a content_type.  This allows passthrough-style
        // nodes (pacer, passthrough, telemetry_tap, etc.) to be inserted before
        // http_output without losing the upstream content type.
        //
        // NOTE: This walk follows a single path — at each step it picks the first
        // connection whose `to_node` matches `cursor`.  For fan-in nodes (e.g. a
        // compositor with multiple inputs) only one arbitrary upstream branch is
        // traversed.  This is correct for content-type discovery because the
        // content-producing node (encoder / muxer) sits downstream of any fan-in
        // point, not upstream of it.
        let static_content_type = {
            let mut cursor = final_node_id.as_str();
            let mut found: Option<String> = None;
            let mut steps = 0;
            // Limit iterations to prevent infinite loops in malformed graphs.
            let max_steps = definition.nodes.len();
            for _ in 0..max_steps {
                steps += 1;
                if let Some(def) = definition.nodes.get(cursor) {
                    // Skip synthetic oneshot nodes — they are not in the
                    // registry and are handled separately by the engine.
                    if def.kind == "streamkit::http_input" || def.kind == "streamkit::http_output" {
                        break;
                    }
                    let temp = registry.create_node(&def.kind, def.params.as_ref())?;
                    if let Some(ct) = temp.content_type() {
                        found = Some(ct);
                        break;
                    }
                }
                // Move to the upstream node that feeds `cursor`.
                match definition.connections.iter().find(|c| c.to_node == cursor) {
                    Some(conn) => cursor = conn.from_node.as_str(),
                    None => break,
                }
            }
            if found.is_none() {
                tracing::warn!(
                    steps,
                    final_node = %final_node_id,
                    "Content-type backward walk did not find a content_type; \
                     response will fall back to configured or default type"
                );
            }
            found
        };

        // --- 4. Instantiate all nodes for the pipeline ---
        tracing::debug!("Creating special output node '{}'", output_node_id);
        let output_node_def = definition.nodes.get(&output_node_id).ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "Output node '{output_node_id}' not found in pipeline definition"
            ))
        })?;
        let output_node = streamkit_nodes::core::bytes_output::BytesOutputNode::new_with_config(
            output_stream_tx,
            output_node_def.params.as_ref(),
        )?;
        let configured_content_type = output_node.configured_content_type();
        nodes.insert(output_node_id.clone(), Box::new(output_node));

        tracing::debug!("Adding final node '{}' to pipeline", final_node_id);
        let final_node_instance =
            registry.create_node(&final_node_def.kind, final_node_def.params.as_ref())?;
        nodes.insert(final_node_id.clone(), final_node_instance);

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

        // --- 5. Wire and spawn ---
        tracing::info!("Wiring up and spawning pipeline graph");

        let node_kinds: HashMap<String, String> =
            definition.nodes.iter().map(|(name, def)| (name.clone(), def.kind.clone())).collect();
        let node_kinds_for_metrics = node_kinds.clone();

        let audio_pool = self.audio_pool.clone();
        let video_pool = self.video_pool.clone();

        let (stats_tx, stats_rx) = mpsc::channel(DEFAULT_STATE_CHANNEL_CAPACITY);

        let live_nodes = graph_builder::wire_and_spawn_graph(
            nodes,
            &definition.connections,
            &node_kinds,
            config.packet_batch_size,
            config.media_channel_capacity,
            None, // No state tracking for oneshot pipelines
            Some(stats_tx),
            Some(cancellation_token.clone()),
            Some(audio_pool),
            Some(video_pool),
        )
        .await?;
        tracing::info!("Pipeline graph successfully spawned");

        spawn_oneshot_metrics_recorder(stats_rx, node_kinds_for_metrics);

        // --- 5.5. Start source / generator nodes ---
        // File readers need an explicit Start signal, and so do generator nodes
        // (e.g. video::colorbars) that follow the Ready → Start lifecycle.
        // We always scan for root nodes (never a to_node in any connection) so
        // that mixed pipelines (e.g. http_input + colorbars) work correctly.
        // http_input nodes are excluded because they are driven by the incoming
        // HTTP stream rather than a Start signal.
        let mut start_node_ids: Vec<String> = source_node_ids.clone();

        {
            let downstream_nodes: std::collections::HashSet<&str> =
                definition.connections.iter().map(|c| c.to_node.as_str()).collect();
            for name in definition.nodes.keys() {
                if name != &output_node_id
                    && !downstream_nodes.contains(name.as_str())
                    && !start_node_ids.contains(name)
                    && !http_input_nodes.contains(name)
                {
                    start_node_ids.push(name.clone());
                }
            }
        }

        if !start_node_ids.is_empty() {
            tracing::info!(
                "Sending Start signals to {} source/generator node(s)",
                start_node_ids.len()
            );
            for source_id in &start_node_ids {
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

        // --- 7. Determine content-type for the response ---
        tracing::debug!(
            "Content type sources - configured: {:?}, static: {:?}, input: {:?}",
            configured_content_type,
            static_content_type,
            first_input_content_type
        );

        let content_type = configured_content_type
            .or(static_content_type)
            .or(first_input_content_type)
            .unwrap_or_else(|| "application/octet-stream".to_string());

        tracing::info!("Using content type for response: '{}'", content_type);

        Ok(OneshotPipelineResult { data_stream: output_stream_rx, content_type })
    }
}

fn spawn_oneshot_metrics_recorder(
    mut stats_rx: mpsc::Receiver<NodeStatsUpdate>,
    node_kinds: HashMap<String, String>,
) {
    let meter = global::meter("skit_engine");
    let node_packets_received_counter = meter
        .u64_counter("node.packets.received")
        .with_description("Total packets received by node")
        .build();
    let node_packets_sent_counter = meter
        .u64_counter("node.packets.sent")
        .with_description("Total packets sent by node")
        .build();
    let node_packets_discarded_counter = meter
        .u64_counter("node.packets.discarded")
        .with_description("Total packets discarded by node")
        .build();
    let node_packets_errored_counter = meter
        .u64_counter("node.packets.errored")
        .with_description("Total packet processing errors by node")
        .build();

    let node_kinds = std::sync::Arc::new(node_kinds);

    tokio::spawn(async move {
        // Track previous stats per node to compute deltas
        let mut prev_stats: HashMap<String, NodeStats> = HashMap::new();

        while let Some(update) = stats_rx.recv().await {
            let node_kind =
                node_kinds.get(&update.node_id).map_or("unknown", std::string::String::as_str);

            let labels = &[
                KeyValue::new("node_id", update.node_id.clone()),
                KeyValue::new("node_kind", node_kind.to_string()),
            ];

            let prev = prev_stats.get(&update.node_id);
            let delta_received = prev.map_or(update.stats.received, |p| {
                if update.stats.received < p.received {
                    update.stats.received
                } else {
                    update.stats.received - p.received
                }
            });
            let delta_sent = prev.map_or(update.stats.sent, |p| {
                if update.stats.sent < p.sent {
                    update.stats.sent
                } else {
                    update.stats.sent - p.sent
                }
            });
            let delta_discarded = prev.map_or(update.stats.discarded, |p| {
                if update.stats.discarded < p.discarded {
                    update.stats.discarded
                } else {
                    update.stats.discarded - p.discarded
                }
            });
            let delta_errored = prev.map_or(update.stats.errored, |p| {
                if update.stats.errored < p.errored {
                    update.stats.errored
                } else {
                    update.stats.errored - p.errored
                }
            });

            // Add deltas to counters (not absolute values)
            node_packets_received_counter.add(delta_received, labels);
            node_packets_sent_counter.add(delta_sent, labels);
            node_packets_discarded_counter.add(delta_discarded, labels);
            node_packets_errored_counter.add(delta_errored, labels);

            // Update previous stats for this node
            prev_stats.insert(update.node_id.clone(), update.stats);
        }
    });
}
