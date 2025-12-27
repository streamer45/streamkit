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

async fn start_test_server_with_base_path(
    base_path: &str,
) -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    let base_path = base_path.to_string();
    let server_handle = tokio::spawn(async move {
        let mut config = Config::default();
        config.server.base_path = Some(base_path);

        let (app, _state) = streamkit_server::server::create_app(config);
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    Some((addr, server_handle))
}

#[tokio::test]
async fn base_path_serves_api_routes_without_proxy_rewrite() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server_with_base_path("/s/test").await else {
        eprintln!("Skipping base_path routing test: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    let root = format!("http://{addr}/api/v1/config");
    let nested = format!("http://{addr}/s/test/api/v1/config");

    let root_resp = client.get(&root).send().await.expect("Failed to GET root config");
    assert_eq!(root_resp.status(), StatusCode::OK);

    let nested_resp = client.get(&nested).send().await.expect("Failed to GET nested config");
    assert_eq!(nested_resp.status(), StatusCode::OK);
}
