// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration test: load a real `.so` plugin that panics in
//! `process_packet` and verify the host survives.
//!
//! The test fixture lives in `tests/fixtures/panicking-plugin/`.
//! A build script (`build.rs`) compiles the fixture before tests run
//! so the `.so` is always fresh.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use streamkit_core::node::{OutputRouting, OutputSender};
use streamkit_core::types::Packet;
use streamkit_core::{NodeContext, NodeStateUpdate, PipelineMode};
use streamkit_plugin_native::LoadedNativePlugin;
use tokio::sync::mpsc;

/// Return the path to the compiled panicking-plugin `.so`.
///
/// The build script (`build.rs`) compiles the fixture crate and writes
/// the artefact path to `OUT_DIR/panicking_plugin_path`.
fn fixture_so_path() -> PathBuf {
    let path_file = PathBuf::from(env!("OUT_DIR")).join("panicking_plugin_path");
    let path_str = std::fs::read_to_string(&path_file).unwrap_or_else(|e| {
        panic!("Failed to read {}: {e}. Was the build script skipped?", path_file.display());
    });
    let so_path = PathBuf::from(path_str.trim());
    assert!(so_path.exists(), "Fixture .so not found at {}", so_path.display());
    so_path
}

/// Build a minimal [`NodeContext`] wired to the returned channels so
/// the test can drive the node's processing loop.
fn test_node_context(
    input_rx: mpsc::Receiver<Packet>,
) -> (
    NodeContext,
    mpsc::Receiver<NodeStateUpdate>,
    mpsc::Sender<streamkit_core::control::NodeControlMessage>,
) {
    let (state_tx, state_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);

    // Output routing: routed channel that we can ignore.
    let (routed_tx, _routed_rx) = mpsc::channel(64);
    let output_sender =
        OutputSender::new("test-node".to_string(), OutputRouting::Routed(routed_tx));

    let mut inputs = HashMap::new();
    inputs.insert("input".to_string(), input_rx);

    let ctx = NodeContext {
        inputs,
        input_types: HashMap::new(),
        control_rx,
        output_sender,
        batch_size: 1,
        state_tx,
        stats_tx: None,
        telemetry_tx: None,
        session_id: None,
        cancellation_token: None,
        pin_management_rx: None,
        audio_pool: None,
        video_pool: None,
        pipeline_mode: PipelineMode::Oneshot,
        view_data_tx: None,
        engine_control_tx: None,
    };
    (ctx, state_rx, control_tx)
}

#[tokio::test]
async fn panicking_plugin_returns_error_not_abort() {
    // ── Load the real .so ──────────────────────────────────────────────
    let so_path = fixture_so_path();
    let loaded = LoadedNativePlugin::load(&so_path)
        .unwrap_or_else(|e| panic!("Failed to load panicking plugin: {e}"));

    assert_eq!(loaded.metadata().kind, "panicking");

    // ── Create a node instance ─────────────────────────────────────────
    let mut plugin = loaded.clone();
    plugin.set_call_timeout(Some(std::time::Duration::from_secs(5)));
    let node = plugin
        .create_node(None)
        .unwrap_or_else(|e| panic!("Failed to create panicking-plugin node: {e}"));

    // ── Wire up channels and run the node ──────────────────────────────
    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, _control_tx) = test_node_context(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    // Wait for the node to reach Running state before sending the packet.
    let mut saw_running = false;
    while let Ok(update) =
        tokio::time::timeout(std::time::Duration::from_secs(5), state_rx.recv()).await
    {
        if let Some(update) = update {
            if matches!(update.state, streamkit_core::NodeState::Running) {
                saw_running = true;
                break;
            }
        }
    }
    assert!(saw_running, "Node never reached Running state");

    // ── Send a packet that will trigger the panic ──────────────────────
    let packet = Packet::Text(Arc::from("trigger panic"));
    input_tx.send(packet).await.expect("input channel should be open");

    // Drain state updates to find the Failed state.
    let mut saw_failed = false;
    let mut failure_reason = String::new();
    while let Ok(update) =
        tokio::time::timeout(std::time::Duration::from_secs(5), state_rx.recv()).await
    {
        if let Some(update) = update {
            if let streamkit_core::NodeState::Failed { reason } = &update.state {
                saw_failed = true;
                failure_reason = reason.clone();
                break;
            }
        }
    }

    // ── Assertions ─────────────────────────────────────────────────────
    // 1. The host received an error (not a process abort).
    assert!(saw_failed, "Node should have emitted Failed state after panic");

    // 2. The panic message is logged / propagated.
    assert!(
        failure_reason.contains("panicking-plugin: intentional panic in process_packet"),
        "Failure reason should contain the panic message, got: {failure_reason}"
    );

    // 3. The node task completed with an error (not a crash).
    let run_result = tokio::time::timeout(std::time::Duration::from_secs(5), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic (JoinError)");

    assert!(run_result.is_err(), "run() should return Err after a plugin panic");
    let err_msg = run_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("panicking-plugin: intentional panic in process_packet"),
        "run() error should contain the panic message, got: {err_msg}"
    );
}
