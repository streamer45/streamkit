// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use rand::{distr::Alphanumeric, Rng};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::client::process_oneshot_with_client;
use crate::load_test::config::LoadTestConfig;
use crate::load_test::metrics::{MetricsCollector, OperationResult, OperationType};

const LOAD_TEST_BROADCAST_PREFIX: &str = "lt/";

fn prefix_load_test_broadcast(broadcast: &str) -> String {
    let base = broadcast.strip_prefix(LOAD_TEST_BROADCAST_PREFIX).unwrap_or(broadcast);
    format!("{LOAD_TEST_BROADCAST_PREFIX}{base}")
}

pub async fn oneshot_worker(
    worker_id: usize,
    config: LoadTestConfig,
    metrics: MetricsCollector,
    shutdown: tokio_util::sync::CancellationToken,
) {
    debug!("OneShot worker {} started", worker_id);

    let client = reqwest::Client::new();

    let pipeline_path = &config.oneshot.pipeline;
    let input_path = &config.oneshot.input_file;

    // Create a temp output path that we won't actually write
    let output_path = if cfg!(windows) { "NUL" } else { "/dev/null" };

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                break;
            }
            () = async {
                let start = Instant::now();
                let result = process_oneshot_with_client(
                    &client,
                    pipeline_path,
                    input_path,
                    output_path,
                    &config.server.url,
                )
                .await;

                let latency = start.elapsed();

                let (success, error) = match result {
                    Ok(()) => (true, None),
                    Err(e) => {
                        warn!("OneShot worker {} error: {}", worker_id, e);
                        (false, Some(e.to_string()))
                    }
                };

                metrics
                    .record(OperationResult {
                        op_type: OperationType::OneShot,
                        latency,
                        success,
                        error,
                    })
                    .await;

                // If operation failed, back off to avoid spinning
                if !success {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            } => {}
        }
    }

    debug!("OneShot worker {} stopped", worker_id);
}

pub struct DynamicSession {
    pub session_id: String,
    #[allow(dead_code)]
    pub pipeline_path: String,
    pub tunable_node_ids: Vec<String>,
}

fn find_tunable_gain_node_ids(v: &serde_json::Value) -> Vec<String> {
    let Some(root) = v.as_object() else {
        return Vec::new();
    };

    let Some(nodes) = root.get("nodes") else {
        return Vec::new();
    };

    let Some(nodes_map) = nodes.as_object() else {
        return Vec::new();
    };

    let mut ids = Vec::new();
    for (node_id, node_def) in nodes_map {
        let Some(node_def) = node_def.as_object() else {
            continue;
        };
        let Some(kind) = node_def.get("kind").and_then(|v| v.as_str()) else {
            continue;
        };

        if kind == "audio::gain" {
            ids.push(node_id.clone());
        }
    }

    ids
}

fn rewrite_paths_for_load_test(v: &mut serde_json::Value, unique_id: &str) {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if key == "path" {
                    if let serde_json::Value::String(s) = val {
                        if let Some(rest) = s.strip_prefix("/moq/") {
                            if !rest.starts_with(unique_id) {
                                *s = format!("/moq/{unique_id}/{rest}");
                            }
                        }
                    }
                    continue;
                }

                rewrite_paths_for_load_test(val, unique_id);
            }
        },
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                rewrite_paths_for_load_test(val, unique_id);
            }
        },
        _ => {},
    }
}

fn rewrite_yaml_for_load_test(v: &mut serde_json::Value, run_id: &str, session_id: &str) {
    let mut subscriber_broadcasts = std::collections::HashSet::<String>::new();
    let mut publisher_broadcasts = std::collections::HashSet::<String>::new();

    // First pass: collect broadcast names per node kind.
    if let Some(root) = v.as_object() {
        if let Some(nodes_map) = root.get("nodes").and_then(|n| n.as_object()) {
            for (_node_id, node_def) in nodes_map {
                let Some(node_def) = node_def.as_object() else {
                    continue;
                };

                let kind = node_def.get("kind").and_then(|v| v.as_str()).unwrap_or("");

                let is_publisher = kind.contains("publisher");
                let is_subscriber = kind.contains("subscriber");

                let Some(params) = node_def.get("params").and_then(|p| p.as_object()) else {
                    continue;
                };

                let Some(broadcast) = params.get("broadcast").and_then(|b| b.as_str()) else {
                    continue;
                };

                if is_subscriber {
                    subscriber_broadcasts.insert(broadcast.to_string());
                }
                if is_publisher {
                    publisher_broadcasts.insert(broadcast.to_string());
                }
            }
        }
    }

    // Any broadcast that is both published and subscribed within the same pipeline is
    // considered "internal" (self-contained). Those should be rewritten per-session.
    let internal_broadcasts: std::collections::HashSet<String> =
        subscriber_broadcasts.intersection(&publisher_broadcasts).cloned().collect();

    // Second pass: rewrite broadcasts.
    if let Some(root) = v.as_object_mut() {
        if let Some(nodes_map) = root.get_mut("nodes").and_then(|n| n.as_object_mut()) {
            for (_node_id, node_def) in nodes_map {
                let Some(node_def) = node_def.as_object_mut() else {
                    continue;
                };

                let kind = node_def.get("kind").and_then(|v| v.as_str()).unwrap_or("");

                let is_publisher = kind.contains("publisher");
                let is_subscriber = kind.contains("subscriber");
                if !is_publisher && !is_subscriber {
                    continue;
                }

                let Some(params) = node_def.get_mut("params").and_then(|p| p.as_object_mut())
                else {
                    continue;
                };

                let Some(broadcast_val) = params.get_mut("broadcast") else {
                    continue;
                };
                let Some(broadcast) = broadcast_val.as_str() else {
                    continue;
                };

                let base_broadcast = prefix_load_test_broadcast(broadcast);

                let rewritten = if internal_broadcasts.contains(broadcast) {
                    format!("{base_broadcast}-{session_id}")
                } else if is_publisher {
                    format!("{base_broadcast}-{run_id}-{session_id}")
                } else {
                    // Subscriber to an external broadcast: use a per-run namespace so we don't
                    // collide with browsers or other local tests.
                    format!("{base_broadcast}-{run_id}")
                };

                if broadcast != rewritten {
                    *broadcast_val = serde_json::Value::String(rewritten);
                }
            }
        }
    }

    rewrite_paths_for_load_test(v, session_id);
}

fn rewrite_broadcaster_yaml_for_load_test(v: &mut serde_json::Value, run_id: &str) {
    // Broadcasters should publish to the same per-run broadcast name that external subscribers use.
    if let Some(root) = v.as_object_mut() {
        if let Some(nodes_map) = root.get_mut("nodes").and_then(|n| n.as_object_mut()) {
            for (_node_id, node_def) in nodes_map {
                let Some(node_def) = node_def.as_object_mut() else {
                    continue;
                };

                let kind = node_def.get("kind").and_then(|v| v.as_str()).unwrap_or("");

                if !kind.contains("publisher") {
                    continue;
                }

                let Some(params) = node_def.get_mut("params").and_then(|p| p.as_object_mut())
                else {
                    continue;
                };

                let Some(broadcast_val) = params.get_mut("broadcast") else {
                    continue;
                };
                let Some(broadcast) = broadcast_val.as_str() else {
                    continue;
                };

                let base_broadcast = prefix_load_test_broadcast(broadcast);
                let rewritten = format!("{base_broadcast}-{run_id}");
                if broadcast != rewritten {
                    *broadcast_val = serde_json::Value::String(rewritten);
                }
            }
        }
    }
}

// Create a session with a full pipeline from a YAML file using HTTP API
async fn create_session_with_pipeline(
    pipeline_path: &str,
    session_name: &str,
    server_url: &str,
    run_id: &str,
) -> Result<(String, Vec<String>), Box<dyn std::error::Error + Send + Sync>> {
    use reqwest;
    use serde::{Deserialize, Serialize};
    use tokio::fs;

    #[derive(Serialize)]
    struct CreateSessionRequest {
        yaml: String,
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct CreateSessionResponse {
        session_id: String,
    }

    // Generate a unique suffix for this session to avoid conflicts
    let unique_id = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();

    // Read + rewrite the pipeline YAML file to reduce cross-session conflicts.
    let yaml_raw = fs::read_to_string(pipeline_path).await?;
    let mut yaml: serde_json::Value = serde_saphyr::from_str(&yaml_raw)?;
    let tunable_node_ids = find_tunable_gain_node_ids(&yaml);
    rewrite_yaml_for_load_test(&mut yaml, run_id, &unique_id);
    let yaml = serde_saphyr::to_string(&yaml)?;

    // Replace any hardcoded ports (if present)
    // Note: This is a simple approach; could be more sophisticated with actual YAML parsing

    // Prepare the request
    let request = CreateSessionRequest { yaml, name: Some(session_name.to_string()) };

    // Send HTTP POST to /api/v1/sessions
    let client = reqwest::Client::new();
    let url = format!("{server_url}/api/v1/sessions");
    let response = client.post(&url).json(&request).send().await?;

    if response.status().is_success() {
        let result: CreateSessionResponse = response.json().await?;
        Ok((result.session_id, tunable_node_ids))
    } else {
        let status = response.status();
        // Using unwrap_or_default is acceptable: error text is for logging only,
        // empty string is a reasonable fallback if response body can't be read
        let body = response.text().await.unwrap_or_default();
        Err(format!("Failed to create session: {status} - {body}").into())
    }
}

/// Create a broadcaster session WITHOUT rewriting broadcast names.
/// Broadcasters publish to per-run broadcast names so subscriber sessions can find them
/// without colliding with browsers or other tests.
pub async fn create_broadcaster_session(
    pipeline_path: &str,
    session_name: &str,
    server_url: &str,
    run_id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use serde::{Deserialize, Serialize};
    use tokio::fs;

    #[derive(Serialize)]
    struct CreateSessionRequest {
        yaml: String,
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct CreateSessionResponse {
        session_id: String,
    }

    let yaml_raw = fs::read_to_string(pipeline_path).await?;
    let mut yaml: serde_json::Value = serde_saphyr::from_str(&yaml_raw)?;
    rewrite_broadcaster_yaml_for_load_test(&mut yaml, run_id);
    let yaml = serde_saphyr::to_string(&yaml)?;

    let request = CreateSessionRequest { yaml, name: Some(session_name.to_string()) };

    let client = reqwest::Client::new();
    let url = format!("{server_url}/api/v1/sessions");
    let response = client.post(&url).json(&request).send().await?;

    if response.status().is_success() {
        let result: CreateSessionResponse = response.json().await?;
        Ok(result.session_id)
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(format!("Failed to create broadcaster session: {status} - {body}").into())
    }
}

pub async fn session_creator_worker(
    config: LoadTestConfig,
    metrics: MetricsCollector,
    session_tx: mpsc::Sender<DynamicSession>,
    shutdown: tokio_util::sync::CancellationToken,
    tracking_tx: Option<mpsc::Sender<String>>,
    run_id: String,
) {
    debug!("Session creator worker started");

    if config.dynamic.pipelines.is_empty() {
        warn!("No pipelines configured for dynamic scenario; session creator worker exiting");
        return;
    }

    let mut created_count = 0;
    let target_count = config.dynamic.session_count;
    let mut last_error: Option<String> = None;
    let mut last_error_repeats: usize = 0;
    let mut total_failures: usize = 0;

    while created_count < target_count {
        tokio::select! {
            () = shutdown.cancelled() => {
                break;
            }
            () = async {
                // Select random pipeline
                // Cast acceptable: selecting from a list of pipelines, usize fits in practical ranges
                #[allow(clippy::cast_possible_truncation)]
                let pipeline_idx = rand::rng().random_range(0..config.dynamic.pipelines.len());
                let pipeline_path = &config.dynamic.pipelines[pipeline_idx];

                let name_suffix = rand::rng()
                    .sample_iter(&Alphanumeric)
                    .take(6)
                    .map(char::from)
                    .collect::<String>()
                    .to_lowercase();
                let session_name = format!("LoadTest-{}-{name_suffix}", created_count + 1);

                let start = Instant::now();
                let result = create_session_with_pipeline(
                    pipeline_path,
                    &session_name,
                    &config.server.url,
                    &run_id,
                ).await;

                let latency = start.elapsed();

                match result {
                    Ok((session_id, tunable_node_ids)) => {
                        debug!("Created session: {} ({})", session_name, session_id);

                        metrics
                            .record(OperationResult {
                                op_type: OperationType::SessionCreate,
                                latency,
                                success: true,
                                error: None,
                            })
                            .await;

                        // Track session ID for cleanup
                        if let Some(ref tx) = tracking_tx {
                            let _ = tx.send(session_id.clone()).await;
                        }

                        let _ = session_tx
                            .send(DynamicSession {
                                session_id,
                                pipeline_path: pipeline_path.clone(),
                                tunable_node_ids,
                            })
                            .await;

                        created_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to create session {}: {}", session_name, e);
                        total_failures += 1;

                        let err_str = e.to_string();
                        if last_error.as_deref() == Some(err_str.as_str()) {
                            last_error_repeats += 1;
                        } else {
                            last_error = Some(err_str.clone());
                            last_error_repeats = 1;
                        }

                        metrics
                            .record(OperationResult {
                                op_type: OperationType::SessionCreate,
                                latency,
                                success: false,
                                error: Some(err_str.clone()),
                            })
                            .await;

                        // If we can't create even the first session due to a repeated deterministic error,
                        // fail fast by cancelling the whole load test so the operator sees the issue quickly.
                        if created_count == 0 && (last_error_repeats >= 5 || total_failures >= 20) {
                            warn!(
                                "Session creation repeatedly failing before any sessions were created ({}x): {}. Cancelling load test.",
                                last_error_repeats,
                                err_str
                            );
                            shutdown.cancel();
                            return;
                        }

                        // Back off on failure to avoid spinning
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            } => {}
        }
    }

    debug!("Session creator worker stopped (created {} sessions)", created_count);
}

#[allow(clippy::cognitive_complexity)]
pub async fn session_tuner_worker(
    config: LoadTestConfig,
    metrics: MetricsCollector,
    mut session_rx: mpsc::Receiver<DynamicSession>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    debug!("Session tuner worker started");

    let mut sessions = Vec::new();
    let mut control_ws = ControlWs::connect(&config.server.url).await.ok();

    // Collect sessions as they're created
    loop {
        tokio::select! {
            Some(session) = session_rx.recv() => {
                sessions.push(session);
            }
            () = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                if !sessions.is_empty() {
                    break;
                }
            }
        }
    }

    debug!("Tuner worker has {} sessions to tune", sessions.len());

    let tune_interval = tokio::time::Duration::from_millis(config.dynamic.tune_interval_ms);
    let mut interval = tokio::time::interval(tune_interval);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if sessions.is_empty() {
                    continue;
                }

                // Pick random session
                // Cast acceptable: selecting from a list of sessions in a load test
                #[allow(clippy::cast_possible_truncation)]
                let idx = rand::rng().random_range(0..sessions.len());
                let session = &sessions[idx];

                let start = Instant::now();
                let result = {
                    if control_ws.is_none() {
                        control_ws = ControlWs::connect(&config.server.url).await.ok();
                    }
                    match control_ws.as_mut() {
                        Some(ws) => {
                            if session.tunable_node_ids.is_empty() {
                                Err("No tunable nodes found for this session".into())
                            } else {
                                // Pick random tunable gain node ID for this session.
                                // Cast acceptable: selecting from a small list of tunable nodes.
                                #[allow(clippy::cast_possible_truncation)]
                                let node_idx =
                                    rand::rng().random_range(0..session.tunable_node_ids.len());
                                let node_id = &session.tunable_node_ids[node_idx];

                                // Generate random gain value between 0.5 and 2.0
                                let gain_value = rand::rng().random_range(0.5..2.0);

                                ws.tune_node(
                                    &session.session_id,
                                    node_id,
                                    "gain",
                                    &gain_value.to_string(),
                                )
                                .await
                            }
                        },
                        None => Err("Failed to connect to control WebSocket".into()),
                    }
                };

                let latency = start.elapsed();

                let (success, error) = match result {
                    Ok(()) => (true, None),
                    Err(e) => {
                        // Don't warn on every error - node might not exist
                        debug!("Tune error on session {}: {}", session.session_id, e);
                        control_ws = None;
                        (false, Some(e.to_string()))
                    }
                };

                metrics
                    .record(OperationResult {
                        op_type: OperationType::NodeTune,
                        latency,
                        success,
                        error,
                    })
                    .await;
            }
            Some(new_session) = session_rx.recv() => {
                sessions.push(new_session);
            }
            () = shutdown.cancelled() => {
                break;
            }
        }
    }

    debug!("Session tuner worker stopped");
}

pub async fn cleanup_sessions(
    session_ids: Vec<String>,
    server_url: &str,
    metrics: MetricsCollector,
) {
    debug!("Cleaning up {} sessions", session_ids.len());

    let mut control_ws = ControlWs::connect(server_url).await.ok();

    for session_id in session_ids {
        let start = Instant::now();
        let result = {
            if control_ws.is_none() {
                control_ws = ControlWs::connect(server_url).await.ok();
            }
            match control_ws.as_mut() {
                Some(ws) => ws.destroy_session(&session_id).await,
                None => Err("Failed to connect to control WebSocket".into()),
            }
        };
        let latency = start.elapsed();

        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => {
                warn!("Failed to destroy session {}: {}", session_id, e);
                (false, Some(e.to_string()))
            },
        };

        metrics
            .record(OperationResult {
                op_type: OperationType::SessionDestroy,
                latency,
                success,
                error,
            })
            .await;
    }

    debug!("Session cleanup complete");
}

async fn recv_response_ignoring_events(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_correlation_id: &str,
) -> Result<streamkit_api::Response, Box<dyn std::error::Error + Send + Sync>> {
    use futures::StreamExt as FuturesStreamExt;
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

    loop {
        match ws_stream.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let v: Value = serde_json::from_str(&text)?;
                if v.get("type").and_then(|t| t.as_str()) == Some("response") {
                    let response: streamkit_api::Response = serde_json::from_str(&text)?;
                    if let Some(cid) = &response.correlation_id {
                        if cid == expected_correlation_id {
                            return Ok(response);
                        }
                    } else {
                        return Ok(response);
                    }
                }
            },
            Some(Ok(_)) => {},
            Some(Err(e)) => return Err(e.into()),
            None => return Err("WebSocket closed before receiving response".into()),
        }
    }
}

struct ControlWs {
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl ControlWs {
    async fn connect(server_url: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        use tokio_tungstenite::connect_async;
        use url::Url;

        let mut ws_url = Url::parse(server_url)?;
        #[allow(clippy::unwrap_used)]
        match ws_url.scheme() {
            "http" => ws_url.set_scheme("ws").unwrap(),
            "https" => ws_url.set_scheme("wss").unwrap(),
            "ws" | "wss" => (),
            _ => return Err("Server URL must be http(s) or ws(s)".into()),
        }
        ws_url.set_path("/api/v1/control");
        let ws_url_str = ws_url.to_string();

        let (ws_stream, _) = connect_async(ws_url_str).await?;
        Ok(Self { ws_stream })
    }

    async fn send_request(
        &mut self,
        payload: streamkit_api::RequestPayload,
    ) -> Result<streamkit_api::ResponsePayload, Box<dyn std::error::Error + Send + Sync>> {
        use futures_util::SinkExt;
        use streamkit_api::{MessageType, Request, ResponsePayload};
        use tokio_tungstenite::tungstenite::protocol::Message;

        let req = Request {
            message_type: MessageType::Request,
            correlation_id: Some(uuid::Uuid::new_v4().to_string()),
            payload,
        };

        let req_json = serde_json::to_string(&req)?;
        self.ws_stream.send(Message::Text(req_json.into())).await?;

        #[allow(clippy::unwrap_used)]
        let correlation_id = req.correlation_id.clone().unwrap();
        let response = recv_response_ignoring_events(&mut self.ws_stream, &correlation_id).await?;
        match response.payload {
            ResponsePayload::Error { message } => Err(message.into()),
            other => Ok(other),
        }
    }

    async fn destroy_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use streamkit_api::{RequestPayload, ResponsePayload};

        match self
            .send_request(RequestPayload::DestroySession { session_id: session_id.to_string() })
            .await?
        {
            ResponsePayload::SessionDestroyed { .. } | ResponsePayload::Success => Ok(()),
            _ => Err("Unexpected response from server".into()),
        }
    }

    async fn tune_node(
        &mut self,
        session_id: &str,
        node_id: &str,
        param: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use streamkit_api::{NodeControlMessage, RequestPayload, ResponsePayload};

        let param_value: serde_json::Value = serde_saphyr::from_str(value)?;
        let mut params = serde_json::Map::new();
        params.insert(param.to_string(), param_value);
        let update_params = serde_json::Value::Object(params);

        match self
            .send_request(RequestPayload::TuneNode {
                session_id: session_id.to_string(),
                node_id: node_id.to_string(),
                message: NodeControlMessage::UpdateParams(update_params),
            })
            .await?
        {
            ResponsePayload::Success => Ok(()),
            _ => Err("Unexpected response from server".into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn get_node_broadcast<'a>(yaml: &'a serde_json::Value, node_name: &str) -> &'a str {
        let Some(nodes) = yaml.get("nodes").and_then(|v| v.as_object()) else {
            panic!("missing nodes mapping");
        };
        let Some(node) = nodes.get(node_name) else {
            panic!("missing node {node_name}");
        };
        let Some(params) = node.get("params") else {
            panic!("missing params for {node_name}");
        };
        let Some(broadcast) = params.get("broadcast").and_then(|v| v.as_str()) else {
            panic!("missing broadcast for {node_name}");
        };
        broadcast
    }

    #[test]
    fn load_test_rewrite_prefixes_broadcasts_with_lt() {
        let yaml_str = r"
nodes:
  moq_sub:
    kind: transport::moq::subscriber
    params:
      broadcast: output
  moq_pub:
    kind: transport::moq::publisher
    params:
      broadcast: output
";
        let Ok(mut yaml) = serde_saphyr::from_str::<serde_json::Value>(yaml_str) else {
            panic!("failed to parse yaml");
        };

        rewrite_yaml_for_load_test(&mut yaml, "run123", "sess456");

        let sub = get_node_broadcast(&yaml, "moq_sub");
        let pub_ = get_node_broadcast(&yaml, "moq_pub");

        // Internal broadcasts are rewritten per-session.
        assert_eq!(sub, "lt/output-sess456");
        assert_eq!(pub_, "lt/output-sess456");
    }

    #[test]
    fn load_test_rewrite_prefixes_external_subscriber_with_lt() {
        let yaml_str = r"
nodes:
  moq_sub:
    kind: transport::moq::subscriber
    params:
      broadcast: output
";
        let Ok(mut yaml) = serde_saphyr::from_str::<serde_json::Value>(yaml_str) else {
            panic!("failed to parse yaml");
        };

        rewrite_yaml_for_load_test(&mut yaml, "run123", "sess456");

        let sub = get_node_broadcast(&yaml, "moq_sub");
        assert_eq!(sub, "lt/output-run123");
    }
}
