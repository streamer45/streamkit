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
use reqwest::header::{HeaderValue, AUTHORIZATION, COOKIE};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use streamkit_server::Config;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

async fn start_test_server_with_auth(
) -> Option<(SocketAddr, tokio::task::JoinHandle<()>, String, TempDir)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    let temp_dir = TempDir::new().unwrap();

    let mut config = Config::default();
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

    sleep(Duration::from_millis(50)).await;
    Some((addr, server_handle, admin_token, temp_dir))
}

#[tokio::test]
async fn http_api_requires_auth_when_enabled() {
    let Some((addr, server_handle, admin_token, _temp_dir)) = start_test_server_with_auth().await
    else {
        eprintln!("Skipping auth integration tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    // Health remains public
    let res =
        client.get(format!("http://{addr}/healthz")).send().await.expect("Failed to GET /healthz");
    assert_eq!(res.status(), StatusCode::OK);

    // /auth/me is public and reports unauthenticated when no token
    let res = client
        .get(format!("http://{addr}/api/v1/auth/me"))
        .send()
        .await
        .expect("Failed to GET /api/v1/auth/me");
    assert_eq!(res.status(), StatusCode::OK);
    let me: serde_json::Value = res.json().await.expect("Invalid JSON from /auth/me");
    assert_eq!(me["auth_enabled"], true);
    assert_eq!(me["authenticated"], false);

    // Protected API should require auth
    let res = client
        .get(format!("http://{addr}/api/v1/permissions"))
        .send()
        .await
        .expect("Failed to GET /api/v1/permissions");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Authorization: Bearer should work
    let res = client
        .get(format!("http://{addr}/api/v1/permissions"))
        .header(AUTHORIZATION, format!("Bearer {admin_token}"))
        .send()
        .await
        .expect("Failed to GET /api/v1/permissions with bearer");
    assert_eq!(res.status(), StatusCode::OK);

    // Login should set session cookie
    let res = client
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&json!({ "token": admin_token }))
        .send()
        .await
        .expect("Failed to POST /api/v1/auth/login");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let set_cookie = res.headers().get("set-cookie").expect("Missing set-cookie").to_str().unwrap();
    let cookie_kv = set_cookie.split(';').next().expect("Invalid set-cookie");

    // Cookie should authenticate
    let res = client
        .get(format!("http://{addr}/api/v1/permissions"))
        .header(COOKIE, HeaderValue::from_str(cookie_kv).unwrap())
        .send()
        .await
        .expect("Failed to GET /api/v1/permissions with cookie");
    assert_eq!(res.status(), StatusCode::OK);

    server_handle.abort();
}

#[tokio::test]
async fn token_revocation_is_enforced() {
    let Some((addr, server_handle, admin_token, _temp_dir)) = start_test_server_with_auth().await
    else {
        eprintln!("Skipping auth integration tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();

    // Mint a viewer token
    let res = client
        .post(format!("http://{addr}/api/v1/auth/tokens"))
        .header(AUTHORIZATION, format!("Bearer {admin_token}"))
        .json(&json!({
            "role": "viewer",
            "label": "test-viewer",
            "ttl_secs": 3600
        }))
        .send()
        .await
        .expect("Failed to mint token");
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Invalid JSON from mint token");
    let viewer_token = body["token"].as_str().unwrap().to_string();
    let jti = body["jti"].as_str().unwrap().to_string();

    // Viewer token should authenticate
    let res = client
        .get(format!("http://{addr}/api/v1/permissions"))
        .header(AUTHORIZATION, format!("Bearer {viewer_token}"))
        .send()
        .await
        .expect("Failed to call API with viewer token");
    assert_eq!(res.status(), StatusCode::OK);
    let perms: serde_json::Value = res.json().await.expect("Invalid JSON from /permissions");
    assert_eq!(perms["role"], "viewer");

    // Revoke token
    let res = client
        .delete(format!("http://{addr}/api/v1/auth/tokens/{jti}"))
        .header(AUTHORIZATION, format!("Bearer {admin_token}"))
        .send()
        .await
        .expect("Failed to revoke token");
    assert_eq!(res.status(), StatusCode::OK);

    // Revoked token should be rejected
    let res = client
        .get(format!("http://{addr}/api/v1/permissions"))
        .header(AUTHORIZATION, format!("Bearer {viewer_token}"))
        .send()
        .await
        .expect("Failed to call API with revoked token");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    server_handle.abort();
}

#[tokio::test]
async fn websocket_requires_auth_when_enabled() {
    let Some((addr, server_handle, admin_token, _temp_dir)) = start_test_server_with_auth().await
    else {
        eprintln!("Skipping auth integration tests: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{addr}/api/v1/control");

    // Unauthenticated websocket should fail
    let err = tokio_tungstenite::connect_async(&ws_url).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = err else {
        panic!("Expected HTTP error, got: {err:?}");
    };
    assert_eq!(response.status(), 401);

    // Authenticated websocket should connect
    let mut req = ws_url.into_client_request().unwrap();
    req.headers_mut()
        .insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {admin_token}")).unwrap());

    let (_ws, _) = tokio_tungstenite::connect_async(req).await.expect("WS connect failed");

    server_handle.abort();
}
