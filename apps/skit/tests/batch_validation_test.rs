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
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as WsMessage},
};

// Type aliases to reduce verbosity of the fully-expanded WebSocket stream types.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsWriter = futures_util::stream::SplitSink<WsStream, WsMessage>;
type WsReader = futures_util::stream::SplitStream<WsStream>;

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

/// Build a Config with a trusted role header so tests can select roles per connection.
fn config_with_role_header() -> Config {
    use streamkit_server::PermissionsConfig;

    Config {
        permissions: PermissionsConfig {
            role_header: Some("x-role".to_string()),
            ..PermissionsConfig::default()
        },
        ..Config::default()
    }
}

/// Connect to the WS control endpoint with a custom role header.
async fn connect_with_role(addr: SocketAddr, role: &str) -> (WsWriter, WsReader) {
    let mut request = format!("ws://{addr}/api/v1/control")
        .into_client_request()
        .expect("Failed to build WS request");
    request.headers_mut().insert("x-role", role.parse().unwrap());
    let (ws_stream, _) = connect_async(request).await.expect("Failed to connect to WebSocket");
    ws_stream.split()
}

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
        ResponsePayload::ValidationResult { errors } => {
            assert_eq!(errors.len(), 1, "Expected exactly one validation error");
            assert!(
                errors[0].message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {}",
                errors[0].message
            );
            assert_eq!(errors[0].node_id.as_deref(), Some("http_in"));
        },
        other => panic!("Expected ValidationResult for http_input, got: {:?}", other),
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
        ResponsePayload::ValidationResult { errors } => {
            assert_eq!(errors.len(), 1, "Expected exactly one validation error");
            assert!(
                errors[0].message.contains("oneshot-only"),
                "Expected oneshot-only error, got: {}",
                errors[0].message
            );
            assert_eq!(errors[0].node_id.as_deref(), Some("http_out"));
        },
        other => panic!("Expected ValidationResult for http_output, got: {:?}", other),
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
        ResponsePayload::ValidationResult { errors } => {
            assert_eq!(errors.len(), 1, "Expected exactly one validation error");
            assert!(
                errors[0].message.contains("plugin") && errors[0].message.contains("not allowed"),
                "Expected plugin not-allowed error, got: {}",
                errors[0].message
            );
            assert_eq!(errors[0].node_id.as_deref(), Some("p1"));
        },
        other => panic!("Expected ValidationResult for disallowed plugin, got: {:?}", other),
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
        ResponsePayload::ValidationResult { errors } => {
            assert_eq!(
                errors.len(),
                1,
                "Expected exactly one validation error for the oneshot node"
            );
            assert!(
                errors[0].message.contains("oneshot-only"),
                "Expected oneshot-only error in mixed batch, got: {}",
                errors[0].message
            );
            assert_eq!(errors[0].node_id.as_deref(), Some("http_in"));
        },
        other => {
            panic!("Expected ValidationResult for mixed batch with oneshot node, got: {:?}", other)
        },
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_duplicate_node_id() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    // Two AddNode ops with the same node_id should trigger a duplicate error.
    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![
            BatchOperation::AddNode {
                node_id: "dup1".to_string(),
                kind: "audio::gain".to_string(),
                params: None,
            },
            BatchOperation::AddNode {
                node_id: "dup1".to_string(),
                kind: "audio::gain".to_string(),
                params: None,
            },
        ],
        "validate-dup-node-id",
    )
    .await;

    match payload {
        ResponsePayload::ValidationResult { errors } => {
            assert!(!errors.is_empty(), "Expected at least one error for duplicate node_id");
            assert!(
                errors.iter().any(|e| e.message.contains("already exists")),
                "Expected duplicate node_id error, got: {:?}",
                errors.iter().map(|e| &e.message).collect::<Vec<_>>()
            );
        },
        other => panic!("Expected ValidationResult for duplicate node_id, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_reports_all_errors() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    // Two invalid nodes — both errors should be reported, not just the first.
    let payload = send_validate_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![
            BatchOperation::AddNode {
                node_id: "http_in".to_string(),
                kind: "streamkit::http_input".to_string(),
                params: None,
            },
            BatchOperation::AddNode {
                node_id: "http_out".to_string(),
                kind: "streamkit::http_output".to_string(),
                params: None,
            },
        ],
        "validate-all-errors",
    )
    .await;

    match payload {
        ResponsePayload::ValidationResult { errors } => {
            assert_eq!(errors.len(), 2, "Expected two validation errors, got {}", errors.len());
            assert!(
                errors.iter().all(|e| e.message.contains("oneshot-only")),
                "Expected both errors to be oneshot-only, got: {:?}",
                errors.iter().map(|e| &e.message).collect::<Vec<_>>()
            );
        },
        other => panic!("Expected ValidationResult with two errors, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_validate_batch_rejects_cross_role_ownership() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) =
        start_test_server_with_config(config_with_role_header()).await
    else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    // Connect as "admin" and create a session.
    let (mut admin_write, mut admin_read) = connect_with_role(addr, "admin").await;
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("admin-create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("admin-session".to_string()) },
    };
    admin_write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();
    let response = read_response(&mut admin_read, "admin-create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        other => panic!("Expected SessionCreated, got: {:?}", other),
    };

    // Connect as "user" (access_all_sessions = false) and try to validate on
    // the admin's session.
    let (mut user_write, mut user_read) = connect_with_role(addr, "user").await;
    let payload = send_validate_batch(
        &mut user_write,
        &mut user_read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: None,
        }],
        "user-validate-admin-session",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("Permission denied") || message.contains("not found"),
                "Expected ownership/permission error, got: {message}"
            );
        },
        other => {
            panic!("Expected Error for cross-role ownership in ValidateBatch, got: {:?}", other)
        },
    }
}

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

#[tokio::test]
async fn test_apply_batch_rejects_nonexistent_session() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect to WebSocket");
    let (mut write, mut read) = ws_stream.split();

    let payload = send_apply_batch(
        &mut write,
        &mut read,
        "nonexistent-session-id",
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: None,
        }],
        "apply-nonexistent-session",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("not found"),
                "Expected session not-found error, got: {message}"
            );
        },
        other => panic!("Expected Error for nonexistent session in ApplyBatch, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_batch_rejects_cross_role_ownership() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) =
        start_test_server_with_config(config_with_role_header()).await
    else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    // Connect as "admin" and create a session.
    let (mut admin_write, mut admin_read) = connect_with_role(addr, "admin").await;
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("admin-create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("admin-session".to_string()) },
    };
    admin_write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();
    let response = read_response(&mut admin_read, "admin-create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        other => panic!("Expected SessionCreated, got: {:?}", other),
    };

    // Connect as "user" (access_all_sessions = false) and try to apply on
    // the admin's session.
    let (mut user_write, mut user_read) = connect_with_role(addr, "user").await;
    let payload = send_apply_batch(
        &mut user_write,
        &mut user_read,
        &session_id,
        vec![BatchOperation::AddNode {
            node_id: "gain1".to_string(),
            kind: "audio::gain".to_string(),
            params: None,
        }],
        "user-apply-admin-session",
    )
    .await;

    match payload {
        ResponsePayload::Error { message } => {
            assert!(
                message.contains("Permission denied") || message.contains("not found"),
                "Expected ownership/permission error, got: {message}"
            );
        },
        other => panic!("Expected Error for cross-role ownership in ApplyBatch, got: {:?}", other),
    }
}

/// Confirmed-add regression test for the batch path (issue #455).
///
/// A batch whose `AddNode` ops mix a valid kind with one that passes
/// validation but fails engine construction must not leave an orphan in
/// `pipeline.nodes` for the failed node.  The valid node is confirmed and
/// visible; the failed one surfaces only as a Failed node state and never
/// lands in the durable snapshot.
#[tokio::test]
async fn test_apply_batch_failed_addnode_leaves_no_orphan() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping: local TCP bind not permitted");
        return;
    };

    let (mut write, mut read, session_id) = setup_session(addr).await;

    // `audio::*` is allowed for the default role, so the bogus kind passes
    // validation (which never consults the node registry) but has no
    // factory — the engine fails its creation after the batch is accepted.
    let payload = send_apply_batch(
        &mut write,
        &mut read,
        &session_id,
        vec![
            BatchOperation::AddNode {
                node_id: "gain_ok".to_string(),
                kind: "audio::gain".to_string(),
                params: Some(json!({"gain": 1.0})),
            },
            BatchOperation::AddNode {
                node_id: "ghost".to_string(),
                kind: "audio::definitely_not_a_real_kind".to_string(),
                params: None,
            },
        ],
        "apply-partial-failure",
    )
    .await;

    match payload {
        ResponsePayload::BatchApplied { success, errors } => {
            assert!(success, "batch should be accepted: validation does not consult the registry");
            assert!(errors.is_empty(), "expected no validation errors, got: {errors:?}");
        },
        other => panic!("Expected BatchApplied, got: {:?}", other),
    }

    // Drain events until the valid node is confirmed and the bogus one
    // reports Failed.  A `nodeadded` for the bogus id would mean an orphan.
    let mut saw_ok_added = false;
    let mut saw_ghost_failed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while (!saw_ok_added || !saw_ghost_failed) && tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(message))) = timeout(Duration::from_secs(5), read.next()).await else {
            continue;
        };
        let Ok(text) = message.to_text() else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { continue };
        if value.get("type").and_then(|v| v.as_str()) != Some("event") {
            continue;
        }
        let payload = value.get("payload");
        let event = payload.and_then(|p| p.get("event")).and_then(|e| e.as_str());
        let node_id = payload.and_then(|p| p.get("node_id")).and_then(|n| n.as_str());

        if event == Some("nodeadded") {
            assert_ne!(node_id, Some("ghost"), "failed creation must not emit nodeadded");
            if node_id == Some("gain_ok") {
                saw_ok_added = true;
            }
        } else if event == Some("nodestatechanged") && node_id == Some("ghost") {
            let state = payload.and_then(|p| p.get("state"));
            let is_failed = state.and_then(|s| s.as_str()) == Some("Failed")
                || state.and_then(|s| s.as_object()).is_some_and(|m| m.contains_key("Failed"));
            if is_failed {
                saw_ghost_failed = true;
            }
        }
    }
    assert!(saw_ok_added, "valid node 'gain_ok' was never confirmed");
    assert!(saw_ghost_failed, "bogus node 'ghost' never reported a Failed state");

    // The durable snapshot must contain only the confirmed node.
    let request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("partial-failure-pipeline".to_string()),
        payload: RequestPayload::GetPipeline { session_id: session_id.clone() },
    };
    write.send(WsMessage::Text(serde_json::to_string(&request).unwrap().into())).await.unwrap();

    match read_response(&mut read, "partial-failure-pipeline").await.payload {
        ResponsePayload::Pipeline { pipeline } => {
            assert!(pipeline.nodes.contains_key("gain_ok"), "confirmed node must be present");
            assert!(
                !pipeline.nodes.contains_key("ghost"),
                "failed AddNode must not leave an orphan, got nodes: {:?}",
                pipeline.nodes.keys().collect::<Vec<_>>(),
            );
        },
        other => panic!("Expected Pipeline response, got: {:?}", other),
    }
}
