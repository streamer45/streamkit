// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Coverage for control-plane edge cases in `dynamic_actor.rs` that the
//! happy-path tests in `async_node_creation.rs` do not exercise:
//!
//! - Connect / Disconnect / TuneNode for unknown endpoints.
//! - Full lifecycle: AddNode → wait Ready → RemoveNode (exercises the
//!   `shutdown_node` path, not just RemoveNode-while-Creating).
//! - Engine-level Shutdown drains live nodes, distributors, and state.

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use streamkit_core::control::{EngineControlMessage, NodeControlMessage};
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::NodeState;
use streamkit_core::types::PacketType;

use crate::{DynamicEngineConfig, DynamicEngineHandle, Engine};

struct IdleNode;

#[streamkit_core::async_trait]
impl ProcessorNode for IdleNode {
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
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                _ => {},
            }
        }
    }
}

fn build_handle() -> (Arc<AtomicU32>, DynamicEngineHandle) {
    let counter = Arc::new(AtomicU32::new(0));
    let factory_counter = counter.clone();
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::idle",
        move |_p| {
            factory_counter.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(IdleNode) as Box<dyn ProcessorNode>)
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
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());
    (counter, handle)
}

async fn add_and_wait(handle: &DynamicEngineHandle, name: &str) {
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: name.to_string(),
            kind: "test::idle".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if states.get(name).is_some_and(|s| !matches!(s, NodeState::Creating)) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("node '{name}' did not leave Creating in time");
}

#[tokio::test]
async fn connect_with_unknown_endpoints_is_silent_noop() {
    let (_counter, handle) = build_handle();

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "ghost_from".to_string(),
            from_pin: "out".to_string(),
            to_node: "ghost_to".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("send Connect");

    // After a Connect to two non-existent nodes, the actor must still be
    // responsive (no state mutation, no panic).
    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.is_empty(), "no nodes should exist; got {states:?}");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn disconnect_with_unknown_endpoints_is_silent_noop() {
    let (_counter, handle) = build_handle();
    add_and_wait(&handle, "real").await;

    handle
        .send_control(EngineControlMessage::Disconnect {
            from_node: "real".to_string(),
            from_pin: "out".to_string(),
            to_node: "ghost".to_string(),
            to_pin: "in".to_string(),
        })
        .await
        .expect("send Disconnect");

    // Real node must still be in node_states after attempting to disconnect
    // a non-existent endpoint - the actor must not collapse on this.
    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.contains_key("real"), "live node should still be present");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn tune_unknown_node_is_silent_noop() {
    let (_counter, handle) = build_handle();

    handle
        .send_control(EngineControlMessage::TuneNode {
            node_id: "ghost".to_string(),
            message: NodeControlMessage::Start,
        })
        .await
        .expect("send TuneNode");

    // Actor must remain responsive after a TuneNode to a non-existent node.
    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.is_empty());

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn remove_node_after_ready_cleans_up_state() {
    let (counter, handle) = build_handle();
    add_and_wait(&handle, "one").await;

    assert_eq!(counter.load(Ordering::SeqCst), 1, "factory must have run exactly once");

    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "one".to_string() })
        .await
        .expect("send RemoveNode");

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut removed = false;
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if !states.contains_key("one") {
                removed = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(removed, "RemoveNode on a Ready node should clear it from node_states");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn engine_shutdown_clears_all_node_states() {
    let (_counter, handle) = build_handle();
    add_and_wait(&handle, "a").await;
    add_and_wait(&handle, "b").await;

    handle.shutdown_and_wait().await.expect("shutdown");

    let post = handle.get_node_states().await;
    assert!(
        post.is_err(),
        "after Shutdown completes the actor must be gone, so queries should fail"
    );
}

#[tokio::test]
async fn duplicate_add_node_does_not_overwrite_state() {
    let (counter, handle) = build_handle();
    add_and_wait(&handle, "dup").await;
    let factory_calls_before = counter.load(Ordering::SeqCst);

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "dup".to_string(),
            kind: "test::idle".to_string(),
            params: None,
        })
        .await
        .expect("send duplicate AddNode");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let states = handle.get_node_states().await.expect("get_node_states");
    let state = states.get("dup").cloned().expect("dup must still exist");
    assert!(
        !matches!(state, NodeState::Creating),
        "duplicate AddNode must NOT regress the existing node to Creating; got {state:?}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        factory_calls_before,
        "duplicate AddNode must not invoke the factory again"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}
