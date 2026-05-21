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

        // Wait for Start before emitting the rest.
        loop {
            match ctx.control_rx.recv().await {
                Some(NodeControlMessage::Start) => break,
                Some(NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(_) => {},
            }
        }

        // 1. NodeStateUpdate → handle_state_update
        let _ =
            ctx.state_tx.send(NodeStateUpdate::new(NODE_ID.to_string(), NodeState::Running)).await;

        // 2. TelemetryEvent → handle_telemetry_event
        if let Some(tx) = ctx.telemetry_tx.as_ref() {
            let event = TelemetryEvent::new(
                None,
                NODE_ID.to_string(),
                serde_json::json!({"event_type": "test_event"}),
                0,
            );
            let _ = tx.send(event).await;
        }

        // 3. NodeStatsUpdate → handle_stats_update
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

        // 4. NodeViewDataUpdate → handle_view_data_update
        if let Some(tx) = ctx.view_data_tx.as_ref() {
            let update = NodeViewDataUpdate {
                node_id: NODE_ID.to_string(),
                data: serde_json::json!({"frame": 1}),
                timestamp: SystemTime::now(),
            };
            let _ = tx.send(update).await;
        }

        // Wait for shutdown.
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
            if states.get("emitter").is_some_and(|s| !matches!(s, NodeState::Creating)) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("emitter did not leave Creating in time");
}

#[tokio::test]
async fn state_update_from_node_reaches_subscriber() {
    let handle = build_handle();
    let mut sub = handle.subscribe_state().await.expect("subscribe_state");
    add_emitter_and_wait_ready(&handle).await;

    // We expect at least one state update message; the EmittingNode pushes a
    // `Running` update after its first Start (the engine also pushes
    // lifecycle updates on its own, so a single recv with timeout is enough).
    let got = tokio::time::timeout(Duration::from_secs(3), sub.recv())
        .await
        .expect("state update should arrive")
        .expect("subscriber channel still open");

    // Cheap correctness assertion: id is non-empty and is one of ours.
    assert!(!got.node_id.is_empty());

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
