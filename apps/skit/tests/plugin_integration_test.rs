// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args
)]

//! Plugin integration tests
//!
//! These tests exercise the full plugin lifecycle including loading, using in pipelines,
//! unloading, and reloading plugins.

use axum::http::StatusCode;
use reqwest::multipart;
use serde_json::json;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use streamkit_server::Config;
use tokio::fs;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, OnceCell};
use tokio::time::{timeout, Duration};

static GAIN_PLUGIN_PATH: OnceCell<std::path::PathBuf> = OnceCell::const_new();
static TEST_PERMIT: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

async fn acquire_test_permit() -> tokio::sync::OwnedSemaphorePermit {
    TEST_PERMIT
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
        .acquire_owned()
        .await
        .unwrap()
}

struct TestServer {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
    _temp_dir: tempfile::TempDir,
}

impl TestServer {
    async fn start() -> Option<Self> {
        let temp_dir = tempfile::tempdir().unwrap();
        let plugins_dir = temp_dir.path().join("plugins");
        fs::create_dir_all(&plugins_dir).await.unwrap();

        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
            Err(e) => panic!("Failed to bind test server listener: {e}"),
        };
        let addr = listener.local_addr().unwrap();

        let mut config = Config::default();
        config.plugins.directory = plugins_dir.to_string_lossy().to_string();
        config.plugins.allow_http_management = true;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let (app, _state) = streamkit_server::server::create_app(config);
            axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        Some(Self { addr, shutdown_tx, handle, _temp_dir: temp_dir })
    }

    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let mut handle = self.handle;
        if timeout(Duration::from_secs(3), &mut handle).await.is_err() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

/// Build the gain plugin if not already built
async fn ensure_gain_plugin_built() -> std::path::PathBuf {
    GAIN_PLUGIN_PATH
        .get_or_init(|| async {
            let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|parent| parent.parent())
                .expect("server crate should be within the workspace");
            let plugin_dir = repo_root.join("examples/plugins/gain-native");
            let plugin_name = if cfg!(target_os = "macos") {
                "libgain_plugin_native.dylib"
            } else if cfg!(target_os = "windows") {
                "gain_plugin_native.dll"
            } else {
                "libgain_plugin_native.so"
            };

            let plugin_path = plugin_dir.join("target/release").join(plugin_name);

            if !plugin_path.exists() {
                println!("Building gain plugin...");
                let output = tokio::process::Command::new("cargo")
                    .args(["build", "--release"])
                    .current_dir(plugin_dir)
                    .output()
                    .await
                    .expect("Failed to build gain plugin");

                assert!(
                    output.status.success(),
                    "Failed to build gain plugin:\nstdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            assert!(
                plugin_path.is_file(),
                "Expected gain plugin artifact at {}",
                plugin_path.display()
            );

            plugin_path
        })
        .await
        .clone()
}

#[tokio::test]
async fn test_list_empty_plugins() {
    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };

    let client = reqwest::Client::new();
    let url = format!("http://{}/api/v1/plugins", server.addr);

    let response = client.get(&url).send().await.expect("Failed to list plugins");

    assert_eq!(response.status(), StatusCode::OK);

    let plugins: Vec<serde_json::Value> =
        response.json().await.expect("Failed to parse plugins list");

    assert_eq!(plugins.len(), 0, "Should have no plugins initially");

    println!("✅ Empty plugin list returned correctly");
    server.shutdown().await;
}

#[tokio::test]
async fn test_load_native_plugin() {
    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };
    let plugin_path = ensure_gain_plugin_built().await;

    // Read plugin file
    let plugin_bytes = fs::read(&plugin_path).await.expect("Failed to read plugin file");

    // Upload plugin
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes)
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );

    let client = reqwest::Client::new();
    let url = format!("http://{}/api/v1/plugins", server.addr);

    let response = timeout(Duration::from_secs(10), client.post(&url).multipart(form).send())
        .await
        .expect("Upload timed out")
        .expect("Failed to upload plugin");

    assert_eq!(response.status(), StatusCode::CREATED, "Expected 201 CREATED");

    let summary: serde_json::Value = response.json().await.expect("Failed to parse response");

    println!("Uploaded plugin: {:?}", summary);

    // Verify plugin metadata
    assert_eq!(summary["kind"], "plugin::native::gain");
    assert_eq!(summary["original_kind"], "gain");
    assert_eq!(summary["plugin_type"], "native");
    assert!(summary["categories"].is_array());
    assert!(summary["loaded_at_ms"].is_number());

    println!("✅ Successfully loaded native plugin");

    // List plugins to verify it's loaded
    let list_url = format!("http://{}/api/v1/plugins", server.addr);
    let list_response = client.get(&list_url).send().await.expect("Failed to list plugins");

    assert_eq!(list_response.status(), StatusCode::OK);

    let plugins: Vec<serde_json::Value> =
        list_response.json().await.expect("Failed to parse plugins list");

    assert_eq!(plugins.len(), 1, "Expected 1 plugin to be loaded");
    assert_eq!(plugins[0]["kind"], "plugin::native::gain");

    println!("✅ Plugin appears in list");
    server.shutdown().await;
}

#[tokio::test]
async fn test_native_plugin_in_pipeline() {
    use futures_util::{SinkExt, StreamExt};
    use streamkit_api::{MessageType, Request, RequestPayload, ResponsePayload};
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };
    let plugin_path = ensure_gain_plugin_built().await;

    // Upload plugin
    let plugin_bytes = fs::read(&plugin_path).await.expect("Failed to read plugin file");
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes)
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );

    let client = reqwest::Client::new();
    let upload_url = format!("http://{}/api/v1/plugins", server.addr);

    let response =
        client.post(&upload_url).multipart(form).send().await.expect("Failed to upload plugin");
    assert_eq!(response.status(), StatusCode::CREATED);

    println!("✅ Uploaded plugin");

    // Create a session with the plugin

    let ws_url = format!("ws://{}/api/v1/control", server.addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect to WebSocket");
    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("create-session".to_string()),
        payload: RequestPayload::CreateSession { name: Some("Plugin Test".to_string()) },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "create-session").await;
    let ResponsePayload::SessionCreated { session_id, .. } = response.payload else {
        panic!("Expected SessionCreated");
    };

    println!("✅ Created session: {}", session_id);

    // Add plugin node to pipeline
    let add_node_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("add-node".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "gain_plugin".to_string(),
            kind: "plugin::native::gain".to_string(),
            params: Some(json!({"gain": 2.0})),
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&add_node_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "add-node").await;
    match response.payload {
        ResponsePayload::Success => {},
        ResponsePayload::Error { message } => panic!("Failed to add plugin node: {}", message),
        _ => panic!("Unexpected response"),
    }

    println!("✅ Added plugin node to pipeline");

    // Get pipeline to verify node was added
    let get_pipeline_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("get-pipeline".to_string()),
        payload: RequestPayload::GetPipeline { session_id: session_id.clone() },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&get_pipeline_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "get-pipeline").await;
    match response.payload {
        ResponsePayload::Pipeline { pipeline } => {
            assert_eq!(pipeline.nodes.len(), 1);
            assert!(pipeline.nodes.contains_key("gain_plugin"));
            assert_eq!(pipeline.nodes.get("gain_plugin").unwrap().kind, "plugin::native::gain");
        },
        _ => panic!("Expected Pipeline response"),
    }

    println!("✅ Plugin node successfully added to pipeline");
    server.shutdown().await;
}

#[tokio::test]
async fn test_load_invalid_plugin() {
    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };

    // Try to upload an invalid plugin (just random bytes)
    let invalid_bytes = vec![0u8; 1024];
    let form = multipart::Form::new()
        .part("plugin", multipart::Part::bytes(invalid_bytes).file_name("invalid.so"));

    let client = reqwest::Client::new();
    let url = format!("http://{}/api/v1/plugins", server.addr);

    let response = client.post(&url).multipart(form).send().await.expect("Failed to send request");

    // Should fail with an error (not 201 CREATED)
    assert_ne!(
        response.status(),
        StatusCode::CREATED,
        "Should not successfully load invalid plugin"
    );
    assert!(
        response.status().is_client_error() || response.status().is_server_error(),
        "Expected error status code"
    );

    println!("✅ Correctly rejected invalid plugin");
    server.shutdown().await;
}

/// Helper to read WebSocket messages, skipping events until we get a response with matching correlation_id
async fn read_response(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    expected_correlation_id: &str,
) -> streamkit_api::Response {
    use futures_util::StreamExt;

    loop {
        let message = timeout(Duration::from_secs(5), read.next())
            .await
            .expect("Timeout waiting for response")
            .expect("No message received")
            .expect("Failed to read message");

        let text = message.to_text().expect("Expected text message");

        // Try to parse as a generic message to check its type
        let value: serde_json::Value = serde_json::from_str(text).expect("Failed to parse message");

        let msg_type = value.get("type").and_then(|v| v.as_str());

        // Skip events, we only care about responses
        if msg_type == Some("event") {
            continue;
        }

        // Parse as response
        let response: streamkit_api::Response =
            serde_json::from_str(text).expect("Failed to parse response");

        // Check if correlation_id matches
        if response.correlation_id.as_deref() == Some(expected_correlation_id) {
            return response;
        }
    }
}

#[tokio::test]
async fn test_unload_native_plugin() {
    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };
    let plugin_path = ensure_gain_plugin_built().await;

    let client = reqwest::Client::new();
    let plugins_url = format!("http://{}/api/v1/plugins", server.addr);

    // Upload plugin
    let plugin_bytes = fs::read(&plugin_path).await.expect("Failed to read plugin file");
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes)
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );

    let response =
        client.post(&plugins_url).multipart(form).send().await.expect("Failed to upload");
    assert_eq!(response.status(), StatusCode::CREATED);
    println!("✅ Loaded plugin");

    // Verify plugin is listed
    let list_response = client.get(&plugins_url).send().await.expect("Failed to list");
    let plugins: Vec<serde_json::Value> = list_response.json().await.unwrap();
    assert_eq!(plugins.len(), 1);
    println!("✅ Plugin appears in list");

    // Unload plugin (URL-encoded plugin::native::gain)
    let unload_url = format!("http://{}/api/v1/plugins/plugin%3A%3Anative%3A%3Again", server.addr);
    let unload_response = client.delete(&unload_url).send().await.expect("Failed to unload");
    assert_eq!(
        unload_response.status(),
        StatusCode::OK,
        "Expected 200 OK on unload (returns plugin summary)"
    );
    println!("✅ Unloaded plugin");

    // Verify plugin is no longer listed
    let list_response = client.get(&plugins_url).send().await.expect("Failed to list");
    let plugins: Vec<serde_json::Value> = list_response.json().await.unwrap();
    assert_eq!(plugins.len(), 0, "Plugin should be unloaded");
    println!("✅ Plugin no longer in list");

    // Verify server is still responsive after unload
    let health_url = format!("http://{}/api/v1/config", server.addr);
    let health_response = client.get(&health_url).send().await.expect("Server not responsive");
    assert!(health_response.status().is_success(), "Server should still be healthy");
    println!("✅ Server still responsive after unload");
    server.shutdown().await;
}

#[tokio::test]
async fn test_reload_native_plugin() {
    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };
    let plugin_path = ensure_gain_plugin_built().await;

    let client = reqwest::Client::new();
    let plugins_url = format!("http://{}/api/v1/plugins", server.addr);
    let unload_url = format!("http://{}/api/v1/plugins/plugin%3A%3Anative%3A%3Again", server.addr);

    // First load
    let plugin_bytes = fs::read(&plugin_path).await.expect("Failed to read plugin file");
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes.clone())
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );
    let response = client.post(&plugins_url).multipart(form).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    println!("✅ First load successful");

    // Unload
    let response = client.delete(&unload_url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    println!("✅ Unload successful");

    // Small delay to ensure cleanup completes
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reload (second load after unload)
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes.clone())
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );
    let response = client.post(&plugins_url).multipart(form).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED, "Reload should succeed");
    println!("✅ Reload successful");

    // Verify plugin works after reload
    let list_response = client.get(&plugins_url).send().await.unwrap();
    let plugins: Vec<serde_json::Value> = list_response.json().await.unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["kind"], "plugin::native::gain");
    println!("✅ Plugin functional after reload");

    // Third cycle: unload and reload again
    let response = client.delete(&unload_url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    println!("✅ Second unload successful");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes)
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );
    let response = client.post(&plugins_url).multipart(form).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    println!("✅ Third load successful");

    // Final health check
    let health_url = format!("http://{}/api/v1/config", server.addr);
    let health_response = client.get(&health_url).send().await.unwrap();
    assert!(health_response.status().is_success());
    println!("✅ Server healthy after multiple reload cycles");
    server.shutdown().await;
}

#[tokio::test]
async fn test_unload_plugin_after_pipeline_use() {
    use futures_util::{SinkExt, StreamExt};
    use streamkit_api::{MessageType, Request, RequestPayload, ResponsePayload};
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    let _ = tracing_subscriber::fmt::try_init();
    let _permit = acquire_test_permit().await;

    let Some(server) = TestServer::start().await else {
        eprintln!("Skipping plugin integration tests: local TCP bind not permitted");
        return;
    };
    let plugin_path = ensure_gain_plugin_built().await;

    let client = reqwest::Client::new();
    let plugins_url = format!("http://{}/api/v1/plugins", server.addr);

    // Upload plugin
    let plugin_bytes = fs::read(&plugin_path).await.expect("Failed to read plugin file");
    let form = multipart::Form::new().part(
        "plugin",
        multipart::Part::bytes(plugin_bytes)
            .file_name(plugin_path.file_name().unwrap().to_string_lossy().to_string()),
    );
    let response = client.post(&plugins_url).multipart(form).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    println!("✅ Loaded plugin");

    // Create session and use plugin
    let ws_url = format!("ws://{}/api/v1/control", server.addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("Unload Test".to_string()) },
    };
    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "create").await;
    let ResponsePayload::SessionCreated { session_id, .. } = response.payload else {
        panic!("Expected SessionCreated");
    };
    println!("✅ Created session: {}", session_id);

    // Add plugin node
    let add_node = Request {
        message_type: MessageType::Request,
        correlation_id: Some("add".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "gain".to_string(),
            kind: "plugin::native::gain".to_string(),
            params: Some(json!({"gain": 1.5})),
        },
    };
    write.send(WsMessage::Text(serde_json::to_string(&add_node).unwrap().into())).await.unwrap();

    let response = read_response(&mut read, "add").await;
    assert!(matches!(response.payload, ResponsePayload::Success));
    println!("✅ Added plugin node to session");

    // Destroy session before unloading plugin
    let destroy = Request {
        message_type: MessageType::Request,
        correlation_id: Some("destroy".to_string()),
        payload: RequestPayload::DestroySession { session_id: session_id.clone() },
    };
    write.send(WsMessage::Text(serde_json::to_string(&destroy).unwrap().into())).await.unwrap();

    let response = read_response(&mut read, "destroy").await;
    assert!(
        matches!(response.payload, ResponsePayload::SessionDestroyed { .. }),
        "Expected SessionDestroyed, got {:?}",
        response.payload
    );
    println!("✅ Destroyed session");

    // Small delay to ensure session cleanup completes
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now unload the plugin
    let unload_url = format!("http://{}/api/v1/plugins/plugin%3A%3Anative%3A%3Again", server.addr);
    let unload_response = client.delete(&unload_url).send().await.expect("Failed to unload");
    assert_eq!(
        unload_response.status(),
        StatusCode::OK,
        "Should be able to unload plugin after session is destroyed"
    );
    println!("✅ Unloaded plugin after pipeline use");

    // Verify server is still healthy
    let health_url = format!("http://{}/api/v1/config", server.addr);
    let health_response = client.get(&health_url).send().await.unwrap();
    assert!(health_response.status().is_success());
    println!("✅ Server healthy after unload");
    server.shutdown().await;
}
