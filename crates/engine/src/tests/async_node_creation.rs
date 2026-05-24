// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Tests for async (non-blocking) node creation in the dynamic engine.
//!
//! Validates that `AddNode` no longer blocks the actor loop: node constructors
//! run inside `spawn_blocking`, connections are deferred while endpoints are
//! `Creating`, and edge cases (cancellation, failure, shutdown) are handled.

use super::super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::state::NodeState;
use streamkit_core::{NodeRegistry, ProcessorNode, StreamKitError};

/// Simulates heavy FFI work (e.g., ONNX model loading) via `std::thread::sleep`
/// inside `spawn_blocking`.
struct SlowTestNode;

impl SlowTestNode {
    fn factory(
        delay: Duration,
        created: Arc<AtomicBool>,
    ) -> impl Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
           + Send
           + Sync
           + 'static {
        move |_params| {
            std::thread::sleep(delay);
            created.store(true, Ordering::SeqCst);
            Ok(Box::new(Self) as Box<dyn ProcessorNode>)
        }
    }
}

#[streamkit_core::async_trait]
impl ProcessorNode for SlowTestNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        vec![streamkit_core::InputPin {
            name: "in".to_string(),
            accepts_types: vec![streamkit_core::types::PacketType::Any],
            cardinality: streamkit_core::PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: streamkit_core::types::PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut context: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(
                    streamkit_core::control::NodeControlMessage::Start
                    | streamkit_core::control::NodeControlMessage::UpdateParams(_),
                ) => {},
            }
        }
    }
}

/// Records `UpdateParams` messages to verify deferred-tune replay.
struct TuneTrackingSlowNode {
    tune_count: Arc<AtomicU32>,
}

impl TuneTrackingSlowNode {
    fn factory(
        delay: Duration,
        created: Arc<AtomicBool>,
        tune_count: Arc<AtomicU32>,
    ) -> impl Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
           + Send
           + Sync
           + 'static {
        move |_params| {
            std::thread::sleep(delay);
            created.store(true, Ordering::SeqCst);
            Ok(Box::new(Self { tune_count: tune_count.clone() }) as Box<dyn ProcessorNode>)
        }
    }
}

#[streamkit_core::async_trait]
impl ProcessorNode for TuneTrackingSlowNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        vec![streamkit_core::InputPin {
            name: "in".to_string(),
            accepts_types: vec![streamkit_core::types::PacketType::Any],
            cardinality: streamkit_core::PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: streamkit_core::types::PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut context: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(streamkit_core::control::NodeControlMessage::UpdateParams(_)) => {
                    self.tune_count.fetch_add(1, Ordering::SeqCst);
                },
                Some(streamkit_core::control::NodeControlMessage::Start) => {},
            }
        }
    }
}

/// A simple source node (no inputs) that stays alive until shutdown.
struct SimpleSourceNode;

#[streamkit_core::async_trait]
impl ProcessorNode for SimpleSourceNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }

    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: streamkit_core::types::PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut context: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(
                    streamkit_core::control::NodeControlMessage::Start
                    | streamkit_core::control::NodeControlMessage::UpdateParams(_),
                ) => {},
            }
        }
    }
}

/// A node whose constructor always fails with an error.
struct FailingConstructorNode;

impl FailingConstructorNode {
    fn factory(
    ) -> impl Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
           + Send
           + Sync
           + 'static {
        |_params| {
            std::thread::sleep(Duration::from_millis(100));
            Err(StreamKitError::Runtime("Model loading failed: out of memory".to_string()))
        }
    }
}

/// A fast node whose constructor records the creation count (for concurrency tests).
struct FastTestNode;

impl FastTestNode {
    fn factory(
        counter: Arc<AtomicU32>,
    ) -> impl Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
           + Send
           + Sync
           + 'static {
        move |_params| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Self) as Box<dyn ProcessorNode>)
        }
    }
}

#[streamkit_core::async_trait]
impl ProcessorNode for FastTestNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        vec![streamkit_core::InputPin {
            name: "in".to_string(),
            accepts_types: vec![streamkit_core::types::PacketType::Any],
            cardinality: streamkit_core::PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: streamkit_core::types::PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut context: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(
                    streamkit_core::control::NodeControlMessage::Start
                    | streamkit_core::control::NodeControlMessage::UpdateParams(_),
                ) => {},
            }
        }
    }
}

fn build_engine(registry: NodeRegistry) -> (Engine, DynamicEngineHandle) {
    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());
    (engine, handle)
}

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

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_basic_async_creation() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_secs(1), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let fast_counter = Arc::new(AtomicU32::new(0));
    registry.register_dynamic(
        "test::fast",
        FastTestNode::factory(fast_counter.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add slow node first, then fast node immediately after.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "slow".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode slow");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "fast".to_string(),
            kind: "test::fast".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode fast");

    // The fast node should become available (past Creating) well before the
    // slow node finishes its 1-second sleep.
    let fast_ready = wait_for_states(&handle, Duration::from_secs(3), |states| {
        states.get("fast").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(fast_ready, "fast node should leave Creating before slow node finishes");

    // At this point the slow node should still be Creating (or just finishing).
    // Wait for it to also complete.
    let slow_ready = wait_for_states(&handle, Duration::from_secs(5), |states| {
        states.get("slow").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(slow_ready, "slow node should eventually leave Creating");
    assert!(slow_created.load(Ordering::SeqCst), "slow constructor should have run");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_deferred_connections() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::source",
        |_params| Ok(Box::new(SimpleSourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_millis(500), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add both nodes.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::source".to_string(),
            params: None,
        })
        .await
        .expect("add source");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "slow".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add slow");

    // Connect immediately — slow node is still Creating.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "slow".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // After slow node finishes, both should be initialized and the deferred
    // connection should have been replayed.
    let both_ready = wait_for_states(&handle, Duration::from_secs(5), |states| {
        let src_ok = states.get("src").is_some_and(|s| {
            matches!(s, NodeState::Ready | NodeState::Running | NodeState::Initializing)
        });
        let slow_ok = states.get("slow").is_some_and(|s| {
            matches!(s, NodeState::Ready | NodeState::Running | NodeState::Initializing)
        });
        src_ok && slow_ok
    })
    .await;
    assert!(both_ready, "both nodes should be initialized after deferred connection replay");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_multiple_slow_nodes_concurrent() {
    let created_a = Arc::new(AtomicBool::new(false));
    let created_b = Arc::new(AtomicBool::new(false));
    let created_c = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow_a",
        SlowTestNode::factory(Duration::from_millis(500), created_a.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow_b",
        SlowTestNode::factory(Duration::from_millis(500), created_b.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow_c",
        SlowTestNode::factory(Duration::from_millis(500), created_c.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    let start = Instant::now();

    for (id, kind) in [("a", "test::slow_a"), ("b", "test::slow_b"), ("c", "test::slow_c")] {
        handle
            .send_control(EngineControlMessage::AddNode {
                node_id: id.to_string(),
                kind: kind.to_string(),
                params: None,
            })
            .await
            .expect("add node");
    }

    // Wait for all three to leave Creating.
    let all_done = wait_for_states(&handle, Duration::from_secs(5), |states| {
        ["a", "b", "c"]
            .iter()
            .all(|id| states.get(*id).is_some_and(|s| !matches!(s, NodeState::Creating)))
    })
    .await;
    assert!(all_done, "all three slow nodes should finish creation");

    let elapsed = start.elapsed();
    // If sequential, ~1.5s; if concurrent, ~0.5s + overhead.
    assert!(
        elapsed < Duration::from_millis(1200),
        "3 x 500ms nodes should complete in ~500ms (concurrent), but took {elapsed:?}",
    );

    assert!(created_a.load(Ordering::SeqCst));
    assert!(created_b.load(Ordering::SeqCst));
    assert!(created_c.load(Ordering::SeqCst));

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_creation_failure() {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::source",
        |_params| Ok(Box::new(SimpleSourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::failing",
        FailingConstructorNode::factory(),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::source".to_string(),
            params: None,
        })
        .await
        .expect("add source");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "bad".to_string(),
            kind: "test::failing".to_string(),
            params: None,
        })
        .await
        .expect("add failing");

    // Queue a connection to the failing node.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "bad".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // The failing node should transition to Failed.
    let failed = wait_for_states(&handle, Duration::from_secs(3), |states| {
        matches!(states.get("bad"), Some(NodeState::Failed { .. }))
    })
    .await;
    assert!(failed, "failing node should transition to Failed");

    // Source node should still be fine.
    let states = handle.get_node_states().await.expect("get states");
    assert!(
        states.get("src").is_some_and(|s| !matches!(s, NodeState::Failed { .. })),
        "source node should be unaffected"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_remove_node_while_creating() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_secs(1), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add slow node.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "doomed".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add doomed");

    // Give the actor a moment to process AddNode and set Creating state.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Remove it while still Creating.
    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "doomed".to_string() })
        .await
        .expect("remove doomed");

    // Wait for the background creation to complete (1s), then verify
    // the node was NOT added to the engine.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let states = handle.get_node_states().await.expect("get states");
    assert!(
        !states.contains_key("doomed"),
        "removed-while-Creating node should not appear in states"
    );

    // The constructor did run (it was already spawned), but the result
    // should have been discarded.
    assert!(
        slow_created.load(Ordering::SeqCst),
        "constructor runs to completion even if cancelled"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_pipeline_activation_timing() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::source",
        |_params| Ok(Box::new(SimpleSourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_millis(800), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Subscribe to state updates to observe activation.
    let mut state_rx = handle.subscribe_state().await.expect("subscribe");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::source".to_string(),
            params: None,
        })
        .await
        .expect("add source");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "proc".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add slow processor");

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "proc".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // Drain state updates; verify source doesn't go to Running before slow node
    // leaves Creating.
    let mut slow_left_creating = false;
    let mut src_ran_before_slow_ready = false;

    let drain_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < drain_deadline {
        match tokio::time::timeout(Duration::from_millis(100), state_rx.recv()).await {
            Ok(Some(update)) => {
                if update.node_id == "proc" && !matches!(update.state, NodeState::Creating) {
                    slow_left_creating = true;
                }
                if update.node_id == "src" && matches!(update.state, NodeState::Running) {
                    if !slow_left_creating {
                        src_ran_before_slow_ready = true;
                    }
                    break;
                }
            },
            _ => {
                if slow_left_creating {
                    break;
                }
            },
        }
    }

    assert!(
        !src_ran_before_slow_ready,
        "source should NOT start Running before slow node leaves Creating"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_duplicate_add_node() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_millis(500), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add node.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "dup".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add first");

    // Give actor time to process.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Try adding the same node_id again.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "dup".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add duplicate");

    // Wait for the original to finish.
    let done = wait_for_states(&handle, Duration::from_secs(3), |states| {
        states.get("dup").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(done, "original node should finish creating");

    // The engine should still be responsive (no double-init crash).
    let states = handle.get_node_states().await.expect("get states");
    assert!(states.contains_key("dup"), "node should exist");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_remove_then_readd_same_id() {
    let created_v1 = Arc::new(AtomicBool::new(false));
    let created_v2 = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow_v1",
        SlowTestNode::factory(Duration::from_secs(1), created_v1.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::fast_v2",
        SlowTestNode::factory(Duration::from_millis(50), created_v2.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add slow v1.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "node".to_string(),
            kind: "test::slow_v1".to_string(),
            params: None,
        })
        .await
        .expect("add v1");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Remove while Creating.
    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "node".to_string() })
        .await
        .expect("remove");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Re-add with a different (fast) kind.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "node".to_string(),
            kind: "test::fast_v2".to_string(),
            params: None,
        })
        .await
        .expect("add v2");

    // Wait for v2 to finish.
    let v2_done = wait_for_states(&handle, Duration::from_secs(3), |states| {
        states.get("node").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(v2_done, "v2 node should finish creating");

    // v2 should have been created.
    assert!(created_v2.load(Ordering::SeqCst), "v2 constructor should have run");

    // Wait for v1 background task to also complete (it was already spawned).
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(created_v1.load(Ordering::SeqCst), "v1 constructor runs to completion");

    // The node should be the v2 version — verify it's not in Creating or
    // Failed state (it should be fully initialized).
    let states = handle.get_node_states().await.expect("get states");
    assert!(states.contains_key("node"), "node should exist");
    let state = states.get("node").expect("node state");
    assert!(
        !matches!(state, NodeState::Creating | NodeState::Failed { .. }),
        "v2 node should be initialized, not Creating/Failed, got: {state:?}"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_shutdown_while_creating() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_secs(2), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "slow".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add slow");

    // Give actor time to set Creating state.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Shutdown while slow node is still Creating.
    let result = handle.shutdown_and_wait().await;
    assert!(result.is_ok(), "shutdown should complete cleanly: {result:?}");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_connect_one_realized_one_creating() {
    let slow_created = Arc::new(AtomicBool::new(false));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::source",
        |_params| Ok(Box::new(SimpleSourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow",
        SlowTestNode::factory(Duration::from_millis(500), slow_created.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add source (fast) — it will be realized quickly.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "source".to_string(),
            kind: "test::source".to_string(),
            params: None,
        })
        .await
        .expect("add source");

    // Wait for source to leave Creating.
    let source_ready = wait_for_states(&handle, Duration::from_secs(2), |states| {
        states.get("source").is_some_and(|s| !matches!(s, NodeState::Creating))
    })
    .await;
    assert!(source_ready, "source should be realized quickly");

    // Now add slow node.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "slow".to_string(),
            kind: "test::slow".to_string(),
            params: None,
        })
        .await
        .expect("add slow");

    // Connect while source is realized but slow is still Creating.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "source".to_string(),
            from_pin: "out".to_string(),
            to_node: "slow".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // Wait for slow node to finish and verify both are initialized.
    let both_done = wait_for_states(&handle, Duration::from_secs(5), |states| {
        let source_ok = states.get("source").is_some_and(|s| {
            matches!(s, NodeState::Ready | NodeState::Running | NodeState::Initializing)
        });
        let slow_ok = states.get("slow").is_some_and(|s| {
            matches!(s, NodeState::Ready | NodeState::Running | NodeState::Initializing)
        });
        source_ok && slow_ok
    })
    .await;
    assert!(both_done, "both nodes should be initialized after deferred connection is replayed");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_tune_node_queued_while_creating() {
    let created = Arc::new(AtomicBool::new(false));
    let tune_count = Arc::new(AtomicU32::new(0));

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::tune_tracking_slow",
        TuneTrackingSlowNode::factory(Duration::from_secs(1), created.clone(), tune_count.clone()),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let (_engine, handle) = build_engine(registry);

    // Add the slow node.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "tracked".to_string(),
            kind: "test::tune_tracking_slow".to_string(),
            params: None,
        })
        .await
        .expect("add tracked");

    // Verify it's still Creating (constructor sleeps 1s).
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!created.load(Ordering::SeqCst), "node should still be creating");

    // Send two TuneNode messages while the node is Creating.
    handle
        .send_control(EngineControlMessage::TuneNode {
            node_id: "tracked".to_string(),
            message: streamkit_core::control::NodeControlMessage::UpdateParams(
                serde_json::json!({"gain": 0.5}),
            ),
        })
        .await
        .expect("tune 1");

    handle
        .send_control(EngineControlMessage::TuneNode {
            node_id: "tracked".to_string(),
            message: streamkit_core::control::NodeControlMessage::UpdateParams(
                serde_json::json!({"gain": 0.8}),
            ),
        })
        .await
        .expect("tune 2");

    // Wait for the node to finish creation and initialization.
    let initialized = wait_for_states(&handle, Duration::from_secs(5), |states| {
        states.get("tracked").is_some_and(|s| {
            matches!(s, NodeState::Ready | NodeState::Running | NodeState::Initializing)
        })
    })
    .await;
    assert!(initialized, "node should be initialized");

    // Give a moment for the queued tunes to be replayed and processed.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Both UpdateParams messages should have been delivered.
    assert_eq!(
        tune_count.load(Ordering::SeqCst),
        2,
        "node should have received both queued TuneNode messages"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}
