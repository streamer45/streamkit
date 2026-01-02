// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! HTTP pull node - Fetches and streams data from HTTP/HTTPS URLs

use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};

/// Configuration for the HttpPullNode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HttpPullConfig {
    /// URL to fetch (HTTP or HTTPS)
    pub url: String,
    /// Size of chunks to read (default: 8192 bytes)
    #[serde(default = "default_chunk_size")]
    #[schemars(range(min = 1))]
    pub chunk_size: usize,
}

const fn default_chunk_size() -> usize {
    8192
}

/// A node that fetches data from an HTTP/HTTPS URL and outputs it as Binary packets.
///
/// This node attempts to use HTTP range requests for efficient streaming.
/// If range requests are not supported by the server, it falls back to downloading
/// the entire file to a temporary location and streaming from there.
pub struct HttpPullNode {
    config: HttpPullConfig,
}

impl HttpPullNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            // For dynamic nodes, allow None to create a default instance for pin inspection
            let config: HttpPullConfig = if params.is_none() {
                // Default config for pin inspection only
                HttpPullConfig {
                    url: "http://example.com".to_string(),
                    chunk_size: default_chunk_size(),
                }
            } else {
                config_helpers::parse_config_required(params)?
            };

            // Validate chunk_size to prevent infinite loop
            if config.chunk_size == 0 {
                return Err(StreamKitError::Configuration(
                    "chunk_size must be greater than 0".to_string(),
                ));
            }

            Ok(Box::new(Self { config }))
        })
    }

    fn shared_http_client() -> Result<&'static reqwest::Client, StreamKitError> {
        static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    // Security: don't follow redirects (avoid SSRF allowlist bypass patterns).
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(5))
                    .build()
            })
            .as_ref()
            .map_err(|e| StreamKitError::Runtime(format!("Failed to initialize HTTP client: {e}")))
    }

    /// Stream response body using bytes_stream() for efficient streaming.
    /// This avoids buffering the entire response in memory and uses a single HTTP request.
    async fn stream_response(
        url: &str,
        chunk_size: usize,
        context: &mut NodeContext,
        stats_tracker: &mut NodeStatsTracker,
    ) -> Result<(), StreamKitError> {
        let client = Self::shared_http_client()?;

        tracing::info!("Starting streaming GET request to {}", url);

        let response = match client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                stats_tracker.errored();
                return Err(StreamKitError::Runtime(format!("HTTP request failed: {e}")));
            },
        };

        if !response.status().is_success() {
            stats_tracker.errored();
            return Err(StreamKitError::Runtime(format!("HTTP error: {}", response.status())));
        }

        // Get content length if available for logging
        let content_length = response.content_length();
        if let Some(len) = content_length {
            tracing::info!("Content-Length: {} bytes", len);
        }

        // Stream the response body using bytes_stream()
        let mut stream = response.bytes_stream();
        let mut chunk_count = 0u64;
        let mut total_bytes = 0u64;

        // Buffer for accumulating small chunks to reach chunk_size
        // Using BytesMut for O(1) split_to() instead of O(n) Vec::drain()
        // Use saturating_mul to prevent overflow for huge chunk_size values
        let mut buffer = BytesMut::with_capacity(chunk_size.saturating_mul(2));

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    stats_tracker.errored();
                    return Err(StreamKitError::Runtime(format!("Failed to read chunk: {e}")));
                },
            };

            total_bytes += chunk.len() as u64;
            buffer.put_slice(&chunk);

            // Send when buffer reaches or exceeds chunk_size
            // split_to() is O(1) - just adjusts internal pointers
            while buffer.len() >= chunk_size {
                let to_send = buffer.split_to(chunk_size).freeze();
                chunk_count += 1;

                if context
                    .output_sender
                    .send(
                        "out",
                        Packet::Binary { data: to_send, content_type: None, metadata: None },
                    )
                    .await
                    .is_err()
                {
                    tracing::debug!("Output channel closed, stopping node");
                    return Ok(());
                }

                stats_tracker.sent();
                stats_tracker.maybe_send();
            }
        }

        // Send any remaining data in the buffer
        if !buffer.is_empty() {
            chunk_count += 1;

            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary { data: buffer.freeze(), content_type: None, metadata: None },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed, stopping node");
                return Ok(());
            }

            stats_tracker.sent();
        }

        tracing::info!("Completed streaming: {} chunks, {} total bytes", chunk_count, total_bytes);

        Ok(())
    }
}

#[async_trait]
impl ProcessorNode for HttpPullNode {
    fn input_pins(&self) -> Vec<InputPin> {
        // HTTP input nodes have no input pins
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

        tracing::info!(
            "HttpPullNode fetching from: {} (chunk_size: {})",
            self.config.url,
            self.config.chunk_size
        );

        // Source nodes emit Ready state and wait for Start signal
        state_helpers::emit_ready(&context.state_tx, &node_name);
        tracing::info!("HttpPullNode ready, waiting for start signal");

        // Wait for Start control message
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Start) => {
                    tracing::info!("HttpPullNode received start signal");
                    break;
                },
                Some(streamkit_core::control::NodeControlMessage::UpdateParams(_)) => {
                    // Ignore param updates while waiting to start - loop continues naturally
                },
                Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                    tracing::info!("HttpPullNode received shutdown before start");
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

        // Use streaming GET - single request, streams response body
        let result = Self::stream_response(
            &self.config.url,
            self.config.chunk_size,
            &mut context,
            &mut stats_tracker,
        )
        .await;

        stats_tracker.force_send();

        match result {
            Ok(()) => {
                state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
                Ok(())
            },
            Err(e) => {
                state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
                Err(e)
            },
        }
    }
}

/// Register HTTP nodes with the registry
///
/// # Panics
///
/// Panics if the config schema cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_http_nodes(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let factory = HttpPullNode::factory();
    registry.register_dynamic_with_description(
        "transport::http::fetcher",
        move |params| (factory)(params),
        serde_json::to_value(schema_for!(HttpPullConfig))
            .expect("HttpPullConfig schema should serialize to JSON"),
        vec!["transport".to_string(), "http".to_string()],
        false,
        "Fetches binary data from an HTTP/HTTPS URL. \
         Security: this is an SSRF-capable node; restrict it via role allowlists. \
         Redirects are disabled (v0.1.x).",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::Request,
        http::{header, StatusCode},
        response::Response,
        routing::get,
        Router,
    };
    use std::collections::HashMap;
    use streamkit_core::node::RoutedPacketMessage;
    use streamkit_core::NodeStatsUpdate;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_http_pull_node_structure() {
        // Test that we can create the node
        let config =
            HttpPullConfig { url: "http://example.com/test.bin".to_string(), chunk_size: 1024 };
        let node = Box::new(HttpPullNode { config });

        // Verify pins
        assert_eq!(node.input_pins().len(), 0);
        assert_eq!(node.output_pins().len(), 1);
        assert_eq!(node.output_pins()[0].name, "out");
        assert_eq!(node.output_pins()[0].produces_type, PacketType::Binary);
    }

    /// Helper to create a mock HTTP server for streaming tests
    #[allow(clippy::unwrap_used)]
    async fn start_mock_server(_test_data: &'static [u8]) -> Option<String> {
        #[allow(clippy::unwrap_used)]
        async fn handle_request(req: Request<Body>) -> Response {
            let test_data: &'static [u8] = b"Hello, StreamKit! This is test data for HTTP pull.";

            if req.method() == "HEAD" {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, test_data.len())
                    .body(Body::empty())
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, test_data.len())
                    .body(Body::from(test_data.to_vec()))
                    .unwrap()
            }
        }

        let app = Router::new().route("/test.bin", get(handle_request).head(handle_request));

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
            Err(e) => panic!("Failed to bind test HTTP listener: {e}"),
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Some(format!("http://{addr}/test.bin"))
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_http_pull_streaming() {
        let Some(url) =
            start_mock_server(b"Hello, StreamKit! This is test data for HTTP pull.").await
        else {
            tracing::warn!("Skipping test_http_pull_streaming: local TCP bind not permitted");
            return;
        };

        // Create test context
        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);
        let (control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_http_pull".to_string(),
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

        // Create and run node with small chunk size for testing
        let config = HttpPullConfig {
            url: url.clone(),
            chunk_size: 10, // Small chunks to test range requests
        };
        let node = Box::new(HttpPullNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for initializing state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Initializing));

        // Wait for ready state
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Ready));

        // Send start signal
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
        assert_eq!(collected_data, b"Hello, StreamKit! This is test data for HTTP pull.");
    }
}
