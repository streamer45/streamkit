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

use streamkit_core::node::{OutputRouting, OutputSender, RoutedPacketMessage};
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

/// Build a NodeContext where the test owns the routed output receiver
/// so it can observe packets emitted by the plugin.
fn test_node_context_with_output_observer(
    input_rx: mpsc::Receiver<Packet>,
) -> (
    NodeContext,
    mpsc::Receiver<NodeStateUpdate>,
    mpsc::Sender<streamkit_core::control::NodeControlMessage>,
    mpsc::Receiver<RoutedPacketMessage>,
) {
    let (state_tx, state_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);

    let (routed_tx, routed_rx) = mpsc::channel(64);
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
    (ctx, state_rx, control_tx, routed_rx)
}

async fn await_running(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) if matches!(update.state, streamkit_core::NodeState::Running) => {
                return;
            },
            Ok(Some(_)) => {},
            Ok(None) => panic!("State channel closed before Running was observed"),
            Err(elapsed) => panic!("Node never reached Running state within deadline: {elapsed}"),
        }
    }
}

async fn await_failed(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) -> String {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) => {
                if let streamkit_core::NodeState::Failed { reason } = &update.state {
                    return reason.clone();
                }
            },
            Ok(None) => panic!("State channel closed before Failed was observed"),
            Err(elapsed) => panic!("Node never reached Failed state within deadline: {elapsed}"),
        }
    }
}

fn load_plugin_fixture() -> LoadedNativePlugin {
    let so_path = fixture_so_path();
    let mut loaded = LoadedNativePlugin::load(&so_path)
        .unwrap_or_else(|e| panic!("Failed to load panicking plugin: {e}"));
    loaded.set_call_timeout(Some(std::time::Duration::from_secs(5)));
    loaded
}

fn create_node_with_mode(
    plugin: &LoadedNativePlugin,
    mode: &str,
) -> Result<Box<dyn streamkit_core::ProcessorNode>, streamkit_core::StreamKitError> {
    plugin.create_node(Some(&serde_json::json!({ "mode": mode })))
}

#[tokio::test]
async fn panicking_plugin_returns_error_not_abort() {
    let plugin = load_plugin_fixture();
    assert_eq!(plugin.metadata().kind, "panicking");

    let node = create_node_with_mode(&plugin, "panic_process")
        .unwrap_or_else(|e| panic!("Failed to create panicking-plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, _control_tx, _routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_running(&mut state_rx).await;

    input_tx.send(Packet::Text(Arc::from("trigger panic"))).await.expect("input open");

    let failure_reason = await_failed(&mut state_rx).await;

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

#[tokio::test]
async fn error_process_surfaces_plugin_error_message_via_failed_state() {
    let plugin = load_plugin_fixture();
    let node = create_node_with_mode(&plugin, "error_process")
        .unwrap_or_else(|e| panic!("Failed to create plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, _control_tx, _routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_running(&mut state_rx).await;

    input_tx.send(Packet::Text(Arc::from("trigger error"))).await.expect("input open");

    let failure_reason = await_failed(&mut state_rx).await;
    assert!(
        failure_reason.contains("intentional error in process_packet"),
        "Failure reason must include the plugin-supplied error message, got: {failure_reason}"
    );

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(5), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic (JoinError)");
    assert!(
        run_result.is_err(),
        "run() must return Err after plugin returns an error from process"
    );
    let err_msg = run_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("intentional error in process_packet"),
        "run() error must include the plugin-supplied error message, got: {err_msg}"
    );
}

#[tokio::test]
async fn create_node_returns_configuration_error_when_plugin_new_returns_null() {
    let plugin = load_plugin_fixture();

    // When the plugin's `new()` returns Err, the SDK macro causes
    // `create_instance` to return a null handle, and the wrapper must
    // surface a Configuration error instead of constructing a node.
    let err = plugin
        .create_node(Some(&serde_json::json!({ "mode": "error_new" })))
        .err()
        .expect("plugin returning Err from new must yield Err from create_node");

    match err {
        streamkit_core::StreamKitError::Configuration(msg) => {
            assert!(
                msg.contains("Plugin failed to create instance"),
                "Configuration error must mention failed instance creation, got: {msg}"
            );
        },
        other => panic!("Expected Configuration error, got: {other:?}"),
    }
}

#[tokio::test]
async fn passthrough_lifecycle_emits_output_packet_and_completes_cleanly_on_input_close() {
    let plugin = load_plugin_fixture();
    let node = create_node_with_mode(&plugin, "passthrough")
        .unwrap_or_else(|e| panic!("Failed to create plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, _control_tx, mut routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_running(&mut state_rx).await;

    input_tx.send(Packet::Text(Arc::from("hello"))).await.expect("input open");

    let (_src_node, pin, packet) =
        tokio::time::timeout(std::time::Duration::from_secs(5), routed_rx.recv())
            .await
            .expect("should receive output within timeout")
            .expect("router channel must yield a packet");
    assert_eq!(&*pin, "output", "packet must arrive on the declared output pin");
    match packet {
        Packet::Text(s) => assert_eq!(&*s, "hello", "passthrough must preserve payload"),
        other => panic!("expected Text packet, got: {other:?}"),
    }

    // Closing the input must terminate the node cleanly (Ok return).
    drop(input_tx);

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic");
    assert!(run_result.is_ok(), "passthrough lifecycle must complete cleanly, got: {run_result:?}");
}

/// Exercises the happy-path of the update_params control flow: control
/// message arrives, worker receives `WorkerRequest::UpdateParams`, plugin's
/// `update_params` returns Ok, and the node keeps running. Coverage gain is
/// in the wrapper's `apply_params_update` async pathway end-to-end.
#[tokio::test]
async fn update_params_control_message_completes_without_failing_the_node() {
    let plugin = load_plugin_fixture();
    let node = create_node_with_mode(&plugin, "passthrough")
        .unwrap_or_else(|e| panic!("Failed to create plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, control_tx, mut routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_running(&mut state_rx).await;

    control_tx
        .send(streamkit_core::control::NodeControlMessage::UpdateParams(
            serde_json::json!({ "mode": "passthrough" }),
        ))
        .await
        .expect("control channel open");

    // After update_params, the node must continue processing packets normally.
    input_tx.send(Packet::Text(Arc::from("after-update"))).await.expect("input open");

    let (_src, pin, packet) =
        tokio::time::timeout(std::time::Duration::from_secs(5), routed_rx.recv())
            .await
            .expect("output should arrive after update_params")
            .expect("router channel must yield a packet");
    assert_eq!(&*pin, "output");
    match packet {
        Packet::Text(s) => assert_eq!(&*s, "after-update"),
        other => panic!("expected Text packet, got: {other:?}"),
    }

    drop(input_tx);

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic");
    assert!(
        run_result.is_ok(),
        "update_params + clean shutdown must produce Ok, got: {run_result:?}"
    );
}

/// BUG (tracked in follow-up issue): `apply_params_update` only logs an
/// `update_params` failure at WARN level and returns `Ok(())`, so the node
/// keeps running with stale parameters and no observable signal escapes to
/// callers. This test pins the current behaviour (plugin error during
/// update_params is silently absorbed) so an intentional contract change
/// will surface as a failed test.
#[tokio::test]
async fn update_params_error_is_silently_absorbed_today_and_node_keeps_running() {
    let plugin = load_plugin_fixture();
    let node = create_node_with_mode(&plugin, "error_update_params")
        .unwrap_or_else(|e| panic!("Failed to create plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, control_tx, _routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });

    await_running(&mut state_rx).await;

    control_tx
        .send(streamkit_core::control::NodeControlMessage::UpdateParams(
            serde_json::json!({ "mode": "error_update_params" }),
        ))
        .await
        .expect("control channel open");

    // Confirm no Failed state arrives anywhere in a 500 ms window. The
    // absence of a Failed transition is what we are asserting, so we must
    // drain every update the worker emits — a single recv would miss a
    // Failed that lands after an unrelated heartbeat.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while let Ok(Some(update)) = tokio::time::timeout_at(deadline, state_rx.recv()).await {
        if let streamkit_core::NodeState::Failed { reason } = &update.state {
            panic!("Plugin update_params error must NOT mark node Failed today, got: {reason}");
        }
    }

    drop(control_tx);
    drop(input_tx);

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic");
    assert!(
        run_result.is_ok(),
        "Today the node still exits Ok after an update_params error, got: {run_result:?}"
    );
}

#[tokio::test]
async fn loaded_plugin_exposes_metadata_and_api_and_library_accessors() {
    let plugin = load_plugin_fixture();
    let meta: &streamkit_plugin_native::PluginMetadata = plugin.metadata();
    assert_eq!(meta.kind, "panicking", "metadata kind matches NodeMetadata::builder");
    assert!(!meta.is_source, "panicking-plugin must register as a processor, not a source");
    assert_eq!(meta.inputs.len(), 1, "panicking-plugin declares one input pin");
    assert_eq!(meta.inputs[0].name, "input");
    assert_eq!(meta.outputs.len(), 1, "panicking-plugin declares one output pin");
    assert_eq!(meta.outputs[0].name, "output");

    let api = plugin.api();
    assert!(api.version >= 6, "fixture is compiled against the current SDK version");

    let lib = plugin.library();
    assert!(Arc::strong_count(lib) >= 1, "library Arc is alive");

    let cloned = plugin.clone();
    assert_eq!(cloned.metadata().kind, "panicking", "clone preserves metadata");
}

#[tokio::test]
async fn register_plugins_adds_namespaced_kind_and_returns_count() {
    use streamkit_core::NodeRegistry;
    use streamkit_plugin_native::register_plugins;

    let plugin = load_plugin_fixture();
    let mut registry = NodeRegistry::new();

    let count = register_plugins(&mut registry, vec![plugin]).expect("registration succeeds");
    assert_eq!(count, 1, "exactly one plugin registered");

    // Confirm the registry now knows the namespaced kind, not the raw kind.
    assert!(registry.contains("plugin::native::panicking"), "namespaced kind must be registered");
    assert!(
        !registry.contains("panicking"),
        "raw (unprefixed) kind must not leak into the registry"
    );
    assert!(
        registry.get_definition("plugin::native::panicking").is_some(),
        "registry must have a NodeDefinition for the namespaced kind"
    );
}

#[tokio::test]
async fn set_call_timeout_overrides_default_and_can_reset_to_none() {
    let mut plugin = load_plugin_fixture();
    plugin.set_call_timeout(Some(std::time::Duration::from_millis(500)));
    // Round-trip a node creation to confirm the overridden timeout does not
    // break instance construction.
    let node = create_node_with_mode(&plugin, "passthrough").expect("create_node still works");
    drop(node);

    plugin.set_call_timeout(None);
    let node = create_node_with_mode(&plugin, "passthrough").expect("create_node still works");
    drop(node);
}

#[tokio::test]
async fn error_flush_keeps_run_loop_alive_with_warn_only_on_input_close() {
    // The wrapper logs flush errors at WARN and still returns Ok() from
    // run().  Pin that behavior: error_flush mode must not cause the node
    // to mark itself Failed.  The error path inside the worker is
    // observably exercised by the plugin's flush implementation returning
    // Err — coverage gain is in wrapper.rs flush error-message branch
    // (~lines 604-612).
    let plugin = load_plugin_fixture();
    let node = create_node_with_mode(&plugin, "error_flush")
        .unwrap_or_else(|e| panic!("Failed to create plugin node: {e}"));

    let (input_tx, input_rx) = mpsc::channel::<Packet>(16);
    let (ctx, mut state_rx, _control_tx, _routed_rx) =
        test_node_context_with_output_observer(input_rx);

    let node_handle = tokio::spawn(async move { node.run(ctx).await });
    await_running(&mut state_rx).await;

    drop(input_tx);

    let run_result = tokio::time::timeout(std::time::Duration::from_secs(10), node_handle)
        .await
        .expect("node task should complete within timeout")
        .expect("node task should not panic");
    assert!(
        run_result.is_ok(),
        "flush errors must surface as WARN only, not propagate as run() failure, got: {run_result:?}"
    );
}
