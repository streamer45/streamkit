// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Direct-construction tests for update handler edge cases in
//! `dynamic_actor.rs`:
//!
//!   * State / stats / view-data updates for removed nodes (early return)
//!   * Stats delta computation when counters reset
//!   * Stats fallback metric labels when cache is missing
//!   * Closed-subscriber cleanup in state / telemetry / view-data handlers
//!
//! Uses the direct `DynamicEngine` construction pattern established by
//! `pipeline_activation.rs` and `connection_types.rs`, which gives
//! deterministic control over internal state without timing dependencies.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// assertion failures. No production code.
#![allow(clippy::expect_used)]

use super::super::*;
use super::create_test_engine;
use crate::dynamic_actor::{NodeMetricLabels, NodePinMetadata};
use opentelemetry::KeyValue;
use std::time::SystemTime;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::view_data::NodeViewDataUpdate;
use streamkit_core::{OutputPin, PinCardinality};
use tokio::sync::mpsc;

/// Register a live node in the engine with pre-built metric labels and a
/// control channel, returning the control receiver.
fn add_live_node(
    engine: &mut DynamicEngine,
    name: &str,
    state: NodeState,
) -> mpsc::Receiver<streamkit_core::control::NodeControlMessage> {
    let (control_tx, control_rx) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(
        name.to_string(),
        graph_builder::LiveNode { control_tx, task_handle, generation: 0 },
    );
    engine.node_pin_metadata.insert(
        name.to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: streamkit_core::types::PacketType::Binary,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );
    engine.node_kinds.insert(name.to_string(), "test::node".to_string());
    engine.node_metric_labels.insert(
        name.to_string(),
        NodeMetricLabels {
            stats: vec![
                KeyValue::new("node_id", name.to_string()),
                KeyValue::new("node_kind", "test::node".to_string()),
            ],
            node_id_kv: KeyValue::new("node_id", name.to_string()),
            attrs: Vec::new(),
        },
    );
    std::sync::Arc::make_mut(&mut engine.node_states).insert(name.to_string(), state);
    control_rx
}

#[tokio::test]
async fn stale_generation_state_update_is_discarded() {
    let mut engine = create_test_engine();
    let _rx = add_live_node(&mut engine, "recycled", NodeState::Running);
    // Simulate a re-added node whose live incarnation is generation 7.
    engine.live_nodes.get_mut("recycled").expect("live node").generation = 7;

    let (sub_tx, mut sub_rx) = mpsc::channel(16);
    engine.state_subscribers.push(sub_tx);

    // A terminal update enqueued by the PREVIOUS instance (generation 6) must
    // not clobber the live node's state nor reach subscribers (#606).
    let stale =
        NodeStateUpdate::new("recycled".to_string(), NodeState::Failed { reason: "old".into() })
            .with_generation(6);
    engine.handle_state_update(&stale);
    assert!(
        matches!(engine.node_states.get("recycled"), Some(NodeState::Running)),
        "stale-generation update must not change live state"
    );
    assert!(sub_rx.try_recv().is_err(), "stale-generation update must not reach subscribers");

    // The matching generation IS applied.
    let fresh =
        NodeStateUpdate::new("recycled".to_string(), NodeState::Failed { reason: "new".into() })
            .with_generation(7);
    engine.handle_state_update(&fresh);
    assert!(
        matches!(engine.node_states.get("recycled"), Some(NodeState::Failed { .. })),
        "matching-generation update must be applied"
    );
    assert!(sub_rx.try_recv().is_ok(), "matching-generation update must reach subscribers");
}

#[tokio::test]
async fn state_update_for_removed_node_is_ignored() {
    let mut engine = create_test_engine();

    let (sub_tx, mut sub_rx) = mpsc::channel(16);
    engine.state_subscribers.push(sub_tx);

    // No live node named "ghost" — simulate a stale update from a lingering task.
    let update = NodeStateUpdate::new("ghost".to_string(), NodeState::Running);
    engine.handle_state_update(&update);

    // The handler should early-return: no state inserted, no subscriber notification.
    assert!(
        !engine.node_states.contains_key("ghost"),
        "state should NOT be inserted for a removed node"
    );
    assert!(
        sub_rx.try_recv().is_err(),
        "subscriber should NOT receive an update for a removed node"
    );
}

#[tokio::test]
async fn stats_update_for_removed_node_is_ignored() {
    let mut engine = create_test_engine();

    let (sub_tx, mut sub_rx) = mpsc::channel(16);
    engine.stats_subscribers.push(sub_tx);

    let update = NodeStatsUpdate {
        node_id: "ghost".to_string(),
        stats: NodeStats { received: 5, sent: 3, discarded: 0, errored: 0, duration_secs: 1.0 },
        timestamp: SystemTime::now(),
    };
    engine.handle_stats_update(&update);

    assert!(
        !engine.node_stats.contains_key("ghost"),
        "stats should NOT be inserted for a removed node"
    );
    assert!(sub_rx.try_recv().is_err(), "subscriber should NOT receive stats for a removed node");
}

#[tokio::test]
async fn view_data_update_for_removed_node_is_ignored() {
    let mut engine = create_test_engine();

    let (sub_tx, mut sub_rx) = mpsc::channel(16);
    engine.view_data_subscribers.push(sub_tx);

    let update = NodeViewDataUpdate {
        node_id: "ghost".to_string(),
        data: serde_json::json!({"frame": 42}),
        timestamp: SystemTime::now(),
    };
    engine.handle_view_data_update(&update);

    assert!(
        !engine.node_view_data.contains_key("ghost"),
        "view data should NOT be inserted for a removed node"
    );
    assert!(
        sub_rx.try_recv().is_err(),
        "subscriber should NOT receive view data for a removed node"
    );
}

#[tokio::test]
async fn closed_state_subscriber_is_pruned() {
    let mut engine = create_test_engine();
    let _control_rx = add_live_node(&mut engine, "n1", NodeState::Initializing);

    let (sub_tx, sub_rx) = mpsc::channel(16);
    engine.state_subscribers.push(sub_tx);

    // Drop the receiver so the channel is closed.
    drop(sub_rx);

    let update = NodeStateUpdate::new("n1".to_string(), NodeState::Running);
    engine.handle_state_update(&update);

    assert!(
        engine.state_subscribers.is_empty(),
        "closed subscriber should have been removed by retain"
    );
}

#[tokio::test]
async fn closed_telemetry_subscriber_is_pruned() {
    let mut engine = create_test_engine();

    let (sub_tx, sub_rx) = mpsc::channel(16);
    engine.telemetry_subscribers.push(sub_tx);

    drop(sub_rx);

    let event = streamkit_core::telemetry::TelemetryEvent::new(
        None,
        "n1".to_string(),
        serde_json::json!({"type": "test"}),
        0,
    );
    engine.handle_telemetry_event(&event);

    assert!(
        engine.telemetry_subscribers.is_empty(),
        "closed telemetry subscriber should have been removed"
    );
}

#[tokio::test]
async fn closed_view_data_subscriber_is_pruned() {
    let mut engine = create_test_engine();
    let _control_rx = add_live_node(&mut engine, "n1", NodeState::Running);

    let (sub_tx, sub_rx) = mpsc::channel(16);
    engine.view_data_subscribers.push(sub_tx);

    drop(sub_rx);

    let update = NodeViewDataUpdate {
        node_id: "n1".to_string(),
        data: serde_json::json!({"frame": 1}),
        timestamp: SystemTime::now(),
    };
    engine.handle_view_data_update(&update);

    assert!(
        engine.view_data_subscribers.is_empty(),
        "closed view-data subscriber should have been removed"
    );
}

#[tokio::test]
async fn closed_stats_subscriber_is_pruned() {
    let mut engine = create_test_engine();
    let _control_rx = add_live_node(&mut engine, "n1", NodeState::Running);

    let (sub_tx, sub_rx) = mpsc::channel(16);
    engine.stats_subscribers.push(sub_tx);

    drop(sub_rx);

    let update = NodeStatsUpdate {
        node_id: "n1".to_string(),
        stats: NodeStats { received: 1, sent: 0, discarded: 0, errored: 0, duration_secs: 0.1 },
        timestamp: SystemTime::now(),
    };
    engine.handle_stats_update(&update);

    assert!(
        engine.stats_subscribers.is_empty(),
        "closed stats subscriber should have been removed"
    );
}

#[tokio::test]
async fn stats_delta_handles_counter_reset() {
    let mut engine = create_test_engine();
    let _control_rx = add_live_node(&mut engine, "n1", NodeState::Running);

    // Seed initial stats.
    let update1 = NodeStatsUpdate {
        node_id: "n1".to_string(),
        stats: NodeStats { received: 10, sent: 5, discarded: 2, errored: 1, duration_secs: 1.0 },
        timestamp: SystemTime::now(),
    };
    engine.handle_stats_update(&update1);

    let stored = engine.node_stats.get("n1").expect("stats should exist");
    assert_eq!(stored.received, 10);
    assert_eq!(stored.sent, 5);

    // Simulate a counter reset: new values are lower than previous.
    let update2 = NodeStatsUpdate {
        node_id: "n1".to_string(),
        stats: NodeStats { received: 3, sent: 2, discarded: 0, errored: 0, duration_secs: 2.0 },
        timestamp: SystemTime::now(),
    };
    engine.handle_stats_update(&update2);

    let stored = engine.node_stats.get("n1").expect("stats should exist");
    assert_eq!(stored.received, 3, "latest stats should be stored after reset");
    assert_eq!(stored.sent, 2, "latest stats should be stored after reset");

    // Normal increment after the reset.
    let update3 = NodeStatsUpdate {
        node_id: "n1".to_string(),
        stats: NodeStats { received: 8, sent: 7, discarded: 1, errored: 0, duration_secs: 3.0 },
        timestamp: SystemTime::now(),
    };
    engine.handle_stats_update(&update3);

    let stored = engine.node_stats.get("n1").expect("stats should exist");
    assert_eq!(stored.received, 8);
    assert_eq!(stored.sent, 7);
}

#[tokio::test]
async fn stats_fallback_labels_when_cache_missing() {
    let mut engine = create_test_engine();

    // Add a live node but intentionally skip populating node_metric_labels
    // so the fallback path synthesises labels from node_kinds.
    let (control_tx, _control_rx) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(
        "uncached".to_string(),
        graph_builder::LiveNode { control_tx, task_handle, generation: 0 },
    );
    engine.node_kinds.insert("uncached".to_string(), "test::special".to_string());
    std::sync::Arc::make_mut(&mut engine.node_states)
        .insert("uncached".to_string(), NodeState::Running);

    let (sub_tx, mut sub_rx) = mpsc::channel(16);
    engine.stats_subscribers.push(sub_tx);

    let update = NodeStatsUpdate {
        node_id: "uncached".to_string(),
        stats: NodeStats { received: 1, sent: 2, discarded: 0, errored: 0, duration_secs: 0.5 },
        timestamp: SystemTime::now(),
    };

    engine.handle_stats_update(&update);

    let stored = engine.node_stats.get("uncached").expect("stats should be stored");
    assert_eq!(stored.received, 1);
    assert_eq!(stored.sent, 2);

    // The fallback path synthesises labels from node_kinds. Verify the
    // node_kind used by the fallback matches what we inserted, and that
    // no cached labels were used (i.e. the fallback was actually taken).
    assert!(
        !engine.node_metric_labels.contains_key("uncached"),
        "metric labels cache must remain empty to confirm fallback was used"
    );
    assert_eq!(
        engine.node_kinds.get("uncached").map(String::as_str),
        Some("test::special"),
        "fallback should use node_kind from the node_kinds map"
    );

    // Stats broadcast still works through the fallback path.
    let received = sub_rx.try_recv().expect("subscriber should receive the stats update");
    assert_eq!(received.node_id, "uncached");
    assert_eq!(received.stats.received, 1);
    assert_eq!(received.stats.sent, 2);
}
