// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

use axum::http::StatusCode;
use std::net::SocketAddr;
use streamkit_server::Config;
use tokio::net::TcpListener;
use tokio::time::Duration;

async fn start_test_server() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        let (app, _state) = streamkit_server::server::create_app(Config::default());
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    Some((addr, server_handle))
}

#[tokio::test]
async fn test_create_session_duplicate_name_returns_conflict() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping HTTP session tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/sessions");

    let pipeline_yaml = r"
mode: dynamic
steps:
  - kind: core::passthrough
";

    let body = serde_json::json!({
        "name": "dup",
        "yaml": pipeline_yaml
    });

    let first = client.post(&url).json(&body).send().await.expect("Failed to create session");
    assert_eq!(first.status(), StatusCode::OK);

    let second =
        client.post(&url).json(&body).send().await.expect("Failed to create duplicate session");
    assert_eq!(second.status(), StatusCode::CONFLICT);
}
