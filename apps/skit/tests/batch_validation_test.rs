// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use streamkit_api::{
    BatchOperation, MessageType, Request, RequestPayload, Response, ResponsePayload,
};
use streamkit_server::Config;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

// Type aliases to reduce verbosity of the fully-expanded WebSocket stream types.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsWriter = futures_util::stream::SplitSink<WsStream, WsMessage>;
type WsReader = futures_util::stream::SplitStream<WsStream>;

/// Helper to read messages from WebSocket, skipping events until we get a response with matching correlation_id
async fn read_response(read: &mut WsReader, expected_correlation_id: &str) -> Response {
    loop {
        let message = timeout(Duration::from_secs(5), read.next())
            .await
            .expect("Timeout waiting for response")
            .expect("No message received")
            .expect("Failed to read message");

        let text = message.to_text().expect("Expected text message");

        let value: serde_json::Value = serde_json::from_str(text).expect("Failed to parse message");
        let msg_type = value.get("type").and_then(|v| v.as_str());

        if msg_type == Some("event") {
            continue;
        }

        let response: Response = serde_json::from_str(text).expect("Failed to parse response");

        if response.correlation_id.as_deref() == Some(expected_correlation_id) {
            return response;
        }
    }
}

async fn start_test_server() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    start_test_server_with_config(Config::default()).await
}

async fn start_test_server_with_config(
    config: Config,
) -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        let (app, _state) = streamkit_server::server::create_app(config, None);
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    Some((addr, server_handle))
}

/// Helper: connect to WS, create a session, and return (write, read, session_id).
async fn setup_session(addr: SocketAddr) -> (WsWriter, WsReader, String) {
    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect to WebSocket");
    let (mut write, mut read) = ws_stream.split();

    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("setup-create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("batch-test".to_string()) },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "setup-create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        other => panic!("Expected SessionCreated, got: {:?}", other),
    };

    (write, read, session_id)
}

/// Helper: send a ValidateBatch request and return the response payload.
async fn send_validate_batch(
    write: &mut WsWriter,
    read: &mut WsReader,
    session_id: &str,
    operations: Vec<BatchOperation>,
    correlation_id: &str,
) -> ResponsePayload {
    let request = Request {
        message_type: MessageType::Request,
        correlation_id: Some(correlation_id.to_string()),
        payload: RequestPayload::ValidateBatch { session_id: session_id.to_string(), operations },
    };

    write.send(WsMessage::Text(serde_json::to_string(&request).unwrap().into())).await.unwrap();

    read_response(read, correlation_id).await.payload
}

/// Helper: send an ApplyBatch request and return the response payload.
async fn send_apply_batch(
    write: &mut WsWriter,
    read: &mut WsReader,
    session_id: &str,
    operations: Vec<BatchOperation>,
    correlation_id: &str,
) -> ResponsePayload {
    let request = Request {
        message_type: MessageType::Request,
        correlation_id: Some(correlation_id.to_string()),
        payload: RequestPayload::ApplyBatch { session_id: session_id.to_string(), operations },
    };

    write.send(WsMessage::Text(serde_json::to_string(&request).unwrap().into())).await.unwrap();

    read_response(read, correlation_id).await.payload
}

/// Build a Config whose default role has an empty plugin allowlist, so
/// `plugin::*` nodes are rejected.
fn config_with_no_plugins_allowed() -> Config {
    use streamkit_server::{Permissions, PermissionsConfig};

    let mut restricted = Permissions::admin();
    restricted.allowed_plugins = Vec::new(); // deny all plugins

    let mut roles = HashMap::new();
    roles.insert("admin".to_string(), restricted);

    Config {
        permissions: PermissionsConfig { roles, ..PermissionsConfig::default() },
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------
// ValidateBatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_validate_batch_rejects_http_input_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "http_in".to_string(),
            kind: "streamkit::http_input".to_string(),
            params: None,
        }],
        "validate-http-input",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {message}"
            );
        },
        other => panic!("Expected Error for http_input, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_http_output_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "http_out".to_string(),
            kind: "streamkit::http_output".to_string(),
            params: None,
        }],
        "validate-http-output",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {message}"
            );
        },
        other => panic!("Expected Error for http_output, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_disallowed_plugin() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) =
        start_test_server_with_config(config_with_no_plugins_allowed()).await
    else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "p1".to_string(),
            kind: "plugin::native::whisper".to_string(),
            params: None,
        }],
        "validate-disallowed-plugin",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("plugin") && message.contains("not allowed"),
                "Expected plugin not-allowed error, got: {message}"
            );
        },
        other => panic!("Expected Error for disallowed plugin, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_allows_valid_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: Some(json!({"gain": 2.0})),
        }],
        "validate-valid-node",
    )
    .await;

    match payload {
        ResponsePayload::ValidationResult { errors } => {
            assert!(errors.is_empty(), "Expected no validation errors for valid node");
        },
        other => panic!("Expected ValidationResult for valid node, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_nonexistent_session() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect to WebSocket");
    let (mut write, mut read) = ws_stream.split();

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        "nonexistent-session-id",
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: None,
        }],
        "validate-no-session",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("not found"),
                "Expected session not-found error, got: {message}"
            );
        },
        other => panic!("Expected Error for nonexistent session, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_mixed_with_oneshot_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![
            BatchOperation::AddNode {
                node_id: "gain1".to_string(),
                kind: "audio::gain".to_string(),
                params: Some(json!({"gain": 1.0})),
            },
            BatchOperation::AddNode {
                node_id: "http_in".to_string(),
                kind: "streamkit::http_input".to_string(),
                params: None,
            },
        ],
        "validate-mixed",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error in mixed batch, got: {message}"
            );
        },
        other => panic!("Expected Error for mixed batch with oneshot node, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// ApplyBatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_apply_batch_rejects_http_input_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "http_in".to_string(),
            kind: "streamkit::http_input".to_string(),
            params: None,
        }],
        "apply-http-input",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {message}"
            );
        },
        other => panic!("Expected Error for http_input in apply, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_batch_rejects_http_output_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "http_out".to_string(),
            kind: "streamkit::http_output".to_string(),
            params: None,
        }],
        "apply-http-output",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {message}"
            );
        },
        other => panic!("Expected Error for http_output in apply, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_batch_rejects_disallowed_plugin() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) =
        start_test_server_with_config(config_with_no_plugins_allowed()).await
    else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "p1".to_string(),
            kind: "plugin::native::whisper".to_string(),
            params: None,
        }],
        "apply-disallowed-plugin",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("plugin") && message.contains("not allowed"),
                "Expected plugin not-allowed error, got: {message}"
            );
        },
        other => panic!("Expected Error for disallowed plugin in apply, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_batch_allows_valid_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: Some(json!({"gain": 2.0})),
        }],
        "apply-valid-node",
    )
    .await;

    match payload {
        ResponsePayload::BatchApplied { success, errors } => {
            assert!(success, "Expected batch apply to succeed");
            assert!(errors.is_empty(), "Expected no errors from batch apply");
        },
        ResponsePayload::Error { message } => {
            panic!("Unexpected error for valid node in apply: {message}");
        },
        other => panic!("Expected BatchApplied for valid node, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_batch_rejects_mixed_with_oneshot_node() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![
            BatchOperation::AddNode {
                node_id: "gain1".to_string(),
                kind: "audio::gain".to_string(),
                params: Some(json!({"gain": 1.0})),
            },
            BatchOperation::AddNode {
                node_id: "http_in".to_string(),
                kind: "streamkit::http_input".to_string(),
                params: None,
            },
        ],
        "apply-mixed",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("oneshot-only"),
                "Expected oneshot-only error in mixed batch, got: {message}"
            );
        },
        other => {
            panic!("Expected Error for mixed batch with oneshot node in apply, got: {:?}", other)
        },
    }
}
