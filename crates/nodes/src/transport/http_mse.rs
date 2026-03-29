// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! HTTP MSE output node — serves WebM streams to HTTP clients for MSE playback.
//!
//! This node accepts `Packet::Binary` data (WebM chunks) from an upstream muxer
//! and broadcasts them to connected HTTP clients as chunked responses. Late-joining
//! clients receive the buffered WebM initialization segment so MSE
//! `SourceBuffer.appendBuffer()` works correctly.

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

/// Configuration for the HttpMse node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HttpMseConfig {
    /// Path suffix for the MSE stream endpoint (e.g., "/video").
    /// Full URL will be: `/mse/{session_id}{path}`
    pub path: String,
    /// Maximum concurrent HTTP clients (default: 10).
    #[serde(default = "default_max_clients")]
    #[schemars(range(min = 1))]
    pub max_clients: u32,
    /// Content type for the HTTP response.
    /// Defaults to `video/webm; codecs="vp9,opus"`.
    /// For best MSE compatibility, include the codecs parameter
    /// (e.g., `video/webm; codecs="vp9,opus"` or `video/webm; codecs="vp9"`).
    #[serde(default)]
    pub content_type: Option<String>,
}

const fn default_max_clients() -> u32 {
    10
}

/// Default content type for WebM MSE streams.
/// Includes the codecs parameter for best browser compatibility with MSE.
const DEFAULT_CONTENT_TYPE: &str = "video/webm; codecs=\"vp9,opus\"";

/// Maximum size for the init segment buffer (16 KB).
/// WebM init segments are typically < 1 KB; this is a generous upper bound.
const MAX_INIT_SEGMENT_SIZE: usize = 16 * 1024;

/// WebM Cluster element ID (0x1F43B675).
/// Used to detect when the init segment ends and media data begins.
const WEBM_CLUSTER_ID: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];

/// A node that serves WebM binary data to HTTP clients for MSE playback.
///
/// It accepts `Packet::Binary` from an upstream WebM muxer (in Live mode) and
/// fans out the chunks to all connected HTTP clients. The WebM initialization
/// segment (EBML header + Segment + Tracks) is buffered and replayed to
/// late-joining clients.
pub struct HttpMseNode {
    config: HttpMseConfig,
}

impl HttpMseNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: HttpMseConfig = if params.is_none() {
                // Default config for pin inspection only (dynamic pipelines)
                HttpMseConfig {
                    path: "/video".to_string(),
                    max_clients: default_max_clients(),
                    content_type: None,
                }
            } else {
                config_helpers::parse_config_required(params)?
            };

            if config.max_clients == 0 {
                return Err(StreamKitError::Configuration(
                    "max_clients must be greater than 0".to_string(),
                ));
            }

            Ok(Box::new(Self { config }))
        })
    }
}

#[async_trait]
impl ProcessorNode for HttpMseNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        // Pure sink node — no outputs.
        vec![]
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Session ID is required for gateway registration.
        let session_id = context.session_id.as_ref().ok_or_else(|| {
            let err = "transport::http::mse requires a session_id for gateway registration";
            tracing::error!("{}", err);
            StreamKitError::Configuration(err.to_string())
        })?;

        // Get the MSE gateway from the global registry.
        let gateway = streamkit_core::mse_gateway::get_mse_gateway().ok_or_else(|| {
            let err = "MSE gateway not available — ensure transport::http::mse is used in a session with gateway support";
            tracing::error!("{}", err);
            StreamKitError::Runtime(err.to_string())
        })?;

        let content_type =
            self.config.content_type.clone().unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string());

        // Build the full registration path: /mse/{session_id}{path}
        let path_suffix = if self.config.path.starts_with('/') {
            self.config.path.clone()
        } else {
            format!("/{}", self.config.path)
        };
        let full_path = format!("/mse/{session_id}{path_suffix}");

        tracing::info!(
            path = %full_path,
            session_id = %session_id,
            content_type = %content_type,
            max_clients = self.config.max_clients,
            "HttpMseNode registering MSE stream"
        );

        // Register with the MSE gateway, passing max_clients so the gateway
        // can enforce capacity and reject clients with a proper 503.
        let mut client_rx = gateway
            .register_stream(
                full_path.clone(),
                session_id.clone(),
                content_type,
                self.config.max_clients,
            )
            .await
            .map_err(|e| {
                let err = format!("Failed to register MSE stream: {e}");
                tracing::error!("{}", err);
                StreamKitError::Runtime(err)
            })?;

        // Take ownership of the input channel.
        let mut input_rx = context.take_input("in").map_err(|e| {
            tracing::error!("Failed to take input pin: {}", e);
            e
        })?;

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Connected clients (body_tx senders).
        let mut clients: Vec<mpsc::Sender<Bytes>> = Vec::new();

        // Init segment buffer: accumulates all bytes before the first WebM Cluster.
        let mut init_segment: Vec<u8> = Vec::new();
        let mut init_complete = false;

        // Overlap buffer for detecting Cluster ID across chunk boundaries.
        // Holds the last 3 bytes of the previous chunk so a 4-byte Cluster ID
        // that straddles two consecutive packets can still be found.
        let mut overlap: Vec<u8> = Vec::new();

        let cancellation_token = context.cancellation_token.clone();

        let result: Result<(), StreamKitError> = loop {
            tokio::select! {
                biased;

                // Shutdown via cancellation token.
                () = async {
                    if let Some(ref token) = cancellation_token {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    tracing::info!("HttpMseNode shutting down (cancellation)");
                    break Ok(());
                }

                // Control messages.
                msg = context.control_rx.recv() => {
                    match msg {
                        Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                            tracing::info!("HttpMseNode received shutdown");
                            break Ok(());
                        }
                        Some(_) => { /* ignore other control messages */ }
                        None => {
                            tracing::debug!("Control channel closed");
                            break Ok(());
                        }
                    }
                }

                // New HTTP client connecting.
                client = client_rx.recv() => {
                    let Some(client) = client else {
                        tracing::warn!("MSE gateway client channel closed");
                        break Ok(());
                    };

                    // Send the init segment to the new client so MSE can initialize.
                    // Use try_send to avoid blocking the event loop if the client's
                    // channel is already full (unlikely for a fresh client, but safe).
                    if !init_segment.is_empty() {
                        let init_bytes = Bytes::copy_from_slice(&init_segment);
                        match client.body_tx.try_send(init_bytes) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                tracing::debug!("New MSE client disconnected before init segment delivery");
                                continue;
                            }
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                tracing::warn!("New MSE client channel full during init segment delivery, disconnecting");
                                continue;
                            }
                        }
                    }

                    tracing::debug!(
                        client_count = clients.len() + 1,
                        "New MSE client connected"
                    );
                    clients.push(client.body_tx);
                }

                // Incoming WebM data from upstream muxer.
                packet = input_rx.recv() => {
                    let Some(packet) = packet else {
                        tracing::info!("Input channel closed");
                        break Ok(());
                    };

                    stats_tracker.received();

                    let Packet::Binary { data, .. } = packet else {
                        tracing::warn!("HttpMseNode received non-binary packet, ignoring");
                        stats_tracker.discarded();
                        continue;
                    };

                    // Buffer the init segment (everything before the first Cluster element).
                    if !init_complete {
                        // Search for the Cluster ID in the overlap + current chunk to handle
                        // the case where the 4-byte ID straddles two consecutive packets.
                        let cluster_found = if overlap.is_empty() {
                            false
                        } else {
                            let mut combined = overlap.clone();
                            combined.extend_from_slice(&data[..data.len().min(WEBM_CLUSTER_ID.len())]);
                            find_cluster_id(&combined).is_some_and(|pos| {
                                // Cluster ID starts at `pos` within the overlap region.
                                // The init segment ends at the overlap boundary minus
                                // the bytes we need to trim from the already-buffered tail.
                                let overlap_len = overlap.len();
                                if pos < overlap_len {
                                    // The Cluster started inside already-buffered data;
                                    // truncate the init segment to exclude those bytes.
                                    init_segment.truncate(init_segment.len() - (overlap_len - pos));
                                } else {
                                    // The Cluster started inside the new chunk.
                                    let extra = pos - overlap_len;
                                    let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                    let to_append = extra.min(remaining_capacity);
                                    init_segment.extend_from_slice(&data[..to_append]);
                                }
                                true
                            })
                        };

                        if cluster_found {
                            init_complete = true;
                        } else if let Some(cluster_offset) = find_cluster_id(&data) {
                            // Cluster ID found entirely within this chunk.
                            if cluster_offset > 0 {
                                let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                let to_append = cluster_offset.min(remaining_capacity);
                                init_segment.extend_from_slice(&data[..to_append]);
                            }
                            init_complete = true;
                        } else {
                            // Still in the init segment — buffer these bytes.
                            let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                            if remaining_capacity > 0 {
                                let to_append = data.len().min(remaining_capacity);
                                init_segment.extend_from_slice(&data[..to_append]);
                            }
                            // Keep the last 3 bytes for cross-chunk Cluster ID detection.
                            let keep = data.len().min(WEBM_CLUSTER_ID.len() - 1);
                            overlap.clear();
                            overlap.extend_from_slice(&data[data.len() - keep..]);
                        }

                        if init_complete {
                            tracing::info!(
                                init_segment_size = init_segment.len(),
                                "WebM init segment captured"
                            );
                            overlap.clear();
                        }
                    }

                    // Broadcast to all connected clients, removing disconnected or slow ones.
                    if !clients.is_empty() {
                        let chunk = data.clone();
                        let mut i = 0;
                        while i < clients.len() {
                            // Use try_send to avoid blocking the pipeline on a slow client.
                            match clients[i].try_send(chunk.clone()) {
                                Ok(()) => {
                                    // stats_tracker.sent() counts per-client delivery, not per-packet.
                                    // This differs from single-output nodes but accurately reflects
                                    // the total number of chunk deliveries across all clients.
                                    stats_tracker.sent();
                                    i += 1;
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    clients.swap_remove(i);
                                    tracing::debug!(
                                        client_count = clients.len(),
                                        "MSE client disconnected"
                                    );
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // Dropping chunks from a WebM stream corrupts the container
                                    // format — MSE SourceBuffer will throw decode errors with
                                    // no recovery path. Disconnect the slow client entirely
                                    // rather than silently dropping data.
                                    clients.swap_remove(i);
                                    tracing::warn!(
                                        client_count = clients.len(),
                                        "MSE client too slow, disconnecting to avoid corrupt stream"
                                    );
                                }
                            }
                        }
                    }

                    stats_tracker.maybe_send();
                }
            }
        };

        // Clean up: unregister from gateway.
        gateway.unregister_stream(&full_path).await;
        clients.clear();

        stats_tracker.force_send();

        match &result {
            Ok(()) => {
                state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
            },
            Err(e) => {
                state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
            },
        }

        result
    }
}

/// Search for the WebM Cluster element ID (0x1F43B675) in a byte slice.
/// Returns the offset of the first occurrence, or `None` if not found.
fn find_cluster_id(data: &[u8]) -> Option<usize> {
    if data.len() < WEBM_CLUSTER_ID.len() {
        return None;
    }
    data.windows(WEBM_CLUSTER_ID.len()).position(|window| window == WEBM_CLUSTER_ID)
}

/// Register HTTP MSE nodes with the registry.
///
/// # Panics
///
/// Panics if the config schema cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)]
pub fn register_http_mse_nodes(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let factory = HttpMseNode::factory();
    registry.register_dynamic_with_description(
        "transport::http::mse",
        move |params| (factory)(params),
        serde_json::to_value(schema_for!(HttpMseConfig))
            .expect("HttpMseConfig schema should serialize to JSON"),
        vec!["transport".to_string(), "http".to_string(), "mse".to_string()],
        false,
        "Serves WebM streams to HTTP clients for MSE (Media Source Extensions) playback. \
         Accepts binary data from an upstream WebM muxer and broadcasts to multiple \
         concurrent HTTP clients with init segment replay for late-joiners.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_cluster_id() {
        // No cluster
        assert_eq!(find_cluster_id(b"hello world"), None);

        // Cluster at start
        let data = vec![0x1F, 0x43, 0xB6, 0x75, 0x00, 0x01];
        assert_eq!(find_cluster_id(&data), Some(0));

        // Cluster after init segment
        let mut with_header = vec![0x1A, 0x45, 0xDF, 0xA3]; // EBML header ID
        with_header.extend_from_slice(&[0x00; 20]); // some header data
        with_header.extend_from_slice(&WEBM_CLUSTER_ID);
        with_header.extend_from_slice(&[0x00; 10]); // cluster data
        assert_eq!(find_cluster_id(&with_header), Some(24));

        // Too short
        assert_eq!(find_cluster_id(&[0x1F, 0x43]), None);
        assert_eq!(find_cluster_id(&[]), None);
    }

    #[test]
    fn test_http_mse_node_structure() {
        let config =
            HttpMseConfig { path: "/video".to_string(), max_clients: 10, content_type: None };
        let node = Box::new(HttpMseNode { config });

        // Verify pins
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.input_pins()[0].name, "in");
        assert_eq!(node.input_pins()[0].accepts_types, vec![PacketType::Binary]);
        assert_eq!(node.output_pins().len(), 0);
    }

    #[test]
    fn test_factory_rejects_zero_max_clients() {
        let factory = HttpMseNode::factory();
        let params = Some(serde_json::json!({
            "path": "/video",
            "max_clients": 0,
        }));
        let result = (factory)(params.as_ref());
        assert!(result.is_err());
    }

    #[test]
    fn test_factory_accepts_valid_config() {
        let factory = HttpMseNode::factory();
        let params = Some(serde_json::json!({
            "path": "/video",
            "max_clients": 5,
        }));
        let result = (factory)(params.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_factory_default_for_pin_inspection() {
        let factory = HttpMseNode::factory();
        let result = (factory)(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_default_content_type_includes_codecs() {
        assert!(DEFAULT_CONTENT_TYPE.contains("codecs"));
    }
}
