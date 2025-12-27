// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_macros,
    clippy::uninlined_format_args,
    clippy::manual_let_else
)]

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use streamkit_api::{MessageType, Request, RequestPayload, Response, ResponsePayload};
use streamkit_core::control::NodeControlMessage;
use streamkit_server::Config;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

/// Helper to read messages from WebSocket, skipping events until we get a response with matching correlation_id
async fn read_response(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    expected_correlation_id: &str,
) -> Response {
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
        let response: Response = serde_json::from_str(text).expect("Failed to parse response");

        // Check if correlation_id matches
        if response.correlation_id.as_deref() == Some(expected_correlation_id) {
            return response;
        }
    }
}

async fn start_test_server() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    // Find an available port by binding to port 0
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(e) => panic!("Failed to bind test server listener: {e}"),
    };
    let addr = listener.local_addr().unwrap();

    // Start server in background using the existing listener
    let server_handle = tokio::spawn(async move {
        let (app, _state) = streamkit_server::server::create_app(Config::default());
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    Some((addr, server_handle))
}

#[tokio::test]
async fn test_create_and_destroy_session() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };

    // Connect to WebSocket
    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect to WebSocket");

    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("test-1".to_string()),
        payload: RequestPayload::CreateSession { name: Some("Test Session".to_string()) },
    };

    let msg = serde_json::to_string(&create_request).unwrap();
    write.send(WsMessage::Text(msg.into())).await.expect("Failed to send create session request");

    // Wait for response
    let response = read_response(&mut read, "test-1").await;

    assert_eq!(response.correlation_id, Some("test-1".to_string()));

    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, name, .. } => {
            assert_eq!(name, Some("Test Session".to_string()));
            session_id
        },
        _ => panic!("Expected SessionCreated response, got: {:?}", response.payload),
    };

    println!("✅ Session created with ID: {}", session_id);

    // List sessions
    let list_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("test-2".to_string()),
        payload: RequestPayload::ListSessions,
    };

    let msg = serde_json::to_string(&list_request).unwrap();
    write.send(WsMessage::Text(msg.into())).await.expect("Failed to send list sessions request");

    let response = read_response(&mut read, "test-2").await;

    match response.payload {
        ResponsePayload::SessionsListed { sessions } => {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session_id);
            assert_eq!(sessions[0].name, Some("Test Session".to_string()));
        },
        _ => panic!("Expected SessionsListed response"),
    }

    println!("✅ Session listed correctly");

    // Destroy session
    let destroy_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("test-3".to_string()),
        payload: RequestPayload::DestroySession { session_id: session_id.clone() },
    };

    let msg = serde_json::to_string(&destroy_request).unwrap();
    write.send(WsMessage::Text(msg.into())).await.expect("Failed to send destroy session request");

    let response = read_response(&mut read, "test-3").await;

    match response.payload {
        ResponsePayload::SessionDestroyed { session_id: destroyed_id } => {
            assert_eq!(destroyed_id, session_id);
        },
        _ => panic!("Expected SessionDestroyed response"),
    }

    println!("✅ Session destroyed successfully");

    // Verify session no longer exists
    let list_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("test-4".to_string()),
        payload: RequestPayload::ListSessions,
    };

    let msg = serde_json::to_string(&list_request).unwrap();
    write.send(WsMessage::Text(msg.into())).await.expect("Failed to send list sessions request");

    let response = read_response(&mut read, "test-4").await;

    match response.payload {
        ResponsePayload::SessionsListed { sessions } => {
            assert_eq!(sessions.len(), 0);
        },
        _ => panic!("Expected SessionsListed response"),
    }

    println!("✅ Session list is empty after destruction");
}

#[tokio::test]
async fn test_multiple_sessions() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut write, mut read) = ws_stream.split();

    // Create three sessions
    let mut session_ids = Vec::new();

    for i in 1..=3 {
        let create_request = Request {
            message_type: MessageType::Request,
            correlation_id: Some(format!("create-{}", i)),
            payload: RequestPayload::CreateSession { name: Some(format!("Session {}", i)) },
        };

        write
            .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
            .await
            .unwrap();

        let response = read_response(&mut read, &format!("create-{}", i)).await;

        if let ResponsePayload::SessionCreated { session_id, .. } = response.payload {
            session_ids.push(session_id);
        }
    }

    assert_eq!(session_ids.len(), 3);
    println!("✅ Created 3 sessions");

    // List all sessions
    let list_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("list".to_string()),
        payload: RequestPayload::ListSessions,
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&list_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "list").await;

    match response.payload {
        ResponsePayload::SessionsListed { sessions } => {
            assert_eq!(sessions.len(), 3);
        },
        _ => panic!("Expected SessionsListed"),
    }

    println!("✅ All 3 sessions listed correctly");

    // Destroy all sessions
    for (i, session_id) in session_ids.iter().enumerate() {
        let destroy_request = Request {
            message_type: MessageType::Request,
            correlation_id: Some(format!("destroy-{}", i)),
            payload: RequestPayload::DestroySession { session_id: session_id.clone() },
        };

        write
            .send(WsMessage::Text(serde_json::to_string(&destroy_request).unwrap().into()))
            .await
            .unwrap();

        let _response = read_response(&mut read, &format!("destroy-{}", i)).await;
    }

    println!("✅ Destroyed all 3 sessions");
}

#[tokio::test]
async fn test_add_and_remove_nodes() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("create".to_string()),
        payload: RequestPayload::CreateSession { name: None },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        _ => panic!("Expected SessionCreated"),
    };

    // Add a gain node
    let add_node_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("add-node".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "gain1".to_string(),
            kind: "gain".to_string(),
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
        ResponsePayload::Error { message } => panic!("Failed to add node: {}", message),
        _ => panic!("Unexpected response"),
    }

    println!("✅ Added gain node");

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
            assert!(pipeline.nodes.contains_key("gain1"));
            assert_eq!(pipeline.nodes.get("gain1").unwrap().kind, "gain");
        },
        _ => panic!("Expected Pipeline response"),
    }

    println!("✅ Pipeline contains the gain node");

    // Remove the node
    let remove_node_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("remove-node".to_string()),
        payload: RequestPayload::RemoveNode {
            session_id: session_id.clone(),
            node_id: "gain1".to_string(),
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&remove_node_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "remove-node").await;
    match response.payload {
        ResponsePayload::Success => {},
        _ => panic!("Expected Success response"),
    }

    println!("✅ Removed gain node");

    // Verify node was removed
    let get_pipeline_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("get-pipeline-2".to_string()),
        payload: RequestPayload::GetPipeline { session_id: session_id.clone() },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&get_pipeline_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "get-pipeline-2").await;
    match response.payload {
        ResponsePayload::Pipeline { pipeline } => {
            assert_eq!(pipeline.nodes.len(), 0);
        },
        _ => panic!("Expected Pipeline response"),
    }

    println!("✅ Pipeline is empty after node removal");
}

#[tokio::test]
async fn test_session_not_found() {
    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut write, mut read) = ws_stream.split();

    // Try to get pipeline for non-existent session
    let request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("test".to_string()),
        payload: RequestPayload::GetPipeline { session_id: "non-existent-id".to_string() },
    };

    write.send(WsMessage::Text(serde_json::to_string(&request).unwrap().into())).await.unwrap();

    let response = read_response(&mut read, "test").await;
    match response.payload {
        ResponsePayload::Error { message } => {
            assert!(message.contains("not found") || message.contains("does not exist"));
        },
        _ => panic!("Expected Error response"),
    }

    println!("✅ Correctly handles non-existent session");
}

#[tokio::test]
async fn test_session_destroy_shuts_down_pipeline() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };

    let ws_url = format!("ws://{}/api/v1/control", addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("Pipeline Test".to_string()) },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        _ => panic!("Expected SessionCreated"),
    };

    println!("✅ Session created: {}", session_id);

    // Add a source node (silence generator)
    let add_source_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("add-source".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "source".to_string(),
            kind: "silence".to_string(),
            params: Some(json!({
                "duration_ms": 10000, // 10 seconds
                "sample_rate": 48000,
                "channels": 2
            })),
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&add_source_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "add-source").await;
    match response.payload {
        ResponsePayload::Success => {},
        ResponsePayload::Error { message } => panic!("Failed to add source: {}", message),
        _ => panic!("Unexpected response"),
    }

    println!("✅ Added source node");

    // Add a gain node
    let add_gain_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("add-gain".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "gain".to_string(),
            kind: "gain".to_string(),
            params: Some(json!({"gain": 1.0})),
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&add_gain_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "add-gain").await;
    match response.payload {
        ResponsePayload::Success => {},
        ResponsePayload::Error { message } => panic!("Failed to add gain: {}", message),
        _ => panic!("Unexpected response"),
    }

    println!("✅ Added gain node");

    // Connect source to gain
    let connect_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("connect".to_string()),
        payload: RequestPayload::Connect {
            session_id: session_id.clone(),
            from_node: "source".to_string(),
            from_pin: "out".to_string(),
            to_node: "gain".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&connect_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "connect").await;
    match response.payload {
        ResponsePayload::Success => {},
        ResponsePayload::Error { message } => panic!("Failed to connect: {}", message),
        _ => panic!("Unexpected response"),
    }

    println!("✅ Connected nodes");

    // Wait a bit to ensure the pipeline is running
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get pipeline state to verify nodes are running
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
            assert_eq!(pipeline.nodes.len(), 2);

            // Check that nodes are in Running state (not Failed or Stopped)
            for (node_id, node) in &pipeline.nodes {
                if let Some(state) = &node.state {
                    println!("Node '{}' state: {:?}", node_id, state);
                    assert!(
                        matches!(
                            state,
                            streamkit_core::NodeState::Initializing
                                | streamkit_core::NodeState::Ready
                                | streamkit_core::NodeState::Running
                        ),
                        "Node '{}' should be initializing/ready/running, got: {:?}",
                        node_id,
                        state
                    );
                } else {
                    println!("Node '{}' has no state yet", node_id);
                }
            }
        },
        _ => panic!("Expected Pipeline response"),
    }

    println!("✅ Pipeline is running with nodes in valid states");

    // Now destroy the session - this should shut down all nodes immediately
    let destroy_start = std::time::Instant::now();

    let destroy_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("destroy".to_string()),
        payload: RequestPayload::DestroySession { session_id: session_id.clone() },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&destroy_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "destroy").await;

    let destroy_duration = destroy_start.elapsed();

    match response.payload {
        ResponsePayload::SessionDestroyed { session_id: destroyed_id } => {
            assert_eq!(destroyed_id, session_id);

            // Verify the shutdown completed within a reasonable time
            // The fix ensures we wait for shutdown, so it should complete
            // within the 10 second timeout we set in shutdown_and_wait
            assert!(
                destroy_duration < Duration::from_secs(11),
                "Session destroy took too long: {:?}",
                destroy_duration
            );

            println!("✅ Session destroyed and pipeline shut down in {:?}", destroy_duration);
        },
        ResponsePayload::Error { message } => panic!("Failed to destroy session: {}", message),
        _ => panic!("Unexpected response"),
    }

    // Verify session no longer exists
    let list_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("list".to_string()),
        payload: RequestPayload::ListSessions,
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&list_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "list").await;
    match response.payload {
        ResponsePayload::SessionsListed { sessions } => {
            assert_eq!(sessions.len(), 0, "Session should be completely removed");
        },
        _ => panic!("Expected SessionsListed response"),
    }

    println!("✅ Session completely removed from session list");
    println!("✅ Test completed: Session destruction properly shuts down pipeline");
}

/// Test that concurrent operations don't cause lock contention.
///
/// This test detects the "async mutex held across await" bug by:
/// 1. Creating multiple sessions
/// 2. Running concurrent operations from multiple WebSocket connections
/// 3. Measuring latency - if any operation takes >2s, lock contention is likely
///
/// The original bug caused GetPipeline to take 21+ seconds under load because
/// the session_manager lock was held while waiting for engine responses.
#[tokio::test]
async fn test_concurrent_operations_no_lock_contention() {
    let _ = tracing_subscriber::fmt::try_init();

    let Some((addr, _server_handle)) = start_test_server().await else {
        eprintln!("Skipping session lifecycle tests: local TCP bind not permitted");
        return;
    };
    let ws_url = format!("ws://{}/api/v1/control", addr);

    // Create initial session and add a node
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut write, mut read) = ws_stream.split();

    // Create session
    let create_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("setup-create".to_string()),
        payload: RequestPayload::CreateSession { name: Some("Contention Test".to_string()) },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&create_request).unwrap().into()))
        .await
        .unwrap();

    let response = read_response(&mut read, "setup-create").await;
    let session_id = match response.payload {
        ResponsePayload::SessionCreated { session_id, .. } => session_id,
        _ => panic!("Expected SessionCreated"),
    };

    // Add a gain node to tune
    let add_node_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("setup-add-node".to_string()),
        payload: RequestPayload::AddNode {
            session_id: session_id.clone(),
            node_id: "gain".to_string(),
            kind: "gain".to_string(),
            params: Some(json!({"gain": 1.0})),
        },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&add_node_request).unwrap().into()))
        .await
        .unwrap();

    let _ = read_response(&mut read, "setup-add-node").await;

    println!("✅ Setup complete: session {} with gain node", session_id);

    // Track max latency across all operations
    let max_latency_ms = Arc::new(AtomicU64::new(0));
    let operation_count = Arc::new(AtomicU64::new(0));

    // Spawn multiple concurrent tasks that perform operations
    let num_tasks = 5;
    let ops_per_task = 10;
    let mut handles = Vec::new();

    for task_id in 0..num_tasks {
        let ws_url = ws_url.clone();
        let session_id = session_id.clone();
        let max_latency = Arc::clone(&max_latency_ms);
        let op_count = Arc::clone(&operation_count);

        let handle = tokio::spawn(async move {
            // Each task gets its own WebSocket connection
            let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
            let (mut write, mut read) = ws_stream.split();

            for op_id in 0..ops_per_task {
                let correlation_id = format!("task{}-op{}", task_id, op_id);
                let start = Instant::now();

                // Alternate between different operation types
                let request = match op_id % 3 {
                    0 => Request {
                        message_type: MessageType::Request,
                        correlation_id: Some(correlation_id.clone()),
                        payload: RequestPayload::GetPipeline { session_id: session_id.clone() },
                    },
                    1 => Request {
                        message_type: MessageType::Request,
                        correlation_id: Some(correlation_id.clone()),
                        payload: RequestPayload::TuneNode {
                            session_id: session_id.clone(),
                            node_id: "gain".to_string(),
                            message: NodeControlMessage::UpdateParams(
                                serde_saphyr::from_str(&format!(
                                    "gain: {}",
                                    f64::from(op_id).mul_add(0.1, 1.0)
                                ))
                                .unwrap(),
                            ),
                        },
                    },
                    _ => Request {
                        message_type: MessageType::Request,
                        correlation_id: Some(correlation_id.clone()),
                        payload: RequestPayload::ListSessions,
                    },
                };

                write
                    .send(WsMessage::Text(serde_json::to_string(&request).unwrap().into()))
                    .await
                    .unwrap();

                let _ = read_response(&mut read, &correlation_id).await;

                #[allow(clippy::cast_possible_truncation)] // latency won't exceed u64::MAX ms
                let latency_ms = start.elapsed().as_millis() as u64;
                op_count.fetch_add(1, Ordering::Relaxed);

                // Update max latency
                let mut current_max = max_latency.load(Ordering::Relaxed);
                while latency_ms > current_max {
                    match max_latency.compare_exchange_weak(
                        current_max,
                        latency_ms,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(x) => current_max = x,
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.expect("Task panicked");
    }

    let total_ops = operation_count.load(Ordering::Relaxed);
    let max_latency = max_latency_ms.load(Ordering::Relaxed);

    println!("✅ Completed {} concurrent operations, max latency: {}ms", total_ops, max_latency);

    // The critical assertion: if any operation took more than 2 seconds,
    // we likely have lock contention. Before the fix, GetPipeline would
    // take 21+ seconds under this kind of load.
    let max_acceptable_latency_ms: u64 = 2000;
    assert!(
        max_latency < max_acceptable_latency_ms,
        "Lock contention detected! Max latency was {}ms (threshold: {}ms). \
         This likely indicates the session_manager lock is being held across await points.",
        max_latency,
        max_acceptable_latency_ms
    );

    // Cleanup
    let destroy_request = Request {
        message_type: MessageType::Request,
        correlation_id: Some("cleanup".to_string()),
        payload: RequestPayload::DestroySession { session_id: session_id.clone() },
    };

    write
        .send(WsMessage::Text(serde_json::to_string(&destroy_request).unwrap().into()))
        .await
        .unwrap();

    let _ = read_response(&mut read, "cleanup").await;

    println!("✅ Test passed: No lock contention detected");
}
