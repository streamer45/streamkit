// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Tests for the public `DynamicEngineHandle` API surface.
//!
//! These exercise each subscribe / getter / shutdown path with an
//! observable effect to keep function coverage above 80%.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// setup failures; the production API distinguishes `get_node_states` vs.
// `get_node_stats` by one letter, so `similar_names` triggers spuriously
// when both are bound in the same test.
#![allow(clippy::expect_used, clippy::similar_names)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use streamkit_core::control::EngineControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::NodeState;
use streamkit_core::types::PacketType;
use tokio::sync::mpsc::error::TryRecvError;

use crate::{DynamicEngineConfig, DynamicEngineHandle, Engine};

struct EchoNode;

#[streamkit_core::async_trait]
impl ProcessorNode for EchoNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        vec![streamkit_core::InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: streamkit_core::PinCardinality::One,
        }]
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
        loop {
            match ctx.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                _ => {},
            }
        }
    }
}

fn make_handle(counter: Arc<AtomicU32>) -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::echo",
        move |_p| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(EchoNode) as Box<dyn ProcessorNode>)
        },
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

async fn add_node_and_wait_ready(handle: &DynamicEngineHandle, name: &str) {
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: name.to_string(),
            kind: "test::echo".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            match states.get(name) {
                Some(NodeState::Initializing | NodeState::Ready | NodeState::Running) => return,
                Some(NodeState::Failed { reason }) => {
                    panic!("node '{name}' transitioned to Failed: {reason}");
                },
                _ => {},
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("node '{name}' did not reach a live state in time");
}

#[tokio::test]
async fn handle_get_node_states_and_stats_reflect_added_node() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter.clone());

    add_node_and_wait_ready(&handle, "echo1").await;

    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.contains_key("echo1"), "states should include the new node");

    // The actor registers every live node in `node_stats` (with zeroed
    // counters) when it transitions to Ready. Assert presence; counter
    // values are exercised by the metrics-recorder test in `oneshot.rs`.
    let stats = handle.get_node_stats().await.expect("get_node_stats");
    let echo_stats = stats.get("echo1").expect("stats map must contain the added node");
    assert_eq!(echo_stats.sent, 0, "new node should report zero sent packets");
    assert_eq!(echo_stats.received, 0, "new node should report zero received packets");

    assert_eq!(counter.load(Ordering::SeqCst), 1, "factory should have run once");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_subscribe_state_yields_updates() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut state_rx = handle.subscribe_state().await.expect("subscribe_state");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "watched".to_string(),
            kind: "test::echo".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let mut saw_update = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), state_rx.recv()).await {
            Ok(Some(update)) if update.node_id == "watched" => {
                saw_update = true;
                break;
            },
            Ok(Some(_)) | Err(_) => {},
            Ok(None) => panic!("state subscription closed unexpectedly"),
        }
    }
    assert!(saw_update, "expected at least one state update for the added node");

    handle.shutdown_and_wait().await.expect("shutdown");
}

// For the three subscription channels that the test EchoNode does NOT
// itself emit on (stats / telemetry / view_data), we cannot easily force
// an update without spinning up a richer node. Instead, assert the
// returned receiver is *wired up*: `try_recv` returns `Empty` (channel
// open, no message) rather than `Disconnected` (channel never connected
// to a live sender). This catches a regression where `subscribe_*` returns
// a stub receiver disconnected from the actor's fan-out.
#[tokio::test]
async fn handle_subscribe_stats_returns_live_receiver() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut stats_rx = handle.subscribe_stats().await.expect("subscribe_stats");

    assert!(
        matches!(stats_rx.try_recv(), Err(TryRecvError::Empty)),
        "stats receiver must be open (Empty), not disconnected"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_subscribe_telemetry_returns_live_receiver() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut telemetry_rx = handle.subscribe_telemetry().await.expect("subscribe_telemetry");

    assert!(
        matches!(telemetry_rx.try_recv(), Err(TryRecvError::Empty)),
        "telemetry receiver must be open (Empty), not disconnected"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_subscribe_view_data_returns_live_receiver() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut view_rx = handle.subscribe_view_data().await.expect("subscribe_view_data");

    assert!(
        matches!(view_rx.try_recv(), Err(TryRecvError::Empty)),
        "view-data receiver must be open (Empty), not disconnected"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_get_node_view_data_returns_map() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let view = handle.get_node_view_data().await.expect("get_node_view_data");
    assert!(view.is_empty(), "no nodes have view data before any are added");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_get_runtime_schemas_returns_map() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let schemas = handle.get_runtime_schemas().await.expect("get_runtime_schemas");
    assert!(schemas.is_empty(), "no nodes report runtime schemas before any are added");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_subscribe_runtime_schemas_returns_live_receiver() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut schema_rx =
        handle.subscribe_runtime_schemas().await.expect("subscribe_runtime_schemas");

    assert!(
        matches!(schema_rx.try_recv(), Err(TryRecvError::Empty)),
        "runtime-schemas receiver must be open (Empty), not disconnected"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_subscribe_node_lifecycle_yields_added_then_removed() {
    use crate::dynamic_messages::NodeLifecycleNotification;

    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    let mut lifecycle_rx =
        handle.subscribe_node_lifecycle().await.expect("subscribe_node_lifecycle");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "subscribed".to_string(),
            kind: "test::echo".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let Ok(Some(NodeLifecycleNotification::Added(added))) =
        tokio::time::timeout(Duration::from_secs(3), lifecycle_rx.recv()).await
    else {
        panic!("expected node-added notification within 3s");
    };
    assert_eq!(added.node_id, "subscribed");
    assert_eq!(added.kind, "test::echo");

    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "subscribed".to_string() })
        .await
        .expect("send RemoveNode");

    let Ok(Some(NodeLifecycleNotification::Removed(removed))) =
        tokio::time::timeout(Duration::from_secs(3), lifecycle_rx.recv()).await
    else {
        panic!("expected node-removed notification within 3s");
    };
    assert_eq!(removed.node_id, "subscribed");
    assert_eq!(removed.generation, added.generation);

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn handle_send_control_after_shutdown_returns_error() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    handle.shutdown_and_wait().await.expect("shutdown");

    // After shutdown completes, send_control may still succeed momentarily
    // (channel close lag), so poll until it fails or we hit a deadline.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut last_result = Ok(());
    while std::time::Instant::now() < deadline {
        last_result = handle
            .send_control(EngineControlMessage::AddNode {
                node_id: "post".to_string(),
                kind: "test::echo".to_string(),
                params: None,
            })
            .await;
        if last_result.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        last_result.is_err(),
        "send_control must eventually surface an error once the actor has shut down"
    );
}

// A second `shutdown_and_wait` after the first one consumed the join
// handle today returns Err from the inner `send_control(Shutdown).await?`
// because the actor's control channel is already closed. The function's
// `else` branch (which would return Ok(())) is therefore unreachable from
// here. We pin the *actual* observable behavior: the second call must
// return cleanly with an Err (no panic, no hang), so a future refactor
// that hangs the second call is caught.
#[tokio::test]
async fn handle_shutdown_and_wait_second_call_returns_clean_err() {
    let counter = Arc::new(AtomicU32::new(0));
    let handle = make_handle(counter);

    handle.shutdown_and_wait().await.expect("first shutdown");

    let second = tokio::time::timeout(Duration::from_secs(2), handle.shutdown_and_wait())
        .await
        .expect("second shutdown_and_wait must not hang");
    assert!(
        second.is_err(),
        "second shutdown_and_wait must surface a clean Err (control channel closed), got {second:?}"
    );
}
