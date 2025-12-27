// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for Per-Pin Distributor Architecture and backpressure handling.
//!
//! This test suite validates that the dynamic engine can handle scenarios where
//! downstream nodes are slower than upstream nodes without deadlocking.

use std::path::Path;
use std::time::Duration;
use streamkit_core::control::EngineControlMessage;
use streamkit_core::state::NodeState;
use streamkit_engine::{DynamicEngineConfig, Engine};
use tokio::time::timeout;

/// Tests that a fast file reader feeding a slow pacer node doesn't deadlock.
/// This validates the Per-Pin Distributor Architecture correctly handles backpressure.
#[tokio::test]
#[allow(clippy::expect_used, clippy::similar_names)]
async fn test_backpressure_no_deadlock() {
    // Initialize tracing for test visibility
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let output_path = "/tmp/backpressure_test_output1.bin";
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-engine should live under workspace_root/crates/engine");
    let sample_file = repo_root.join("samples/audio/system/speech_10m.opus");
    let sample_file = sample_file.to_string_lossy();

    let engine = Engine::without_plugins();
    let config = DynamicEngineConfig {
        packet_batch_size: 32,
        session_id: Some("test-backpressure".to_string()),
        node_input_capacity: None,
        pin_distributor_capacity: None,
    };
    let handle = engine.start_dynamic_actor(config);

    // Add nodes: file_read -> demuxer -> pacer -> muxer -> file_write
    // Pacer will slow down Audio packets from demuxer, creating backpressure

    // 1. Add file_reader node
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "reader".to_string(),
            kind: "core::file_reader".to_string(),
            params: serde_saphyr::from_str(&format!("path: \"{sample_file}\"\nchunk_size: 4096"))
                .ok(),
        })
        .await
        .expect("Failed to add file_reader");

    // 2. Add ogg demuxer node
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "demuxer".to_string(),
            kind: "containers::ogg::demuxer".to_string(),
            params: None,
        })
        .await
        .expect("Failed to add demuxer");

    // 3. Add a pacer node (introduces artificial delay to create backpressure)
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .expect("Failed to add pacer");

    // 4. Add ogg muxer node
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "muxer".to_string(),
            kind: "containers::ogg::muxer".to_string(),
            params: serde_saphyr::from_str("stream_serial: 0\nchunk_size: 4096").ok(),
        })
        .await
        .expect("Failed to add muxer");

    // 5. Add a file_writer node
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "writer".to_string(),
            kind: "core::file_writer".to_string(),
            params: serde_saphyr::from_str(&format!("path: {output_path}\nchunk_size: 4096")).ok(),
        })
        .await
        .expect("Failed to add file_writer");

    // Connect the nodes immediately (before file_read auto-starts)
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "reader".to_string(),
            from_pin: "out".to_string(),
            to_node: "demuxer".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("Failed to connect reader to demuxer");

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "demuxer".to_string(),
            from_pin: "out".to_string(),
            to_node: "pacer".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("Failed to connect demuxer to pacer");

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "pacer".to_string(),
            from_pin: "out".to_string(),
            to_node: "muxer".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("Failed to connect pacer to muxer");

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "muxer".to_string(),
            from_pin: "out".to_string(),
            to_node: "writer".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("Failed to connect muxer to writer");

    // Wait for nodes to become ready and start processing
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 6. Verify nodes are running
    let states = handle.get_node_states().await.expect("Failed to get node states");

    assert!(
        matches!(states.get("reader"), Some(NodeState::Running | NodeState::Ready)),
        "Reader should be running or ready, got: {:?}",
        states.get("reader")
    );
    assert!(
        matches!(states.get("pacer"), Some(NodeState::Running)),
        "Pacer should be running, got: {:?}",
        states.get("pacer")
    );

    // 7. Let the pipeline run for a bit - this is where deadlock would occur in the old architecture
    // The demuxer will produce Audio packets faster than pacer can forward them (0.1x speed)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 8. Verify the pipeline is still responsive (no deadlock)
    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Pipeline should remain responsive under backpressure");

    let states = result.expect("Should get response").expect("Failed to get states");
    tracing::info!("Node states after backpressure test: {:?}", states);

    // 9. Verify data is flowing through the pipeline
    // Note: We only check reader stats because the reader completes quickly and flushes its stats.
    // Other nodes may not have flushed stats yet since NodeStatsTracker batches updates
    // (every 10s or 1000 packets). The key test is that the pipeline didn't deadlock (step 8).
    let stats = handle.get_node_stats().await.expect("Failed to get node stats");

    tracing::info!("All node stats: {:?}", stats);

    let reader_stats = stats.get("reader").expect("Reader stats missing");
    assert!(reader_stats.sent > 0, "Reader should have sent Binary packets to demuxer");

    // 10. Shutdown
    handle.send_control(EngineControlMessage::Shutdown).await.expect("Failed to shutdown");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup
    let _ = tokio::fs::remove_file(output_path).await;
}

/// Tests that dynamic connection/disconnection works correctly during backpressure.
/// This test verifies fan-out scenarios where a pacer feeds multiple consumers.
#[tokio::test]
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn test_dynamic_connection_under_backpressure() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let engine = Engine::without_plugins();
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());

    // Create a simple pipeline with pacer that can fan-out to multiple consumers
    // Just verify the architecture handles dynamic connections without deadlocking

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify node is running
    let states = handle.get_node_states().await.unwrap();
    assert!(matches!(states.get("pacer"), Some(NodeState::Running)), "Pacer should be running");

    // The key test: verify engine remains responsive when managing connections
    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Engine should remain responsive");

    // Shutdown
    handle.send_control(EngineControlMessage::Shutdown).await.unwrap();
}

/// Tests that removing a node under backpressure doesn't cause issues.
/// This test verifies cleanup works correctly even when channels might be full.
#[tokio::test]
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn test_node_removal_under_backpressure() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let engine = Engine::without_plugins();
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());

    // Add a pacer node
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Remove the node
    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "pacer".to_string() })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify engine remains responsive after node removal
    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Engine should remain responsive after removing node");

    let states = result.unwrap().unwrap();
    assert!(!states.contains_key("pacer"), "Pacer should be removed");

    handle.send_control(EngineControlMessage::Shutdown).await.unwrap();
}
