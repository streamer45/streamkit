// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Coverage for the four "node-emits-update" handlers in
//! `dynamic_actor.rs`:
//!
//!   * `handle_state_update`     (line 425+)
//!   * `handle_telemetry_event`  (line 471+)
//!   * `handle_view_data_update` (line 479+)
//!   * `handle_stats_update`     (line 506+)
//!
//! The happy-path tests in `async_node_creation.rs` only exercise the
//! control-plane (AddNode / Connect / Tune); they do not run a node that
//! actually emits state / telemetry / stats / view_data, which is what
//! these handlers process. This file boots a `DynamicEngine` with a single
//! `EmittingNode` that pushes one of each update kind, then asserts the
//! handle's subscriber streams receive them.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// setup failures (channel sends, subscription failures). No production code.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use streamkit_core::control::{EngineControlMessage, NodeControlMessage};
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::types::PacketType;
use streamkit_core::view_data::NodeViewDataUpdate;

use crate::{DynamicEngineConfig, DynamicEngineHandle, Engine};

/// A node that, on the first `Start` control message, emits one
/// `NodeStateUpdate`, one `TelemetryEvent`, one `NodeStatsUpdate` and one
/// `NodeViewDataUpdate`, then waits for `Shutdown`.
struct EmittingNode;

#[streamkit_core::async_trait]
impl ProcessorNode for EmittingNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut ctx: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        const NODE_ID: &str = "emitter";

        // The engine only sends `Start` once every node reports `Ready`, so
        // we must self-promote to Ready before waiting for Start.
        let _ =
            ctx.state_tx.send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Ready)).await;

        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Start) => break,
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(_) => {},
            }
        }

        let _ =
            ctx.state_tx.send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Running)).await;

        if let Some(tx) = ctx.telemetry_tx.as_ref() {
            let event = TelemetryEvent::new(
                None,
                NODE_ID.to_string(),
                serde_json::json!({"event_type": "test_event"}),
                0,
            );
            let _ = tx.send(event).await;
        }

        if let Some(tx) = ctx.stats_tx.as_ref() {
            let update = NodeStatsUpdate {
                node_id: NODE_ID.to_string(),
                stats: NodeStats {
                    received: 1,
                    sent: 2,
                    discarded: 0,
                    errored: 0,
                    duration_secs: 1.0,
                },
                timestamp: SystemTime::now(),
            };
            let _ = tx.send(update).await;
        }

        if let Some(tx) = ctx.view_data_tx.as_ref() {
            let update = NodeViewDataUpdate {
                node_id: NODE_ID.to_string(),
                data: serde_json::json!({"frame": 1}),
                timestamp: SystemTime::now(),
            };
            let _ = tx.send(update).await;
        }

        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(_) => {},
            }
        }
    }
}

fn build_handle() -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::emitter",
        |_p| Ok(Box::new(EmittingNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    engine.start_dynamic_actor(DynamicEngineConfig::default())
}

async fn add_emitter_and_wait_ready(handle: &DynamicEngineHandle) {
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "emitter".to_string(),
            kind: "test::emitter".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            match states.get("emitter") {
                Some(NodeState::Ready | NodeState::Running) => return,
                Some(NodeState::Failed { reason }) => {
                    panic!("emitter transitioned to Failed: {reason}");
                },
                _ => {},
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("emitter did not reach Ready/Running in time");
}

#[tokio::test]
async fn state_update_from_node_reaches_subscriber() {
    let handle = build_handle();
    let mut sub = handle.subscribe_state().await.expect("subscribe_state");
    add_emitter_and_wait_ready(&handle).await;

    // Consume updates until we observe the EmittingNode's `Running` state,
    // which the node itself pushes after receiving Start. Engine-emitted
    // lifecycle states (Creating/Ready) for the same node id arrive first;
    // ignore them so a regression where node-emitted updates are dropped
    // is actually detectable.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_running = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
            Ok(Some(u)) if u.node_id == "emitter" && matches!(u.state, NodeState::Running) => {
                saw_running = true;
                break;
            },
            Ok(Some(_)) | Err(_) => {},
            Ok(None) => break,
        }
    }
    assert!(saw_running, "node-emitted Running state update should reach subscriber");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn telemetry_event_from_node_reaches_subscriber() {
    let handle = build_handle();
    let mut sub = handle.subscribe_telemetry().await.expect("subscribe_telemetry");
    add_emitter_and_wait_ready(&handle).await;

    let got = tokio::time::timeout(Duration::from_secs(3), sub.recv())
        .await
        .expect("telemetry event should arrive")
        .expect("subscriber channel still open");

    assert_eq!(got.node_id, "emitter");
    assert_eq!(got.event_type(), Some("test_event"));

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn stats_update_from_node_reaches_subscriber() {
    let handle = build_handle();
    let mut sub = handle.subscribe_stats().await.expect("subscribe_stats");
    add_emitter_and_wait_ready(&handle).await;

    // Receive until we see our emitter update; ignore engine-internal
    // updates that may arrive first.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
            Ok(Some(u)) if u.node_id == "emitter" => {
                assert_eq!(u.stats.received, 1);
                assert_eq!(u.stats.sent, 2);
                saw = true;
                break;
            },
            Ok(Some(_)) | Err(_) => {},
            Ok(None) => break,
        }
    }
    assert!(saw, "stats update from emitter should reach subscriber");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn view_data_update_from_node_reaches_subscriber() {
    let handle = build_handle();
    let mut sub = handle.subscribe_view_data().await.expect("subscribe_view_data");
    add_emitter_and_wait_ready(&handle).await;

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
            Ok(Some(u)) if u.node_id == "emitter" => {
                assert_eq!(u.data["frame"], 1);
                saw = true;
                break;
            },
            Ok(Some(_)) | Err(_) => {},
            Ok(None) => break,
        }
    }
    assert!(saw, "view data update from emitter should reach subscriber");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn get_node_view_data_returns_emitter_payload() {
    let handle = build_handle();
    add_emitter_and_wait_ready(&handle).await;

    // The actor only stores view data updates for live nodes; allow the
    // EmittingNode time to emit after receiving Start.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = serde_json::Value::Null;
    while Instant::now() < deadline {
        if let Ok(map) = handle.get_node_view_data().await {
            if let Some(v) = map.get("emitter") {
                got = v.clone();
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(got["frame"], 1);

    handle.shutdown_and_wait().await.expect("shutdown");
}

/// A node that promotes itself to `Ready`, waits for `Start`, then returns
/// `Err` WITHOUT emitting any terminal state — modelling a worker that dies
/// and whose best-effort `Failed` `try_send` was dropped under backpressure.
struct DyingNode;

#[streamkit_core::async_trait]
impl ProcessorNode for DyingNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut ctx: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        const NODE_ID: &str = "dier";

        let _ =
            ctx.state_tx.send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Ready)).await;

        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Start) => break,
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(_) => {},
            }
        }

        Err(StreamKitError::Runtime("simulated worker death".to_string()))
    }
}

fn build_dying_handle() -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::dier",
        |_p| Ok(Box::new(DyingNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    engine.start_dynamic_actor(DynamicEngineConfig::default())
}

/// Regression for #570: a node task that returns `Err` must surface a terminal
/// `Failed` to subscribers even when the node itself emitted no terminal state
/// (the worker died and the best-effort notification was lost). The actor
/// reconciles the task result onto the state channel as a backstop.
#[tokio::test]
async fn node_task_error_surfaces_failed_without_node_emitting_it() {
    let handle = build_dying_handle();
    let mut sub = handle.subscribe_state().await.expect("subscribe_state");
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "dier".to_string(),
            kind: "test::dier".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_failed = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), sub.recv()).await {
            Ok(Some(u)) if u.node_id == "dier" && matches!(u.state, NodeState::Failed { .. }) => {
                saw_failed = true;
                break;
            },
            Ok(Some(_)) | Err(_) => {},
            Ok(None) => break,
        }
    }
    assert!(saw_failed, "actor must surface Failed from the node task's Err result");

    handle.shutdown_and_wait().await.expect("shutdown");
}

/// A node that, on `Shutdown`, saturates the shared state channel with
/// `try_send` so the wrapper's terminal backstop send (initialize_node) lands
/// on a full channel.
struct FloodOnShutdownNode;

#[streamkit_core::async_trait]
impl ProcessorNode for FloodOnShutdownNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    async fn run(
        self: Box<Self>,
        mut ctx: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        const NODE_ID: &str = "flooder";

        let _ =
            ctx.state_tx.send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Ready)).await;

        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Shutdown) | None => break,
                Some(_) => {},
            }
        }

        while ctx
            .state_tx
            .try_send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Running))
            .is_ok()
        {}

        Ok(())
    }
}

fn build_flood_handle() -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::flooder",
        |_p| Ok(Box::new(FloodOnShutdownNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    engine.start_dynamic_actor(DynamicEngineConfig::default())
}

/// Regression: the node-task terminal backstop must not stall shutdown. The
/// actor stops draining the state channel while it joins node tasks, so a
/// task whose backstop send hits a full channel would block until the 2s join
/// timeout aborts it. The actor closes the state receiver on shutdown so the
/// send fails fast; assert teardown finishes well under that timeout.
#[tokio::test]
async fn shutdown_does_not_stall_on_saturated_state_channel() {
    let handle = build_flood_handle();
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "flooder".to_string(),
            kind: "test::flooder".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(states) = handle.get_node_states().await {
            if matches!(states.get("flooder"), Some(NodeState::Ready | NodeState::Running)) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "flooder did not reach Ready in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let start = Instant::now();
    handle.shutdown_and_wait().await.expect("shutdown");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "shutdown stalled on a saturated state channel: {elapsed:?}"
    );
}
