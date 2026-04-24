// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for the embedded MCP (Model Context Protocol) server.
//!
//! These tests exercise the MCP endpoint through real HTTP requests,
//! verifying auth enforcement, tool routing, and permission filtering.
//!
//! Requires the `mcp` feature: `cargo test --features mcp -p streamkit-server`

#![cfg(feature = "mcp")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

use axum::http::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use streamkit_server::Config;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::Duration;

/// Start a test server with MCP enabled and built-in auth.
async fn start_mcp_server() -> Option<(SocketAddr, tokio::task::JoinHandle<()>, String, TempDir)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();
    let temp_dir = TempDir::new().unwrap();

    let mut config = Config::default();
    config.mcp.enabled = true;
    config.auth.mode = streamkit_server::config::AuthMode::Enabled;
    config.auth.state_dir = temp_dir.path().to_string_lossy().to_string();

    let auth_state = streamkit_server::auth::AuthState::new(&config.auth, true)
        .await
        .expect("Failed to init auth state");
    let auth_state = Arc::new(auth_state);

    let admin_token_path = temp_dir.path().join("admin.token");
    let admin_token =
        tokio::fs::read_to_string(&admin_token_path).await.expect("Missing admin.token");
    let admin_token = admin_token.trim().to_string();

    let (app, _state) = streamkit_server::server::create_app(config, Some(auth_state));
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    Some((addr, server_handle, admin_token, temp_dir))
}

/// Send a JSON-RPC request to the MCP endpoint.
async fn mcp_post(
    client: &reqwest::Client,
    addr: SocketAddr,
    body: &serde_json::Value,
    auth_header: Option<&str>,
) -> reqwest::Response {
    let mut req = client
        .post(format!("http://{addr}/api/v1/mcp"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(body);

    if let Some(token) = auth_header {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    req.send().await.expect("Failed to send MCP request")
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn mcp_unauthenticated_request_is_rejected() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _handle, _token, _dir)) = start_mcp_server().await else {
        eprintln!("Skipping MCP tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    // JSON-RPC initialize request without auth
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0.1" }
        }
    });

    let res = mcp_post(&client, addr, &body, None).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_authenticated_initialize_succeeds() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _handle, token, _dir)) = start_mcp_server().await else {
        eprintln!("Skipping MCP tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0.1" }
        }
    });

    let res = mcp_post(&client, addr, &body, Some(&token)).await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn mcp_validate_pipeline_returns_diagnostics() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _handle, token, _dir)) = start_mcp_server().await else {
        eprintln!("Skipping MCP tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    // Initialize session first
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0.1" }
        }
    });
    let res = mcp_post(&client, addr, &init, Some(&token)).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Extract session ID from response header
    let session_id = res
        .headers()
        .get("mcp-session-id")
        .expect("missing mcp-session-id header")
        .to_str()
        .unwrap()
        .to_string();

    // Send initialized notification
    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let _ = client
        .post(format!("http://{addr}/api/v1/mcp"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .json(&initialized)
        .send()
        .await
        .expect("initialized notification failed");

    // Call validate_pipeline with invalid YAML
    let validate = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "validate_pipeline",
            "arguments": {
                "yaml": "not: valid: yaml: [["
            }
        }
    });
    let res = client
        .post(format!("http://{addr}/api/v1/mcp"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .json(&validate)
        .send()
        .await
        .expect("validate_pipeline request failed");
    assert_eq!(res.status(), StatusCode::OK);

    // Response is SSE stream — extract JSON-RPC result from `data:` lines.
    // The first `data:` line may be empty (SSE priming); find the one with JSON.
    let body_text = res.text().await.expect("failed to read response body");
    let json_str = body_text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .find(|s| s.starts_with('{'))
        .expect("no SSE data line with JSON found in response");
    let body: serde_json::Value = serde_json::from_str(json_str).expect("invalid JSON in SSE data");
    let result = &body["result"];
    assert!(!result.is_null(), "expected result in response, got: {body}");

    // The tool should return text content with validation diagnostics
    let content = &result["content"][0]["text"];
    assert!(content.is_string(), "expected text content in result");
    let text = content.as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(text).expect("tool output not valid JSON");
    assert_eq!(parsed["valid"], false);
    assert!(!parsed["errors"].as_array().unwrap().is_empty());
}
