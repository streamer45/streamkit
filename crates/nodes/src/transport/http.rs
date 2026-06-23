// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

        let content_length = response.content_length();
        if let Some(len) = content_length {
            tracing::info!("Content-Length: {} bytes", len);
        }

        let mut stream = response.bytes_stream();
        let mut chunk_count = 0u64;
        let mut total_bytes = 0u64;

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

        state_helpers::emit_ready(&context.state_tx, &node_name);
        tracing::info!("HttpPullNode ready, waiting for start signal");

        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Start) => {
                    tracing::info!("HttpPullNode received start signal");
                    break;
                },
                Some(streamkit_core::control::NodeControlMessage::UpdateParams(_)) => {},
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

/// Register HTTP nodes with the registry.
pub fn register_http_nodes(registry: &mut streamkit_core::NodeRegistry) {
    register_dynamic_node!(
        registry,
        "transport::http::fetcher",
        HttpPullNode,
        HttpPullConfig,
        ["transport", "http"],
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

    const MOCK_BODY: &[u8] = b"Hello, StreamKit! This is test data for HTTP pull.";

    /// Returns `None` when sandboxed CI cannot bind a loopback listener.
    #[allow(clippy::unwrap_used)] // test server setup unwraps only known-good static responses and listener state
    async fn start_mock_server() -> Option<String> {
        #[allow(clippy::unwrap_used)] // building static test responses cannot fail
        async fn handle_test_bin(req: Request<Body>) -> Response {
            if req.method() == "HEAD" {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, MOCK_BODY.len())
                    .body(Body::empty())
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, MOCK_BODY.len())
                    .body(Body::from(MOCK_BODY.to_vec()))
                    .unwrap()
            }
        }

        #[allow(clippy::unwrap_used)] // building static test responses cannot fail
        async fn handle_chunked() -> Response {
            let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
                Ok(bytes::Bytes::from_static(b"chunk-one;")),
                Ok(bytes::Bytes::from_static(b"chunk-two;")),
                Ok(bytes::Bytes::from_static(b"chunk-three")),
            ];
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap()
        }

        #[allow(clippy::unwrap_used)] // building static test responses cannot fail
        async fn handle_not_found() -> Response {
            Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap()
        }

        #[allow(clippy::unwrap_used)] // building static test responses cannot fail
        async fn handle_typed() -> Response {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(MOCK_BODY.to_vec()))
                .unwrap()
        }

        let app = Router::new()
            .route("/test.bin", get(handle_test_bin).head(handle_test_bin))
            .route("/chunked", get(handle_chunked))
            .route("/status/404", get(handle_not_found))
            .route("/typed", get(handle_typed));

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
            Err(e) => panic!("Failed to bind test HTTP listener: {e}"),
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // No startup wait needed: the listener is already bound, so the kernel
        // queues connections until `axum::serve` accepts them.
        Some(format!("http://{addr}"))
    }

    struct PullOutcome {
        data: Vec<u8>,
        content_types: Vec<Option<std::borrow::Cow<'static, str>>>,
        final_state: streamkit_core::NodeState,
        result: Result<(), StreamKitError>,
    }

    #[allow(clippy::unwrap_used)] // the lifecycle channels are expected to deliver these messages in tests
    async fn drive_pull(url: String, chunk_size: usize) -> PullOutcome {
        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(32);
        let (control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_http_pull".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs: HashMap::new(),
            input_types: HashMap::new(),
            control_rx,
            output_sender,
            batch_size: 32,
            state_tx,
            stats_tx: Some(stats_tx),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: None,
            pin_management_rx: None,
            audio_pool: None,
            video_pool: None,
            pipeline_mode: streamkit_core::PipelineMode::Dynamic,
            view_data_tx: None,
            engine_control_tx: None,
            asset_root: crate::test_utils::test_asset_root(),
        };

        let node = Box::new(HttpPullNode { config: HttpPullConfig { url, chunk_size } });
        let node_handle = tokio::spawn(async move { node.run(context).await });

        assert!(matches!(
            state_rx.recv().await.unwrap().state,
            streamkit_core::NodeState::Initializing
        ));
        assert!(matches!(state_rx.recv().await.unwrap().state, streamkit_core::NodeState::Ready));
        control_tx.send(streamkit_core::control::NodeControlMessage::Start).await.unwrap();
        assert!(matches!(state_rx.recv().await.unwrap().state, streamkit_core::NodeState::Running));

        let mut data = Vec::new();
        let mut content_types = Vec::new();
        while let Some((_node, _pin, packet)) = packet_rx.recv().await {
            if let Packet::Binary { data: chunk, content_type, .. } = packet {
                data.extend_from_slice(&chunk);
                content_types.push(content_type);
            }
        }

        let final_state = state_rx.recv().await.unwrap().state;
        let result = node_handle.await.unwrap();

        PullOutcome { data, content_types, final_state, result }
    }

    #[tokio::test]
    async fn test_http_pull_streaming() {
        let Some(base) = start_mock_server().await else {
            tracing::warn!("Skipping test_http_pull_streaming: local TCP bind not permitted");
            return;
        };

        // Small chunk_size exercises the range-request re-splitting path.
        let outcome = drive_pull(format!("{base}/test.bin"), 10).await;

        assert!(outcome.result.is_ok());
        assert!(matches!(outcome.final_state, streamkit_core::NodeState::Stopped { .. }));
        assert_eq!(outcome.data, MOCK_BODY);
    }

    #[tokio::test]
    async fn streams_reassembled_chunked_body() {
        let Some(base) = start_mock_server().await else {
            tracing::warn!(
                "Skipping streams_reassembled_chunked_body: local TCP bind not permitted"
            );
            return;
        };

        // Small chunk_size forces the node to re-split the multiple network
        // chunks into several output packets before reassembly.
        let outcome = drive_pull(format!("{base}/chunked"), 4).await;

        assert!(outcome.result.is_ok());
        assert!(matches!(outcome.final_state, streamkit_core::NodeState::Stopped { .. }));
        assert_eq!(outcome.data, b"chunk-one;chunk-two;chunk-three");
        assert!(outcome.content_types.len() > 1, "small chunk_size should yield multiple packets");
    }

    #[tokio::test]
    async fn non_success_status_fails_node() {
        let Some(base) = start_mock_server().await else {
            tracing::warn!("Skipping non_success_status_fails_node: local TCP bind not permitted");
            return;
        };

        let outcome = drive_pull(format!("{base}/status/404"), 1024).await;

        assert!(outcome.data.is_empty(), "no body should be streamed on a 404");
        match outcome.result {
            Err(StreamKitError::Runtime(msg)) => assert!(msg.contains("404")),
            other => panic!("expected a Runtime error mentioning the status, got {other:?}"),
        }
        assert!(matches!(outcome.final_state, streamkit_core::NodeState::Failed { .. }));
    }

    /// `HttpPullNode` emits opaque `Binary` packets and intentionally does not
    /// forward the upstream `Content-Type`; pin that behaviour so a future
    /// change is a conscious one.
    #[tokio::test]
    async fn content_type_is_not_propagated() {
        let Some(base) = start_mock_server().await else {
            tracing::warn!("Skipping content_type_is_not_propagated: local TCP bind not permitted");
            return;
        };

        let outcome = drive_pull(format!("{base}/typed"), 1024).await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.data, MOCK_BODY);
        assert!(
            outcome.content_types.iter().all(Option::is_none),
            "node should not propagate the server Content-Type"
        );
    }
}
