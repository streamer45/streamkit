// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Regression tests for wasmtime epoch-based guest interruption (#532).
//!
//! A guest that spins in a tight compute loop (no host calls) never yields
//! to the async runtime, so without an execution-time bound the node task
//! would hang forever and ignore `Shutdown`. These tests load a hand-written
//! WAT component whose constructor / process / update-params bodies spin,
//! and assert the host interrupts the call within the configured deadline,
//! the node transitions to `Failed`, and the run task completes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::time::Duration;

use streamkit_core::control::NodeControlMessage;
use streamkit_core::node::{OutputRouting, OutputSender};
use streamkit_core::types::Packet;
use streamkit_core::{NodeContext, NodeState, NodeStateUpdate, PipelineMode, StreamKitError};
use streamkit_plugin_wasm::{PluginRuntime, PluginRuntimeConfig};
use tokio::sync::mpsc;

const SPIN: &str = "(loop $spin (br $spin)) unreachable";

/// Builds a minimal plugin component in WAT form implementing the
/// `streamkit:plugin` world, with selectable runaway (tight-loop) bodies.
/// Compiling WAT in-test keeps the fixture hermetic — no wasm toolchain
/// (cargo-component, wasm32 targets) is needed at build time.
fn component_wat(
    metadata_body: &str,
    ctor_body: &str,
    process_body: &str,
    update_params_body: &str,
) -> String {
    format!(
        r#"(component
  (component $inner
    (core module $impl
      (import "rt" "resource-new" (func $rnew (param i32) (result i32)))
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 8192))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $ret i32)
        (local.set $ret (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get 3)))
        (local.get $ret))
      (func (export "metadata") (result i32) {metadata_body})
      (func (export "ctor") (param i32 i32 i32) (result i32) {ctor_body})
      (func (export "process") (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32) {process_body})
      (func (export "update-params") (param i32 i32 i32 i32) (result i32) {update_params_body})
      (func (export "cleanup") (param i32))
    )
    (type $node-instance (resource (rep i32)))
    (core func $rnew (canon resource.new $node-instance))
    (core instance $rt (export "resource-new" (func $rnew)))
    (core instance $i (instantiate $impl (with "rt" (instance $rt))))
    (alias core export $i "memory" (core memory $mem))
    (alias core export $i "cabi_realloc" (core func $realloc))

    ;; Exported functions may only reference *named* (exported) types, so
    ;; each composite type is exported and the exported id used downstream.
    (type $sample-format0 (enum "float32" "s16-le"))
    (export $sample-format "sample-format" (type $sample-format0))
    (type $audio-format0 (record
      (field "sample-rate" u32)
      (field "channels" u16)
      (field "sample-format" $sample-format)))
    (export $audio-format "audio-format" (type $audio-format0))
    (type $packet-type0 (variant
      (case "raw-audio" $audio-format)
      (case "opus-audio")
      (case "text")
      (case "binary")
      (case "custom" string)
      (case "any")))
    (export $packet-type "packet-type" (type $packet-type0))
    (type $input-pin0 (record
      (field "name" string)
      (field "accepts-types" (list $packet-type))))
    (export $input-pin "input-pin" (type $input-pin0))
    (type $output-pin0 (record
      (field "name" string)
      (field "produces-type" $packet-type)))
    (export $output-pin "output-pin" (type $output-pin0))
    (type $node-metadata0 (record
      (field "kind" string)
      (field "inputs" (list $input-pin))
      (field "outputs" (list $output-pin))
      (field "param-schema" string)
      (field "categories" (list string))))
    (export $node-metadata "node-metadata" (type $node-metadata0))
    (type $custom-encoding0 (enum "json"))
    (export $custom-encoding "custom-encoding" (type $custom-encoding0))
    (type $custom-packet0 (record
      (field "type-id" string)
      (field "encoding" $custom-encoding)
      (field "data" string)))
    (export $custom-packet "custom-packet" (type $custom-packet0))
    (type $audio-frame0 (record
      (field "sample-rate" u32)
      (field "channels" u16)
      (field "samples" (list f32))))
    (export $audio-frame "audio-frame" (type $audio-frame0))
    (type $packet0 (variant
      (case "audio" $audio-frame)
      (case "text" string)
      (case "binary" (list u8))
      (case "custom" $custom-packet)))
    (export $packet "packet" (type $packet0))

    ;; The resource must be exported (named) before functions whose types
    ;; reference it can be exported under [constructor]/[method] names.
    (export $ni "node-instance" (type $node-instance))

    (func $metadata (result $node-metadata)
      (canon lift (core func $i "metadata") (memory $mem) (realloc $realloc)))
    (export "metadata" (func $metadata))
    (func $ctor (param "params" (option string)) (result (own $ni))
      (canon lift (core func $i "ctor") (memory $mem) (realloc $realloc)))
    (export "[constructor]node-instance" (func $ctor))
    (func $process
      (param "self" (borrow $ni))
      (param "input-pin" string)
      (param "packet" $packet)
      (result (result (error string)))
      (canon lift (core func $i "process") (memory $mem) (realloc $realloc)))
    (export "[method]node-instance.process" (func $process))
    (func $update-params
      (param "self" (borrow $ni))
      (param "params" (option string))
      (result (result (error string)))
      (canon lift (core func $i "update-params") (memory $mem) (realloc $realloc)))
    (export "[method]node-instance.update-params" (func $update-params))
    (func $cleanup (param "self" (borrow $ni))
      (canon lift (core func $i "cleanup")))
    (export "[method]node-instance.cleanup" (func $cleanup))
  )
  (instance $node (instantiate $inner))
  (export "streamkit:plugin/node@0.1.0" (instance $node))
)"#
    )
}

// node-metadata with empty strings/lists: 40 zero bytes at offset 64.
const OK_METADATA: &str = "(i32.const 64)";
const OK_CTOR: &str = "(call $rnew (i32.const 0))";
// Pointer to zeroed memory = ok-case of result<_, string>.
const OK_RESULT: &str = "(i32.const 0)";

const TEST_DEADLINE: Duration = Duration::from_millis(300);
// Generous wall-clock bound for the whole interruption to be observed; the
// actual interruption should happen shortly after TEST_DEADLINE.
const TEST_TIMEOUT: Duration = Duration::from_secs(15);
const WELL_BEHAVED_DEADLINE: Duration = Duration::from_secs(2);

fn load_runaway_plugin(
    ctor_body: &str,
    process_body: &str,
    update_params_body: &str,
    call_timeout: Duration,
) -> Box<dyn streamkit_core::ProcessorNode> {
    let runtime = PluginRuntime::new(PluginRuntimeConfig::default()).expect("runtime initializes");
    let dir = tempfile::TempDir::new().expect("temp dir creates");
    let path = dir.path().join("runaway.wasm");
    std::fs::write(&path, component_wat(OK_METADATA, ctor_body, process_body, update_params_body))
        .expect("fixture writes");
    let mut plugin = runtime.load_plugin(&path).expect("WAT fixture must load as a component");
    plugin.set_call_timeout(call_timeout);
    plugin.create_node(None).expect("node creates")
}

struct TestHarness {
    state_rx: mpsc::Receiver<NodeStateUpdate>,
    control_tx: mpsc::Sender<NodeControlMessage>,
    input_tx: mpsc::Sender<Packet>,
}

fn node_context() -> (NodeContext, TestHarness) {
    let (state_tx, state_rx) = mpsc::channel(32);
    let state_tx = streamkit_core::NodeStateSender::new(state_tx, 0);
    let (control_tx, control_rx) = mpsc::channel(16);
    let (routed_tx, routed_rx) = mpsc::channel(64);
    // Keep the output side open for the duration of the test.
    std::mem::forget(routed_rx);
    let (input_tx, input_rx) = mpsc::channel(16);

    let output_sender =
        OutputSender::new("runaway-test".to_string(), OutputRouting::Routed(routed_tx));

    let ctx = NodeContext {
        inputs: HashMap::from([("in".to_string(), input_rx)]),
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
        asset_root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    (ctx, TestHarness { state_rx, control_tx, input_tx })
}

async fn await_failed_state(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) -> String {
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) => {
                if let NodeState::Failed { reason } = update.state {
                    return reason;
                }
            },
            Ok(None) => panic!("state channel closed before Failed was observed"),
            Err(elapsed) => panic!("node never reached Failed within {TEST_TIMEOUT:?}: {elapsed}"),
        }
    }
}

async fn await_running_state(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, state_rx.recv()).await {
            Ok(Some(update)) => {
                if matches!(update.state, NodeState::Running) {
                    return;
                }
            },
            Ok(None) => panic!("state channel closed before Running was observed"),
            Err(elapsed) => {
                panic!("node never reached Running within {TEST_TIMEOUT:?}: {elapsed}")
            },
        }
    }
}

#[test]
fn runaway_metadata_is_interrupted_at_load_time() {
    let runtime = PluginRuntime::new(PluginRuntimeConfig {
        call_timeout: TEST_DEADLINE,
        ..PluginRuntimeConfig::default()
    })
    .expect("runtime initializes");
    let dir = tempfile::TempDir::new().expect("temp dir creates");
    let path = dir.path().join("runaway.wasm");
    std::fs::write(&path, component_wat(SPIN, OK_CTOR, OK_RESULT, OK_RESULT))
        .expect("fixture writes");

    let start = std::time::Instant::now();
    let Err(err) = runtime.load_plugin(&path) else {
        panic!("runaway metadata() must not load successfully")
    };
    assert!(start.elapsed() < TEST_TIMEOUT, "load_plugin took too long: {:?}", start.elapsed());
    let msg = format!("{err:#}");
    assert!(
        msg.contains("exceeded execution deadline"),
        "error must mention the deadline, got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runaway_constructor_is_interrupted_and_node_fails() {
    let node = load_runaway_plugin(SPIN, OK_RESULT, OK_RESULT, TEST_DEADLINE);
    let (ctx, mut harness) = node_context();

    let run = tokio::spawn(node.run(ctx));

    let reason = await_failed_state(&mut harness.state_rx).await;
    assert!(
        reason.contains("exceeded execution deadline"),
        "failure reason must mention the deadline, got: {reason}"
    );

    let result = tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("run task must complete after interruption")
        .expect("run task must not panic");
    assert!(matches!(result, Err(StreamKitError::Configuration(_))), "got: {result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runaway_process_is_interrupted_with_shutdown_queued() {
    let node = load_runaway_plugin(OK_CTOR, SPIN, OK_RESULT, TEST_DEADLINE);
    let (ctx, mut harness) = node_context();

    let run = tokio::spawn(node.run(ctx));
    await_running_state(&mut harness.state_rx).await;

    harness.input_tx.send(Packet::Text("spin".into())).await.expect("packet sends");
    // Wait until the node has dequeued the packet (it is then inside the
    // spinning process() call; the biased select would otherwise observe
    // Shutdown before the packet), then verify a queued Shutdown cannot be
    // starved forever by the runaway guest.
    while harness.input_tx.capacity() < harness.input_tx.max_capacity() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    harness.control_tx.send(NodeControlMessage::Shutdown).await.expect("shutdown sends");

    let reason = await_failed_state(&mut harness.state_rx).await;
    assert!(
        reason.contains("exceeded execution deadline"),
        "failure reason must mention the deadline, got: {reason}"
    );

    let result = tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("run task must complete after interruption")
        .expect("run task must not panic");
    assert!(matches!(result, Err(StreamKitError::Runtime(_))), "got: {result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runaway_update_params_is_interrupted_and_node_fails() {
    let node = load_runaway_plugin(OK_CTOR, OK_RESULT, SPIN, TEST_DEADLINE);
    let (ctx, mut harness) = node_context();

    let run = tokio::spawn(node.run(ctx));
    await_running_state(&mut harness.state_rx).await;

    harness
        .control_tx
        .send(NodeControlMessage::UpdateParams(serde_json::json!({"x": 1})))
        .await
        .expect("update sends");

    let reason = await_failed_state(&mut harness.state_rx).await;
    assert!(
        reason.contains("exceeded execution deadline"),
        "failure reason must mention the deadline, got: {reason}"
    );

    let result = tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("run task must complete after interruption")
        .expect("run task must not panic");
    assert!(matches!(result, Err(StreamKitError::Configuration(_))), "got: {result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn well_behaved_guest_is_not_interrupted() {
    // A generous deadline keeps slow-CI instantiation from tripping it; the
    // idle sleep below still exceeds it to prove the deadline is per-call.
    let node = load_runaway_plugin(OK_CTOR, OK_RESULT, OK_RESULT, WELL_BEHAVED_DEADLINE);
    let (ctx, mut harness) = node_context();

    let run = tokio::spawn(node.run(ctx));
    await_running_state(&mut harness.state_rx).await;

    harness.input_tx.send(Packet::Text("ok".into())).await.expect("packet sends");
    // Stay idle past the deadline, proving the deadline is per-call and idle
    // time between calls is exempt.
    tokio::time::sleep(WELL_BEHAVED_DEADLINE + Duration::from_millis(500)).await;

    harness.control_tx.send(NodeControlMessage::Shutdown).await.expect("shutdown sends");
    let result = tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("run task must complete after shutdown")
        .expect("run task must not panic");
    assert!(result.is_ok(), "well-behaved guest must shut down cleanly, got: {result:?}");
}
