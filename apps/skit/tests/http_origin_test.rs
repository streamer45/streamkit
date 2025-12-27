// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use streamkit_server::Config;
use tokio::net::TcpListener;
use tokio::time::{sleep, Duration};

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

    sleep(Duration::from_millis(50)).await;
    Some((addr, server_handle))
}

#[tokio::test]
async fn http_origin_middleware_blocks_mutating_requests_from_disallowed_origins() {
    let Some((addr, server_handle)) = start_test_server().await else {
        return;
    };

    let url = format!("http://{addr}/api/v1/process");
    let client = reqwest::Client::new();

    // Disallowed origin should be blocked before request parsing.
    let res = client.post(&url).header("Origin", "https://evil.example").send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // Allowed origin should not be blocked by Origin middleware (it will still fail validation
    // because the request is not a valid multipart payload).
    let res = client.post(&url).header("Origin", "http://localhost:1234").send().await.unwrap();
    assert_ne!(res.status(), reqwest::StatusCode::FORBIDDEN);

    server_handle.abort();
}
