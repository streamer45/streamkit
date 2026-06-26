// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Defensive-hardening coverage for `dynamic_actor.rs`:
//!
//! - #289: a node reaching a terminal state at runtime drops the deferred
//!   connections/tunes that referenced it, without touching unrelated
//!   pending state.
//! - #290: `TuneNode` for a node that has already gone terminal is dropped
//!   instead of being pushed into its (likely dead) control channel.
//!
//! Uses the direct `DynamicEngine` construction pattern (see
//! `dynamic_actor_update_filters.rs`) for deterministic control over
//! internal state without timing dependencies.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// assertion failures. No production code.
#![allow(clippy::expect_used)]

use super::super::*;
use super::create_test_engine;
use std::sync::Arc;
use streamkit_core::control::{ConnectionMode, EngineControlMessage, NodeControlMessage};
use streamkit_core::state::{NodeState, NodeStateUpdate};
use tokio::sync::mpsc;

/// Register a minimal live node (control channel + state) and return its
/// control receiver so tests can observe what the actor forwarded.
fn add_live_node(
    engine: &mut DynamicEngine,
    name: &str,
    state: NodeState,
) -> mpsc::Receiver<NodeControlMessage> {
    let (control_tx, control_rx) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(
        name.to_string(),
        graph_builder::LiveNode { control_tx, task_handle, generation: 0 },
    );
    Arc::make_mut(&mut engine.node_states).insert(name.to_string(), state);
    control_rx
}

fn set_creating(engine: &mut DynamicEngine, name: &str) {
    Arc::make_mut(&mut engine.node_states).insert(name.to_string(), NodeState::Creating);
}

async fn defer_connection(engine: &mut DynamicEngine, from: &str, to: &str) {
    engine
        .handle_engine_control(EngineControlMessage::Connect {
            from_node: from.to_string(),
            from_pin: "out".to_string(),
            to_node: to.to_string(),
            to_pin: "in".to_string(),
            mode: ConnectionMode::Reliable,
        })
        .await;
}

#[tokio::test]
async fn terminal_state_prunes_pending_for_that_node_only() {
    let mut engine = create_test_engine();

    // "a" is live; "b"/"x"/"y" are still Creating so connections defer.
    let _rx = add_live_node(&mut engine, "a", NodeState::Running);
    set_creating(&mut engine, "b");
    set_creating(&mut engine, "x");
    set_creating(&mut engine, "y");

    // a -> b references the soon-to-fail node; x -> y is unrelated.
    defer_connection(&mut engine, "a", "b").await;
    defer_connection(&mut engine, "x", "y").await;

    assert_eq!(engine.pending_connections.len(), 2, "both connections should defer");

    let failed = NodeStateUpdate::new("a".to_string(), NodeState::Failed { reason: "boom".into() });
    engine.handle_state_update(&failed);

    assert_eq!(
        engine.pending_connections.len(),
        1,
        "the deferred edge referencing the failed node must be dropped; x->y must survive"
    );
}

#[tokio::test]
async fn tune_for_terminal_node_is_dropped() {
    let mut engine = create_test_engine();
    let mut control_rx =
        add_live_node(&mut engine, "a", NodeState::Failed { reason: "dead".into() });

    engine
        .handle_engine_control(EngineControlMessage::TuneNode {
            node_id: "a".to_string(),
            message: NodeControlMessage::UpdateParams(serde_json::json!({"gain": 2})),
        })
        .await;

    assert!(
        control_rx.try_recv().is_err(),
        "TuneNode for a terminal node must not be forwarded to its control channel"
    );
}

#[tokio::test]
async fn tune_for_running_node_is_forwarded() {
    let mut engine = create_test_engine();
    let mut control_rx = add_live_node(&mut engine, "a", NodeState::Running);

    engine
        .handle_engine_control(EngineControlMessage::TuneNode {
            node_id: "a".to_string(),
            message: NodeControlMessage::UpdateParams(serde_json::json!({"gain": 2})),
        })
        .await;

    assert!(
        matches!(control_rx.try_recv(), Ok(NodeControlMessage::UpdateParams(_))),
        "TuneNode for a running node must reach its control channel"
    );
}
