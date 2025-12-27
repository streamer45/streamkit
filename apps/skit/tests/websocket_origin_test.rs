// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use streamkit_server::Config;
use tokio::net::TcpListener;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

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
async fn websocket_rejects_disallowed_origin() {
    let Some((addr, server_handle)) = start_test_server().await else {
        return;
    };

    let ws_url = format!("ws://{addr}/api/v1/control");
    let mut req = ws_url.into_client_request().unwrap();
    req.headers_mut().insert("Origin", "https://evil.example".parse().unwrap());

    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = err else {
        panic!("Expected HTTP error, got: {err:?}");
    };
    assert_eq!(response.status(), 403);

    server_handle.abort();
}
