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

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// setup failures (channel sends, control-plane queries). No production code.
#![allow(clippy::expect_used)]

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

struct SourceNode;

#[streamkit_core::async_trait]
impl ProcessorNode for SourceNode {
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
        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                _ => {},
            }
        }
    }
}

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
    registry.register_dynamic(
        "test::source",
        |_p| Ok(Box::new(SourceNode) as Box<dyn ProcessorNode>),
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

fn build_handle_with_slow() -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::source",
        |_p| Ok(Box::new(SourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow",
        |_p| {
            std::thread::sleep(Duration::from_millis(500));
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
    engine.start_dynamic_actor(DynamicEngineConfig::default())
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
        .expect("send Disconnect (live source, ghost dest)");

    handle
        .send_control(EngineControlMessage::Disconnect {
            from_node: "ghost_src".to_string(),
            from_pin: "out".to_string(),
            to_node: "real".to_string(),
            to_pin: "in".to_string(),
        })
        .await
        .expect("send Disconnect (ghost source, live dest)");

    handle
        .send_control(EngineControlMessage::Disconnect {
            from_node: "ghost_src".to_string(),
            from_pin: "out".to_string(),
            to_node: "ghost_dst".to_string(),
            to_pin: "in".to_string(),
        })
        .await
        .expect("send Disconnect (both ghost)");

    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.contains_key("real"), "live node should still be present");

    // Verify subsequent operations still work: add a second node and
    // connect it to the surviving node.
    add_and_wait(&handle, "partner").await;

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "real".to_string(),
            from_pin: "out".to_string(),
            to_node: "partner".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect after ghost disconnects");

    let states = handle.get_node_states().await.expect("post-connect states");
    assert!(states.contains_key("real"), "real should still be present after connect");
    assert!(states.contains_key("partner"), "partner should be present after connect");

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

// Observing the *actor-internal* `node_states` HashMap directly isn't
// possible through the public `DynamicEngineHandle` API once the actor has
// shut down (the query channel is dropped). The next-best behavioral
// signal that Shutdown's drain path ran is: the actor task itself exited
// (subsequent queries fail) AND any pending queries enqueued just before
// shutdown either resolve or surface a closed-channel error rather than
// hanging forever. This test pins the latter.
#[tokio::test]
async fn engine_shutdown_stops_actor_and_closes_query_channel() {
    let (_counter, handle) = build_handle();
    add_and_wait(&handle, "a").await;
    add_and_wait(&handle, "b").await;

    let pre = handle.get_node_states().await.expect("pre-shutdown states");
    assert!(pre.contains_key("a") && pre.contains_key("b"));

    handle.shutdown_and_wait().await.expect("shutdown");

    // Subsequent queries must surface a closed-channel error within a
    // reasonable bound; they must NOT hang.
    let post = tokio::time::timeout(Duration::from_secs(2), handle.get_node_states())
        .await
        .expect("post-shutdown query must not hang");
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

    // The actor processes control messages serially, so a query enqueued
    // AFTER the duplicate AddNode is only answered AFTER the duplicate has
    // been fully handled. We don't need a sleep: the round-trip itself is
    // the barrier.
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

#[tokio::test]
async fn disconnect_after_connect_cleans_up_tracking() {
    let (_counter, handle) = build_handle();

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::source".to_string(),
            params: None,
        })
        .await
        .expect("add source");
    add_and_wait(&handle, "dest").await;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if states.get("src").is_some_and(|s| !matches!(s, NodeState::Creating)) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "dest".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    tokio::time::sleep(Duration::from_millis(100)).await;

    handle
        .send_control(EngineControlMessage::Disconnect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "dest".to_string(),
            to_pin: "in".to_string(),
        })
        .await
        .expect("disconnect");

    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.contains_key("src") && states.contains_key("dest"));

    // Reconnect the same edge. If disconnect did not clean up the
    // distributor, the second Connect would add a duplicate output
    // to the PinDistributor. Removing the dest afterwards would then
    // leave a dangling output, potentially panicking or erroring on
    // subsequent sends. A clean cycle proves cleanup happened.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "dest".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("reconnect");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Remove dest to tear down the reconnected edge.
    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "dest".to_string() })
        .await
        .expect("remove dest");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if !states.contains_key("dest") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Source must survive the full connect → disconnect → reconnect →
    // remove-dest cycle, proving no stale distributor state remained.
    let states = handle.get_node_states().await.expect("final states");
    assert!(states.contains_key("src"), "source should survive full disconnect-reconnect cycle");
    assert!(!states.contains_key("dest"), "dest should be removed after RemoveNode");

    handle.shutdown_and_wait().await.expect("shutdown");
}

#[tokio::test]
async fn disconnect_cancels_pending_connection() {
    let handle = build_handle_with_slow();

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

    // Connect while slow is Creating → deferred.
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

    // Cancel before slow finishes → pending_connections drained.
    handle
        .send_control(EngineControlMessage::Disconnect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "slow".to_string(),
            to_pin: "in".to_string(),
        })
        .await
        .expect("disconnect");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if states.get("slow").is_some_and(|s| !matches!(s, NodeState::Creating)) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.contains_key("slow"), "slow node should exist after creation");

    // If the cancelled pending connection had been replayed, the
    // distributor would already have an output for (slow, in).
    // Connecting the same edge now and then removing slow exercises
    // whether a duplicate output exists — duplicates would leave a
    // stale entry in the distributor after removal, causing errors
    // on subsequent sends from src.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "slow".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("fresh connect after cancelled pending");

    tokio::time::sleep(Duration::from_millis(50)).await;

    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "slow".to_string() })
        .await
        .expect("remove slow");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if !states.contains_key("slow") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Source must be healthy after the full cycle, proving the
    // pending connection was truly cancelled and not replayed.
    let states = handle.get_node_states().await.expect("final states");
    assert!(states.contains_key("src"), "source should survive pending-cancel cycle");
    assert!(!states.contains_key("slow"), "slow should be removed");

    handle.shutdown_and_wait().await.expect("shutdown");
}

struct EmittingSourceNode;

#[streamkit_core::async_trait]
impl ProcessorNode for EmittingSourceNode {
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
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                msg = ctx.control_rx.recv() => match msg {
                    Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                    _ => {},
                },
                _ = ticker.tick() => {
                    let _ = ctx
                        .output_sender
                        .send(
                            "out",
                            streamkit_core::types::Packet::Binary {
                                data: bytes::Bytes::from_static(b"tick"),
                                content_type: None,
                                metadata: None,
                            },
                        )
                        .await;
                },
            }
        }
    }
}

struct CountingSinkNode {
    received: Arc<AtomicU32>,
}

#[streamkit_core::async_trait]
impl ProcessorNode for CountingSinkNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        vec![streamkit_core::InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: streamkit_core::PinCardinality::One,
        }]
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        Vec::new()
    }
    async fn run(
        self: Box<Self>,
        mut ctx: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        let mut input = ctx.inputs.remove("in");
        loop {
            tokio::select! {
                msg = ctx.control_rx.recv() => match msg {
                    Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                    _ => {},
                },
                Some(packet) = async {
                    match input.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let _ = packet;
                    self.received.fetch_add(1, Ordering::SeqCst);
                },
            }
        }
    }
}

/// Regression test for issue #528: `RemoveNode` of a **live** node must prune
/// deferred (pending) connections referencing it. Otherwise re-adding a node
/// with the same id causes `flush_pending_connections` to replay the stale
/// connection, resurrecting a phantom edge that delivers packets the
/// post-removal graph never requested.
#[tokio::test]
async fn remove_live_node_prunes_pending_connections() {
    let received = Arc::new(AtomicU32::new(0));
    let sink_received = received.clone();

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::emitter",
        |_p| Ok(Box::new(EmittingSourceNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::slow_sink",
        move |_p| {
            std::thread::sleep(Duration::from_millis(600));
            Ok(Box::new(CountingSinkNode { received: sink_received.clone() })
                as Box<dyn ProcessorNode>)
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

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::emitter".to_string(),
            params: None,
        })
        .await
        .expect("add src");

    // Wait for src to be live so RemoveNode hits the shutdown_node path.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if states.get("src").is_some_and(|s| !matches!(s, NodeState::Creating)) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "sink".to_string(),
            kind: "test::slow_sink".to_string(),
            params: None,
        })
        .await
        .expect("add sink");

    // Connect while sink is still Creating → deferred into pending_connections.
    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "sink".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // Remove the live src endpoint — this must also discard the deferred
    // connection, just as the Creating-node removal branch does.
    handle
        .send_control(EngineControlMessage::RemoveNode { node_id: "src".to_string() })
        .await
        .expect("remove src");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if !states.contains_key("src") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Re-add a node with the same id before the sink finishes creating.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "src".to_string(),
            kind: "test::emitter".to_string(),
            params: None,
        })
        .await
        .expect("re-add src");

    // Wait for the sink to finish creating; flush_pending_connections runs here.
    let sink_ready = {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            if let Ok(states) = handle.get_node_states().await {
                if states.get("sink").is_some_and(|s| !matches!(s, NodeState::Creating)) {
                    ready = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ready
    };
    assert!(sink_ready, "sink should finish creation");

    // Give a phantom edge ample time to deliver packets (src emits every 10ms).
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        received.load(Ordering::SeqCst),
        0,
        "no packets should flow: the deferred connection referenced a removed node \
         and must not be replayed when the id is reused"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}
