// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use futures::StreamExt as FuturesStreamExt;
use futures_util::SinkExt;
use reqwest::multipart;
use std::path::Path;
use streamkit_api::{
    AudioAsset, BatchOperation, MessageType, PermissionsInfo, Request, RequestPayload, Response,
    ResponsePayload, SamplePipeline, SavePipelineRequest,
};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info};
use url::Url;

fn http_base_url(server_url: &str) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
    let mut url = Url::parse(server_url)?;
    match url.scheme() {
        "http" | "https" => {},
        "ws" => {
            url.set_scheme("http")
                .map_err(|()| "Failed to convert ws:// to http:// for server URL")?;
        },
        "wss" => {
            url.set_scheme("https")
                .map_err(|()| "Failed to convert wss:// to https:// for server URL")?;
        },
        _ => return Err("Server URL must be http(s) or ws(s)".into()),
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn control_ws_url(server_url: &str) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
    let mut ws_url = Url::parse(server_url)?;
    match ws_url.scheme() {
        "http" => ws_url
            .set_scheme("ws")
            .map_err(|()| "Failed to convert http:// to ws:// for server URL")?,
        "https" => ws_url
            .set_scheme("wss")
            .map_err(|()| "Failed to convert https:// to wss:// for server URL")?,
        "ws" | "wss" => {},
        _ => return Err("Server URL must be http(s) or ws(s)".into()),
    }
    ws_url.set_path("/api/v1/control");
    ws_url.set_query(None);
    ws_url.set_fragment(None);
    Ok(ws_url)
}

async fn recv_response_ignoring_events(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_correlation_id: &str,
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

    loop {
        match ws_stream.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let v: Value = serde_json::from_str(&text)?;
                if v.get("type").and_then(|t| t.as_str()) == Some("response") {
                    let response: Response = serde_json::from_str(&text)?;
                    if let Some(cid) = &response.correlation_id {
                        if cid == expected_correlation_id {
                            return Ok(response);
                        }
                        // Ignore responses for other requests - continue to next iteration
                    } else {
                        // No correlation id - accept it
                        return Ok(response);
                    }
                }
                // Ignore events and unknown message types - continue to next iteration
            },
            Some(Ok(_)) => {
                // Non-text message; ignore and continue
            },
            Some(Err(e)) => return Err(e.into()),
            None => return Err("WebSocket closed before receiving response".into()),
        }
    }
}

async fn ws_request(
    server_url: &str,
    payload: RequestPayload,
) -> Result<ResponsePayload, Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = control_ws_url(server_url)?.to_string();
    let (mut ws_stream, _) = connect_async(ws_url).await?;

    let req = Request {
        message_type: MessageType::Request,
        correlation_id: Some(uuid::Uuid::new_v4().to_string()),
        payload,
    };
    let req_json = serde_json::to_string(&req)?;
    ws_stream.send(Message::Text(req_json.into())).await?;

    #[allow(clippy::unwrap_used)] // correlation_id is always Some() as set above
    let correlation_id = req.correlation_id.clone().unwrap();
    let response = recv_response_ignoring_events(&mut ws_stream, &correlation_id).await?;
    ws_stream.close(None).await?;

    match response.payload {
        ResponsePayload::Error { message } => Err(message.into()),
        other => Ok(other),
    }
}

async fn ws_send_fire_and_forget(
    server_url: &str,
    payload: RequestPayload,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = control_ws_url(server_url)?.to_string();
    let (mut ws_stream, _) = connect_async(ws_url).await?;

    let req = Request {
        message_type: MessageType::Request,
        correlation_id: Some(uuid::Uuid::new_v4().to_string()),
        payload,
    };
    let req_json = serde_json::to_string(&req)?;
    ws_stream.send(Message::Text(req_json.into())).await?;
    ws_stream.close(None).await?;
    Ok(())
}

fn parse_object_params(
    s: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        return Ok(v);
    }

    let json_value: serde_json::Value = serde_saphyr::from_str(s)?;
    Ok(json_value)
}

fn parse_batch_operations(
    s: &str,
) -> Result<Vec<BatchOperation>, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(v) = serde_json::from_str::<Vec<BatchOperation>>(s) {
        return Ok(v);
    }
    Ok(serde_saphyr::from_str::<Vec<BatchOperation>>(s)?)
}

/// Process a pipeline using a remote server in oneshot mode
///
/// # Errors
///
/// Returns an error if:
/// - Pipeline or input files do not exist
/// - Failed to read input files
/// - Server returns a non-success status
/// - Network communication fails
/// - Failed to write output file
#[allow(clippy::cognitive_complexity)]
pub async fn process_oneshot(
    pipeline_path: &str,
    input_path: &str,
    output_path: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    process_oneshot_with_client(&client, pipeline_path, input_path, output_path, server_url).await
}

/// Process a pipeline using a remote server in oneshot mode with a caller-provided HTTP client.
///
/// This enables connection pooling and reduces per-request overhead when invoking repeatedly
/// (e.g. in a load test).
///
/// # Errors
///
/// Returns an error if:
/// - Pipeline or input files do not exist
/// - Failed to read input files
/// - Server returns a non-success status
/// - Network communication fails
/// - Failed to write output file
#[allow(clippy::cognitive_complexity)]
pub async fn process_oneshot_with_client(
    client: &reqwest::Client,
    pipeline_path: &str,
    input_path: &str,
    output_path: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        pipeline = %pipeline_path,
        input = %input_path,
        output = %output_path,
        server = %server_url,
        "Starting oneshot pipeline processing"
    );

    // Validate input files exist
    if !Path::new(pipeline_path).exists() {
        return Err(format!("Pipeline file not found: {pipeline_path}").into());
    }
    if !Path::new(input_path).exists() {
        return Err(format!("Input file not found: {input_path}").into());
    }

    // Read pipeline configuration
    debug!("Reading pipeline configuration from {pipeline_path}");
    let pipeline_content = fs::read_to_string(pipeline_path).await?;

    // Read input media file
    debug!("Reading input media file from {input_path}");
    let media_data = fs::read(input_path).await?;

    // Extract filename for the multipart form
    let input_filename = Path::new(input_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("input")
        .to_string();

    // Create multipart form
    let media_len = media_data.len();
    debug!("Creating multipart form with {media_len} bytes of media data");
    let form = multipart::Form::new()
        .text("config", pipeline_content)
        .part("media", multipart::Part::bytes(media_data).file_name(input_filename));

    // Send request to server
    let url = http_base_url(server_url)?.join("/api/v1/process")?;

    info!("Sending request to {url}");
    let response = client.post(url).multipart(form).send().await?;

    // Check response status
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {error_text}").into());
    }

    // Get content type for logging
    let content_type =
        response.headers().get("content-type").and_then(|ct| ct.to_str().ok()).unwrap_or("unknown");

    info!("Received response with content-type: {content_type}");

    // Stream response to output file
    debug!("Writing response to {output_path}");
    let mut file = tokio::fs::File::create(output_path).await?;
    let mut stream = response.bytes_stream();

    let mut total_bytes = 0;
    while let Some(chunk) = FuturesStreamExt::next(&mut stream).await {
        let chunk = chunk?;
        total_bytes += chunk.len();
        file.write_all(&chunk).await?;
    }

    file.flush().await?;

    info!(
        output_file = %output_path,
        bytes_written = total_bytes,
        "Pipeline processing completed successfully"
    );

    Ok(())
}

/// Create a new dynamic session with a pipeline configuration using HTTP POST.
///
/// This atomically creates the session and deploys the entire pipeline, preventing
/// race conditions where source nodes start producing packets before connections are established.
///
/// # Errors
///
/// Returns an error if:
/// - Pipeline file does not exist or cannot be read
/// - Server returns a non-success status
/// - Network communication fails
/// - Invalid response format from server
#[allow(clippy::cognitive_complexity)]
pub async fn create_session(
    pipeline_path: &str,
    name: &Option<String>,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[derive(serde::Serialize)]
    struct CreateSessionRequest {
        name: Option<String>,
        yaml: String,
    }

    #[derive(serde::Deserialize)]
    struct CreateSessionResponse {
        session_id: String,
        name: Option<String>,
        created_at: String,
    }

    info!(
        pipeline = %pipeline_path,
        server = %server_url,
        "Creating dynamic session via HTTP POST"
    );

    // Read pipeline YAML content
    let pipeline_content = fs::read_to_string(pipeline_path).await?;

    // Prepare HTTP request body
    let request_body = CreateSessionRequest { name: name.clone(), yaml: pipeline_content };

    // Send HTTP POST request
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/sessions")?;

    info!("Sending HTTP POST request to {url}");
    let response = client.post(url).json(&request_body).send().await?;

    // Check response status
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        error!("Failed to create session: {status} - {error_text}");
        return Err(format!("Server returned error {status}: {error_text}").into());
    }

    // Parse response
    let result: CreateSessionResponse = response.json().await?;
    let session_id = result.session_id;
    let session_name = result.name;
    let created_at = result.created_at;

    info!("Created session: {session_id} (name: {session_name:?}) at {created_at}");

    // Print the session info prominently so users can capture it
    println!("✅ Session created successfully!");
    if let Some(ref name) = session_name {
        println!("📋 Session Name: {name}");
        println!("🆔 Session ID: {session_id}");
        println!("💡 Use these commands to manage the session:");
        println!("   skit-cli tune {name} <node> <param> <value>");
        println!("   skit-cli destroy {name}");
    } else {
        println!("🆔 Session ID: {session_id}");
        println!("💡 Use these commands to manage the session:");
        println!("   skit-cli tune {session_id} <node> <param> <value>");
        println!("   skit-cli destroy {session_id}");
    }

    info!("Session created successfully. ID: {session_id}, Name: {session_name:?}");
    Ok(())
}

/// Destroy a dynamic session and cleanup its resources
///
/// # Errors
///
/// Returns an error if:
/// - Server URL is invalid
/// - Failed to establish WebSocket connection
/// - Server returns an error response
/// - Network communication fails
///
/// # Panics
///
/// May panic if URL scheme conversion fails (extremely unlikely with valid http/https URLs)
#[allow(clippy::cognitive_complexity)]
pub async fn destroy_session(
    session_id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        session_id = %session_id,
        server = %server_url,
        "Destroying dynamic session"
    );

    match ws_request(
        server_url,
        RequestPayload::DestroySession { session_id: session_id.to_string() },
    )
    .await?
    {
        ResponsePayload::SessionDestroyed { session_id: destroyed_id } => {
            info!("Successfully destroyed session: {destroyed_id}");
        },
        ResponsePayload::Success => {},
        other => return Err(format!("Unexpected response from server: {other:?}").into()),
    }

    info!("Session '{session_id}' destroyed successfully");
    Ok(())
}

/// Tune a node's parameters in a dynamic session
///
/// # Errors
///
/// Returns an error if:
/// - Server URL is invalid
/// - Parameter value is not valid YAML
/// - Failed to establish WebSocket connection
/// - Server returns an error response
/// - Network communication fails
///
/// # Panics
///
/// May panic if URL scheme conversion fails (extremely unlikely with valid http/https URLs)
#[allow(clippy::cognitive_complexity)]
pub async fn tune_node(
    session_id: &str,
    node_id: &str,
    param: &str,
    value: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        session_id = %session_id,
        node_id = %node_id,
        param = %param,
        value = %value,
        server = %server_url,
        "Tuning node parameter"
    );

    // Parse the parameter value as YAML
    let param_value: serde_json::Value = serde_saphyr::from_str(value)?;

    // Create a JSON object with the single parameter
    let mut params = serde_json::Map::new();
    params.insert(param.to_string(), param_value);
    let update_params = serde_json::Value::Object(params);

    match ws_request(
        server_url,
        RequestPayload::TuneNode {
            session_id: session_id.to_string(),
            node_id: node_id.to_string(),
            message: streamkit_api::NodeControlMessage::UpdateParams(update_params),
        },
    )
    .await?
    {
        ResponsePayload::Success => {
            info!("Successfully tuned node parameter");
            println!("✅ Node parameter updated successfully!");
            println!("📋 Session: {session_id}");
            println!("🎛️  Node: {node_id} -> {param}: {value}");
        },
        other => return Err(format!("Unexpected response from server: {other:?}").into()),
    }

    info!("Node tuning completed successfully");
    Ok(())
}

/// List all active dynamic sessions
///
/// # Errors
///
/// Returns an error if:
/// - Server URL is invalid
/// - Failed to establish WebSocket connection
/// - Server returns an error response
/// - Network communication fails
///
/// # Panics
///
/// May panic if URL scheme conversion fails (extremely unlikely with valid http/https URLs)
#[allow(clippy::cognitive_complexity)]
pub async fn list_sessions(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        server = %server_url,
        "Listing active sessions"
    );

    match ws_request(server_url, RequestPayload::ListSessions).await? {
        ResponsePayload::SessionsListed { sessions } => {
            let count = sessions.len();
            info!("Successfully retrieved {count} sessions");

            if sessions.is_empty() {
                println!("No active sessions found.");
            } else {
                println!("Active Sessions:");
                println!("{:<20} {:<36} STATUS", "NAME", "SESSION ID");
                println!("{}", "-".repeat(70));

                for session in sessions {
                    let name = session.name.as_deref().unwrap_or("<unnamed>");
                    println!("{:<20} {:<36} Running", name, session.id);
                }
            }
        },
        other => return Err(format!("Unexpected response from server: {other:?}").into()),
    }

    info!("Session listing completed successfully");
    Ok(())
}

/// List available node types via WebSocket (action: `listnodes`).
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket request fails, the server returns
/// an error response, or the output cannot be serialized.
pub async fn control_list_nodes(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match ws_request(server_url, RequestPayload::ListNodes).await? {
        ResponsePayload::NodesListed { nodes } => {
            println!("{}", serde_json::to_string_pretty(&nodes)?);
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Fetch a session pipeline via WebSocket (action: `getpipeline`).
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket request fails, the server returns
/// an error response, or the output cannot be serialized.
pub async fn control_get_pipeline(
    session_id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match ws_request(server_url, RequestPayload::GetPipeline { session_id: session_id.to_string() })
        .await?
    {
        ResponsePayload::Pipeline { pipeline } => {
            println!("{}", serde_json::to_string_pretty(&pipeline)?);
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Add a node to a session via WebSocket (action: `addnode`).
///
/// `params` may be JSON or YAML (object).
///
/// # Errors
///
/// Returns an error if `params` cannot be parsed, the server URL is invalid, the WebSocket request
/// fails, or the server returns an error response.
pub async fn control_add_node(
    session_id: &str,
    node_id: &str,
    kind: &str,
    params: Option<&str>,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let params = match params {
        Some(s) => Some(parse_object_params(s)?),
        None => None,
    };

    match ws_request(
        server_url,
        RequestPayload::AddNode {
            session_id: session_id.to_string(),
            node_id: node_id.to_string(),
            kind: kind.to_string(),
            params,
        },
    )
    .await?
    {
        ResponsePayload::Success => {
            println!("✅ Added node '{node_id}' ({kind}) to session '{session_id}'");
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Remove a node from a session via WebSocket (action: `removenode`).
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket request fails, or the server
/// returns an error response.
pub async fn control_remove_node(
    session_id: &str,
    node_id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match ws_request(
        server_url,
        RequestPayload::RemoveNode {
            session_id: session_id.to_string(),
            node_id: node_id.to_string(),
        },
    )
    .await?
    {
        ResponsePayload::Success => {
            println!("✅ Removed node '{node_id}' from session '{session_id}'");
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Connect two nodes in a session via WebSocket (action: `connect`).
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket request fails, or the server
/// returns an error response.
pub async fn control_connect(
    session_id: &str,
    from_node: &str,
    from_pin: &str,
    to_node: &str,
    to_pin: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match ws_request(
        server_url,
        RequestPayload::Connect {
            session_id: session_id.to_string(),
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: to_node.to_string(),
            to_pin: to_pin.to_string(),
            mode: streamkit_api::ConnectionMode::default(),
        },
    )
    .await?
    {
        ResponsePayload::Success => {
            println!("✅ Connected {from_node}.{from_pin} -> {to_node}.{to_pin} (session '{session_id}')");
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Disconnect two nodes in a session via WebSocket (action: `disconnect`).
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket request fails, or the server
/// returns an error response.
pub async fn control_disconnect(
    session_id: &str,
    from_node: &str,
    from_pin: &str,
    to_node: &str,
    to_pin: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match ws_request(
        server_url,
        RequestPayload::Disconnect {
            session_id: session_id.to_string(),
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: to_node.to_string(),
            to_pin: to_pin.to_string(),
        },
    )
    .await?
    {
        ResponsePayload::Success => {
            println!(
                "✅ Disconnected {from_node}.{from_pin} -> {to_node}.{to_pin} (session '{session_id}')"
            );
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Validate a batch of operations via WebSocket (action: `validatebatch`).
///
/// The file at `ops_file` must contain `BatchOperation[]` as JSON or YAML.
///
/// # Errors
///
/// Returns an error if the ops file cannot be read, cannot be parsed, the server URL is invalid,
/// the WebSocket request fails, or the server returns an error response.
pub async fn control_validate_batch(
    session_id: &str,
    ops_file: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = fs::read_to_string(ops_file).await?;
    let operations = parse_batch_operations(&content)?;

    match ws_request(
        server_url,
        RequestPayload::ValidateBatch { session_id: session_id.to_string(), operations },
    )
    .await?
    {
        ResponsePayload::ValidationResult { errors } => {
            println!("{}", serde_json::to_string_pretty(&errors)?);
            Ok(())
        },
        ResponsePayload::Success => {
            println!("✅ Batch validated (no errors)");
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Apply a batch of operations via WebSocket (action: `applybatch`).
///
/// The file at `ops_file` must contain `BatchOperation[]` as JSON or YAML.
///
/// # Errors
///
/// Returns an error if the ops file cannot be read, cannot be parsed, the server URL is invalid,
/// the WebSocket request fails, or the server returns an error response.
pub async fn control_apply_batch(
    session_id: &str,
    ops_file: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = fs::read_to_string(ops_file).await?;
    let operations = parse_batch_operations(&content)?;

    match ws_request(
        server_url,
        RequestPayload::ApplyBatch { session_id: session_id.to_string(), operations },
    )
    .await?
    {
        ResponsePayload::BatchApplied { success, errors } => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({ "success": success, "errors": errors })
                )?
            );
            Ok(())
        },
        ResponsePayload::Success => {
            println!("✅ Batch applied successfully");
            Ok(())
        },
        other => Err(format!("Unexpected response from server: {other:?}").into()),
    }
}

/// Fire-and-forget node tuning via WebSocket (action: `tunenodeasync`).
///
/// # Errors
///
/// Returns an error if the parameter value is not valid YAML, the server URL is invalid, or the
/// WebSocket request fails.
pub async fn control_tune_async(
    session_id: &str,
    node_id: &str,
    param: &str,
    value: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let param_value: serde_json::Value = serde_saphyr::from_str(value)?;
    let mut params = serde_json::Map::new();
    params.insert(param.to_string(), param_value);
    let update_params = serde_json::Value::Object(params);

    ws_send_fire_and_forget(
        server_url,
        RequestPayload::TuneNodeAsync {
            session_id: session_id.to_string(),
            node_id: node_id.to_string(),
            message: streamkit_api::NodeControlMessage::UpdateParams(update_params),
        },
    )
    .await?;

    println!("✅ Sent async tune for {node_id} ({param}={value}) in session '{session_id}'");
    Ok(())
}

#[derive(serde::Deserialize, serde::Serialize)]
struct FrontendConfig {
    #[serde(default)]
    moq_gateway_url: Option<String>,
}

/// Fetch UI bootstrap config (GET /api/v1/config)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn get_config(server_url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/config")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let config: FrontendConfig = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PermissionsResponse {
    role: String,
    permissions: PermissionsInfo,
}

/// Fetch permissions for the current request (GET /api/v1/permissions)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn get_permissions(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/permissions")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let perms: PermissionsResponse = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&perms)?);
    Ok(())
}

/// List node schemas (GET /api/v1/schema/nodes)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_node_schemas(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/schema/nodes")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let nodes: Vec<streamkit_api::NodeDefinition> = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(())
}

/// List packet schemas (GET /api/v1/schema/packets)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_packet_schemas(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/schema/packets")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }

    let packet_meta: serde_json::Value = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&packet_meta)?);
    Ok(())
}

/// Fetch a session's pipeline (GET /api/v1/sessions/{id}/pipeline)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn get_pipeline(
    session_id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url =
        http_base_url(server_url)?.join(&format!("/api/v1/sessions/{session_id}/pipeline"))?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let pipeline: streamkit_api::ApiPipeline = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&pipeline)?);
    Ok(())
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum PluginType {
    Wasm,
    Native,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct PluginSummary {
    kind: String,
    original_kind: String,
    file_name: String,
    categories: Vec<String>,
    loaded_at_ms: u128,
    plugin_type: PluginType,
}

/// List loaded plugins (GET /api/v1/plugins)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_plugins(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/plugins")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let plugins: Vec<PluginSummary> = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&plugins)?);
    Ok(())
}

/// Upload a plugin file (POST /api/v1/plugins, multipart field "plugin")
///
/// # Errors
///
/// Returns an error if the plugin file cannot be read, the server URL is invalid, the request
/// fails, the server returns a non-success status, or the response cannot be parsed.
pub async fn upload_plugin(
    path: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let base = http_base_url(server_url)?;
    let url = base.join("/api/v1/plugins")?;

    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(format!("Plugin file not found: {}", file_path.display()).into());
    }

    let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("plugin").to_string();
    let file_bytes = fs::read(file_path).await?;

    let part = multipart::Part::bytes(file_bytes).file_name(file_name);
    let form = multipart::Form::new().part("plugin", part);

    let response = client.post(url).multipart(form).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let summary: PluginSummary = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// Unload/delete a plugin (DELETE /api/v1/plugins/{kind}?keep_file=...)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn delete_plugin(
    kind: &str,
    keep_file: bool,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let mut url = http_base_url(server_url)?.join(&format!("/api/v1/plugins/{kind}"))?;
    if keep_file {
        url.query_pairs_mut().append_pair("keep_file", "true");
    }

    let response = client.delete(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let summary: PluginSummary = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// List oneshot sample pipelines (GET /api/v1/samples/oneshot)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_samples_oneshot(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/samples/oneshot")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let samples: Vec<SamplePipeline> = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&samples)?);
    Ok(())
}

/// List dynamic sample pipelines (GET /api/v1/samples/dynamic)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_samples_dynamic(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/samples/dynamic")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let samples: Vec<SamplePipeline> = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&samples)?);
    Ok(())
}

/// Fetch a sample pipeline by ID (GET /api/v1/samples/oneshot/{id})
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn get_sample(
    id: &str,
    yaml_only: bool,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join(&format!("/api/v1/samples/oneshot/{id}"))?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let sample: SamplePipeline = response.json().await?;
    if yaml_only {
        print!("{}", sample.yaml);
    } else {
        println!("{}", serde_json::to_string_pretty(&sample)?);
    }
    Ok(())
}

/// Save a sample pipeline (POST /api/v1/samples/oneshot)
///
/// # Errors
///
/// Returns an error if the YAML file cannot be read, the server URL is invalid, the request fails,
/// the server returns a non-success status, or the response cannot be parsed.
pub async fn save_sample(
    name: &str,
    description: &str,
    yaml_path: &str,
    overwrite: bool,
    fragment: bool,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let yaml = fs::read_to_string(yaml_path).await?;
    let req = SavePipelineRequest {
        name: name.to_string(),
        description: description.to_string(),
        yaml,
        overwrite,
        is_fragment: fragment,
    };

    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/samples/oneshot")?;
    let response = client.post(url).json(&req).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let sample: SamplePipeline = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&sample)?);
    Ok(())
}

/// Delete a sample pipeline by ID (DELETE /api/v1/samples/oneshot/{id})
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, or the server returns a
/// non-success status.
pub async fn delete_sample(
    id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join(&format!("/api/v1/samples/oneshot/{id}"))?;
    let response = client.delete(url).send().await?;
    if response.status().is_success() {
        println!("✅ Deleted sample: {id}");
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(format!("Server returned error {status}: {body}").into())
}

/// List audio assets (GET /api/v1/assets/audio)
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, the server returns a
/// non-success status, or the response cannot be parsed.
pub async fn list_audio_assets(
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/assets/audio")?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let assets: Vec<AudioAsset> = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&assets)?);
    Ok(())
}

/// Upload an audio asset (POST /api/v1/assets/audio)
///
/// # Errors
///
/// Returns an error if the audio file cannot be read, the server URL is invalid, the request
/// fails, the server returns a non-success status, or the response cannot be parsed.
pub async fn upload_audio_asset(
    path: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join("/api/v1/assets/audio")?;

    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(format!("Audio file not found: {}", file_path.display()).into());
    }

    let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("audio").to_string();
    let file_bytes = fs::read(file_path).await?;

    let part = multipart::Part::bytes(file_bytes).file_name(file_name);
    let form = multipart::Form::new().part("file", part);

    let response = client.post(url).multipart(form).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server returned error {status}: {body}").into());
    }
    let asset: AudioAsset = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&asset)?);
    Ok(())
}

/// Delete an audio asset (DELETE /api/v1/assets/audio/{id})
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the request fails, or the server returns a
/// non-success status.
pub async fn delete_audio_asset(
    id: &str,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = http_base_url(server_url)?.join(&format!("/api/v1/assets/audio/{id}"))?;
    let response = client.delete(url).send().await?;
    if response.status().is_success() {
        println!("✅ Deleted audio asset: {id}");
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(format!("Server returned error {status}: {body}").into())
}

/// Watch WebSocket events and print them as JSON.
///
/// If `session_filter` is provided, only events with matching `payload.session_id` are printed.
///
/// # Errors
///
/// Returns an error if the server URL is invalid, the WebSocket connection fails, or event output
/// cannot be serialized.
pub async fn watch_events(
    session_filter: Option<&str>,
    pretty: bool,
    server_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = control_ws_url(server_url)?.to_string();
    let (mut ws_stream, _) = connect_async(ws_url).await?;

    eprintln!("Watching events (Ctrl-C to stop)...");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
            msg = ws_stream.next() => {
                let Some(msg) = msg else {
                    break;
                };
                let msg = msg?;
                let Message::Text(text) = msg else {
                    continue;
                };

                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if v.get("type").and_then(|t| t.as_str()) != Some("event") {
                    continue;
                }

                if let Some(filter) = session_filter {
                    let sid = v
                        .get("payload")
                        .and_then(|p| p.get("session_id"))
                        .and_then(|s| s.as_str());
                    if sid != Some(filter) {
                        continue;
                    }
                }

                if pretty {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!("{text}");
                }
            }
        }
    }

    ws_stream.close(None).await?;
    Ok(())
}
