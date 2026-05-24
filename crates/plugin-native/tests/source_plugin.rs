// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests that exercise the source-plugin lifecycle
//! (`NativeNodeWrapper::run_source`) through the FFI boundary.
//!
//! The fixture `tests/fixtures/source-plugin/` is a real cdylib that
//! exports a tick-driven source plugin via `native_source_plugin_entry!`.
//! The build script copies it next to the test binary so we can `dlopen`
//! it like any other plugin.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::PathBuf;

use streamkit_core::control::NodeControlMessage;
use streamkit_core::node::{OutputRouting, OutputSender, RoutedPacketMessage};
use streamkit_core::types::Packet;
use streamkit_core::{NodeContext, NodeState, NodeStateUpdate, PipelineMode};
use streamkit_plugin_native::LoadedNativePlugin;
use tokio::sync::mpsc;

fn source_plugin_so_path() -> PathBuf {
    let path_file = PathBuf::from(env!("OUT_DIR")).join("source_plugin_path");
    let path_str = std::fs::read_to_string(&path_file).unwrap_or_else(|e| {
        panic!("Failed to read {}: {e}", path_file.display());
    });
    let so_path = PathBuf::from(path_str.trim());
    assert!(so_path.exists(), "Fixture .so not found at {}", so_path.display());
    so_path
}

fn load_source_plugin() -> LoadedNativePlugin {
    let so_path = source_plugin_so_path();
    let mut loaded = LoadedNativePlugin::load(&so_path)
        .unwrap_or_else(|e| panic!("Failed to load source-plugin: {e}"));
    loaded.set_call_timeout(Some(std::time::Duration::from_secs(5)));
    loaded
}

fn source_node_context() -> (
    NodeContext,
    mpsc::Receiver<NodeStateUpdate>,
    mpsc::Sender<NodeControlMessage>,
    mpsc::Receiver<RoutedPacketMessage>,
) {
    let (state_tx, state_rx) = mpsc::channel(32);
    let (control_tx, control_rx) = mpsc::channel(16);
    let (routed_tx, routed_rx) = mpsc::channel(64);

    let output_sender =
        OutputSender::new("source-test".to_string(), OutputRouting::Routed(routed_tx));

    let ctx = NodeContext {
        inputs: HashMap::new(),
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
    (ctx, state_rx, control_tx, routed_rx)
}

async fn await_state_matching<F: Fn(&NodeState) -> bool>(
    state_rx: &mut mpsc::Receiver<NodeStateUpdate>,
    matcher: F,
    label: &str,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) => {
                if matcher(&update.state) {
                    return;
                }
            },
            Ok(None) => panic!("State channel closed before '{label}' was observed"),
            Err(elapsed) => {
                panic!("Source node never reached state '{label}' within deadline: {elapsed}")
            },
        }
    }
}

/// Briefly yield to let the detached worker thread drain its channel and
/// run `InstanceState::drop` (which calls back into the loaded `.so` via
/// `destroy_instance`) before the test subprocess starts unloading the
/// dlopen'd library.
///
/// Under coverage instrumentation the first call into a freshly-loaded
/// `.so` is noticeably slower, exposing a teardown race in
/// `NativeNodeWrapper::run_source`: `run()` returns without joining or
/// signalling the worker, so the detached worker can still be inside the
/// `.so` when the subprocess exits.  Tracked as a follow-up issue (see
/// the PR description).
async fn drain_detached_worker() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

#[tokio::test]
async fn source_plugin_metadata_reports_is_source_after_probe() {
    let plugin = load_source_plugin();
    let meta = plugin.metadata();
    assert!(meta.is_source, "source-config probe must mark plugin as a source, got: {meta:?}");
    assert_eq!(meta.tick_interval_us, 1_000, "tick interval propagates from SourceConfig");
    assert_eq!(meta.outputs.len(), 1);
    assert_eq!(meta.outputs[0].name, "output");
    assert!(meta.inputs.is_empty(), "source plugins must declare no inputs at the metadata level");

    // The source plugin fixture declares one category and a non-empty
    // JSON Schema, exercising the host's categories loop and the
    // non-empty param_schema parse branch in extract_metadata.
    assert_eq!(
        meta.categories,
        vec!["test".to_string()],
        "category set by the fixture must round-trip through metadata extraction"
    );
    assert_eq!(
        meta.param_schema.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "non-empty param_schema must parse into the declared JSON object"
    );
    assert!(
        meta.param_schema.get("properties").is_some(),
        "param_schema must round-trip its 'properties' object, got: {:?}",
        meta.param_schema
    );
}

#[tokio::test]
async fn source_plugin_ticks_and_emits_then_completes_after_max_ticks() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "emit_n", "max_ticks": 2 })))
        .expect("create_node succeeds for source plugin");

    let (ctx, mut state_rx, control_tx, mut routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    // Source plugins go through Initializing → Ready → wait for Start → Running.
    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;

    control_tx.send(NodeControlMessage::Start).await.expect("control open");

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Running), "Running").await;

    let mut received = Vec::new();
    for _ in 0..2 {
        let (_src, pin, packet) =
            tokio::time::timeout(std::time::Duration::from_secs(5), routed_rx.recv())
                .await
                .expect("source must emit within timeout")
                .expect("router channel must yield a packet");
        assert_eq!(&*pin, "output", "all packets must arrive on the declared output pin");
        match packet {
            Packet::Text(s) => received.push(s.to_string()),
            other => panic!("expected Text packet, got: {other:?}"),
        }
    }

    assert_eq!(
        received,
        vec!["tick-1", "tick-2"],
        "source must emit exactly max_ticks packets in order"
    );

    // After max_ticks the tick callback signals completion; the node should
    // exit its run loop with Ok.
    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should complete within timeout")
        .expect("source node task should not panic");
    assert!(
        run_result.is_ok(),
        "source plugin run loop must exit Ok after tick returns completion signal, got: {run_result:?}"
    );
    drain_detached_worker().await;
}

#[tokio::test]
async fn source_plugin_tick_error_marks_node_failed_with_plugin_message() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "error_tick" })))
        .expect("create_node succeeds for source plugin");

    let (ctx, mut state_rx, control_tx, _routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;
    control_tx.send(NodeControlMessage::Start).await.expect("control open");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let failure_reason = loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) => {
                if let NodeState::Failed { reason } = &update.state {
                    break reason.clone();
                }
            },
            Ok(None) => panic!("State channel closed before Failed was observed"),
            Err(elapsed) => {
                panic!("Source node never reached Failed state within deadline: {elapsed}")
            },
        }
    };
    assert!(
        failure_reason.contains("intentional error in tick"),
        "Failure reason must come from the plugin's tick error, got: {failure_reason}"
    );

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should complete within timeout")
        .expect("source node task should not panic");
    assert!(
        run_result.is_err(),
        "source plugin run loop must return Err after tick error, got: {run_result:?}"
    );
    drain_detached_worker().await;
}

#[tokio::test]
async fn source_plugin_shutdown_control_terminates_cleanly_before_max_ticks() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "emit_n", "max_ticks": 1_000_000 })))
        .expect("create_node succeeds for source plugin");

    let (ctx, mut state_rx, control_tx, mut routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;
    control_tx.send(NodeControlMessage::Start).await.expect("control open");

    // Pull at least one packet to confirm ticks are flowing.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), routed_rx.recv())
        .await
        .expect("at least one tick must arrive")
        .expect("router channel open");

    // Now issue Shutdown via the control channel. The source loop must exit
    // cleanly without waiting for max_ticks.
    control_tx.send(NodeControlMessage::Shutdown).await.expect("control open");

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should shut down within timeout")
        .expect("source node task should not panic");
    assert!(
        run_result.is_ok(),
        "shutdown control must produce Ok exit even mid-stream, got: {run_result:?}"
    );
    drain_detached_worker().await;
}

#[tokio::test]
async fn source_plugin_shutdown_before_start_exits_cleanly() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "emit_n", "max_ticks": 1 })))
        .expect("create_node succeeds");

    let (ctx, mut state_rx, control_tx, _routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;

    // Shutdown BEFORE Start exercises the early-shutdown branch of the
    // Ready→Start handshake (run_source ~line 1743-1755).
    control_tx.send(NodeControlMessage::Shutdown).await.expect("control open");

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should shut down within timeout")
        .expect("source node task should not panic");
    assert!(
        run_result.is_ok(),
        "Shutdown received before Start must yield clean Ok exit, got: {run_result:?}"
    );
    drain_detached_worker().await;
}

#[tokio::test]
async fn source_plugin_control_channel_close_before_start_exits_cleanly() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "emit_n", "max_ticks": 1 })))
        .expect("create_node succeeds");

    let (ctx, mut state_rx, control_tx, _routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;

    // Dropping the control sender exercises the control_rx.recv()=None
    // branch of the Ready→Start handshake (~line 1768-1780).
    drop(control_tx);

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should shut down within timeout")
        .expect("source node task should not panic");
    assert!(
        run_result.is_ok(),
        "control channel close before Start must yield clean Ok exit, got: {run_result:?}"
    );
    drain_detached_worker().await;
}

/// Pins the host contract that an `UpdateParams` arriving in the
/// Ready→Start window is accepted by the wrapper (routed through
/// `apply_params_update` and the worker thread) without failing the
/// node, even when the fixture's `update_params` is a no-op.
///
/// This intentionally does NOT assert that the new param values changed
/// observable behaviour — the TickingSource fixture inherits the SDK's
/// default `update_params` (an `Ok(())` no-op), so `max_ticks` cannot
/// change post-construction.  The branch under coverage is the host's
/// pre-Start `UpdateParams` plumbing, not the fixture's own params
/// handling.
#[tokio::test]
async fn source_plugin_update_params_before_start_is_accepted_without_failing_the_node() {
    let plugin = load_source_plugin();
    let node = plugin
        .create_node(Some(&serde_json::json!({ "mode": "emit_n", "max_ticks": 1_000_000 })))
        .expect("create_node succeeds");

    let (ctx, mut state_rx, control_tx, mut routed_rx) = source_node_context();
    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_state_matching(&mut state_rx, |s| matches!(s, NodeState::Ready), "Ready").await;

    control_tx
        .send(NodeControlMessage::UpdateParams(serde_json::json!({
            "mode": "emit_n",
            "max_ticks": 1
        })))
        .await
        .expect("control open");

    control_tx.send(NodeControlMessage::Start).await.expect("control open");

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), routed_rx.recv())
        .await
        .expect("at least one tick must arrive")
        .expect("router channel open");

    control_tx.send(NodeControlMessage::Shutdown).await.expect("control open");

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("source node should shut down within timeout")
        .expect("source node task should not panic");
    // A pre-Start UpdateParams that flips the node to Failed would also
    // make run() return Err, so this single assertion subsumes a separate
    // state-stream scan for Failed transitions.
    assert!(
        run_result.is_ok(),
        "UpdateParams before Start must not fail the node, got: {run_result:?}"
    );
    drain_detached_worker().await;
}
