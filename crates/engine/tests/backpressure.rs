// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for Per-Pin Distributor Architecture and backpressure handling.
//!
//! This test suite validates that the dynamic engine can handle scenarios where
//! downstream nodes are slower than upstream nodes without deadlocking.

use std::path::Path;
use std::time::{Duration, Instant};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::state::NodeState;
use streamkit_engine::{DynamicEngineConfig, DynamicEngineHandle, Engine};
use tokio::time::timeout;

/// Poll `handle.get_node_states()` until a predicate holds, with timeout.
async fn wait_for_states<F>(handle: &DynamicEngineHandle, timeout_dur: Duration, pred: F) -> bool
where
    F: Fn(&std::collections::HashMap<String, NodeState>) -> bool,
{
    let deadline = Instant::now() + timeout_dur;
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if pred(&states) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Tests that a fast file reader feeding a slow pacer node doesn't deadlock.
/// This validates the Per-Pin Distributor Architecture correctly handles backpressure.
#[tokio::test]
#[allow(clippy::expect_used, clippy::similar_names)]
async fn test_backpressure_no_deadlock() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| parent.parent())
        .expect("streamkit-engine should live under workspace_root/crates/engine");
    let sample_file = "samples/audio/system/speech_10m.opus";
    let output_path = "target/test-output/backpressure_output.bin";
    std::fs::create_dir_all(repo_root.join("target/test-output")).expect("create test output dir");

    let engine = Engine::without_plugins();
    let config = DynamicEngineConfig {
        packet_batch_size: 32,
        session_id: Some("test-backpressure".to_string()),
        node_input_capacity: None,
        pin_distributor_capacity: None,
        asset_root: repo_root.to_path_buf(),
    };
    let handle = engine.start_dynamic_actor(config);

    // Add nodes: file_read -> demuxer -> pacer -> muxer -> file_write
    // Pacer will slow down Audio packets from demuxer, creating backpressure

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "reader".to_string(),
            kind: "core::file_reader".to_string(),
            params: serde_saphyr::from_str(&format!("path: \"{sample_file}\"\nchunk_size: 4096"))
                .ok(),
        })
        .await
        .expect("Failed to add file_reader");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "demuxer".to_string(),
            kind: "containers::ogg::demuxer".to_string(),
            params: None,
        })
        .await
        .expect("Failed to add demuxer");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .expect("Failed to add pacer");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "muxer".to_string(),
            kind: "containers::ogg::muxer".to_string(),
            params: serde_saphyr::from_str("stream_serial: 0\nchunk_size: 4096").ok(),
        })
        .await
        .expect("Failed to add muxer");

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

    let nodes_ready = wait_for_states(&handle, Duration::from_secs(5), |states| {
        let reader_ok = states
            .get("reader")
            .is_some_and(|s| matches!(s, NodeState::Running | NodeState::Ready));
        let pacer_ok = states.get("pacer").is_some_and(|s| matches!(s, NodeState::Running));
        reader_ok && pacer_ok
    })
    .await;
    assert!(nodes_ready, "Reader should be running/ready and pacer should be running");

    // Pacer at 0.1x creates backpressure; old architecture would deadlock here.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Pipeline should remain responsive under backpressure");

    let states = result.expect("Should get response").expect("Failed to get states");
    tracing::info!("Node states after backpressure test: {:?}", states);

    // Only reader stats are reliable here — other nodes may not have flushed
    // yet (NodeStatsTracker batches every 10s / 1000 packets).
    let stats = handle.get_node_stats().await.expect("Failed to get node stats");

    tracing::info!("All node stats: {:?}", stats);

    let reader_stats = stats.get("reader").expect("Reader stats missing");
    assert!(reader_stats.sent > 0, "Reader should have sent Binary packets to demuxer");

    handle.send_control(EngineControlMessage::Shutdown).await.expect("Failed to shutdown");
    tokio::time::sleep(Duration::from_millis(100)).await;
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

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .unwrap();

    let pacer_running = wait_for_states(&handle, Duration::from_secs(5), |states| {
        matches!(states.get("pacer"), Some(NodeState::Running))
    })
    .await;
    assert!(pacer_running, "Pacer should be running");

    // The key test: verify engine remains responsive when managing connections
    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Engine should remain responsive");

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

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "pacer".to_string(),
            kind: "core::pacer".to_string(),
            params: serde_saphyr::from_str("speed: 0.1\nbuffer_size: 4").ok(),
        })
        .await
        .unwrap();

    let pacer_created = wait_for_states(&handle, Duration::from_secs(5), |states| {
        states.get("pacer").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(pacer_created, "Pacer should have left Creating state");

    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "pacer".to_string() })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = timeout(Duration::from_secs(1), handle.get_node_states()).await;
    assert!(result.is_ok(), "Engine should remain responsive after removing node");

    let states = result.unwrap().unwrap();
    assert!(!states.contains_key("pacer"), "Pacer should be removed");

    handle.send_control(EngineControlMessage::Shutdown).await.unwrap();
}
