// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Tests for pipeline activation logic in the dynamic engine.
//!
//! Verifies that `check_and_activate_pipeline()` sends Start signals to source
//! nodes even when other nodes are in non-Running states (Degraded, Recovering).

use super::super::*;
use super::create_test_engine;
use crate::dynamic_actor::NodePinMetadata;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::state::NodeState;
use streamkit_core::{InputPin, OutputPin, PinCardinality};
use tokio::sync::mpsc;

/// Register a source node (no input pins) in the engine with a control channel,
/// returning the control receiver so the test can check for Start messages.
fn add_source_node(
    engine: &mut DynamicEngine,
    name: &str,
    state: NodeState,
) -> mpsc::Receiver<NodeControlMessage> {
    let (control_tx, control_rx) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(name.to_string(), graph_builder::LiveNode { control_tx, task_handle });
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
    std::sync::Arc::make_mut(&mut engine.node_states).insert(name.to_string(), state);
    control_rx
}

/// Register a non-source node (has input pins) in the engine.
fn add_processor_node(engine: &mut DynamicEngine, name: &str, state: NodeState) {
    let (control_tx, _) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine.live_nodes.insert(name.to_string(), graph_builder::LiveNode { control_tx, task_handle });
    engine.node_pin_metadata.insert(
        name.to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![streamkit_core::types::PacketType::Binary],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: streamkit_core::types::PacketType::Binary,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );
    std::sync::Arc::make_mut(&mut engine.node_states).insert(name.to_string(), state);
}

/// Source node in Ready should receive Start when all other nodes are Running.
#[tokio::test]
async fn test_activation_with_all_running() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(&mut engine, "processor", NodeState::Running);

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(
        matches!(msg, Ok(NodeControlMessage::Start)),
        "source node should receive Start when all nodes are Ready|Running"
    );
}

/// Source node in Ready should receive Start even when another node is Degraded.
/// This is the exact scenario from the production bug: the mixer enters Degraded
/// (slow_input_timeout) before the file_reader source gets its Start signal.
#[tokio::test]
async fn test_activation_with_degraded_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "file_reader", NodeState::Ready);
    add_processor_node(
        &mut engine,
        "mixer",
        NodeState::Degraded { reason: "slow_input_timeout".to_string(), details: None },
    );
    add_processor_node(&mut engine, "encoder", NodeState::Running);

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(
        matches!(msg, Ok(NodeControlMessage::Start)),
        "source node should receive Start even when mixer is Degraded"
    );
}

/// Source node in Ready should receive Start even when another node is Recovering.
/// Transport nodes can enter Recovering on transient connection failures.
#[tokio::test]
async fn test_activation_with_recovering_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "file_reader", NodeState::Ready);
    add_processor_node(
        &mut engine,
        "transport",
        NodeState::Recovering { reason: "connection lost".to_string(), details: None },
    );

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(
        matches!(msg, Ok(NodeControlMessage::Start)),
        "source node should receive Start even when transport is Recovering"
    );
}

/// Source node should NOT receive Start while any node is still Initializing.
#[tokio::test]
async fn test_activation_blocked_by_initializing_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(&mut engine, "slow_node", NodeState::Initializing);

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(
        msg.is_err(),
        "source node should NOT receive Start while a node is still Initializing"
    );
}

/// Non-source nodes (nodes with input pins) should not receive Start signals.
#[tokio::test]
async fn test_activation_only_starts_source_nodes() {
    let mut engine = create_test_engine();

    // Source node in Ready — should get Start
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);

    // Processor node also in Ready — should NOT get Start (it has input pins)
    let (control_tx, mut processor_rx) = mpsc::channel(8);
    let task_handle = tokio::spawn(async { Ok(()) });
    engine
        .live_nodes
        .insert("processor".to_string(), graph_builder::LiveNode { control_tx, task_handle });
    engine.node_pin_metadata.insert(
        "processor".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![streamkit_core::types::PacketType::Binary],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );
    std::sync::Arc::make_mut(&mut engine.node_states)
        .insert("processor".to_string(), NodeState::Ready);

    engine.check_and_activate_pipeline();

    assert!(
        matches!(source_rx.try_recv(), Ok(NodeControlMessage::Start)),
        "source node should receive Start"
    );
    assert!(
        processor_rx.try_recv().is_err(),
        "processor node should NOT receive Start (it has input pins)"
    );
}

/// Pipeline with multiple Degraded and Recovering nodes should still activate.
#[tokio::test]
async fn test_activation_with_mixed_degraded_and_recovering() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(
        &mut engine,
        "mixer",
        NodeState::Degraded { reason: "slow_input_timeout".to_string(), details: None },
    );
    add_processor_node(
        &mut engine,
        "transport_in",
        NodeState::Recovering { reason: "reconnecting".to_string(), details: None },
    );
    add_processor_node(&mut engine, "encoder", NodeState::Running);

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(
        matches!(msg, Ok(NodeControlMessage::Start)),
        "source should receive Start with mixed Degraded/Recovering/Running nodes"
    );
}

/// Source node should NOT receive Start when a downstream node has Failed.
/// Starting sources into a broken pipeline would produce packets that go nowhere.
#[tokio::test]
async fn test_activation_blocked_by_failed_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(
        &mut engine,
        "broken",
        NodeState::Failed { reason: "configuration error".to_string() },
    );

    engine.check_and_activate_pipeline();

    assert!(
        source_rx.try_recv().is_err(),
        "source node should NOT receive Start when a node has Failed"
    );
}

/// Source node should NOT receive Start while any node is still Creating.
#[tokio::test]
async fn test_activation_blocked_by_creating_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(&mut engine, "slow_node", NodeState::Creating);

    engine.check_and_activate_pipeline();

    let msg = source_rx.try_recv();
    assert!(msg.is_err(), "source node should NOT receive Start while a node is still Creating");
}

/// Source node should NOT receive Start when a downstream node has Stopped.
#[tokio::test]
async fn test_activation_blocked_by_stopped_node() {
    let mut engine = create_test_engine();
    let mut source_rx = add_source_node(&mut engine, "source", NodeState::Ready);
    add_processor_node(
        &mut engine,
        "done",
        NodeState::Stopped { reason: streamkit_core::state::StopReason::Completed },
    );

    engine.check_and_activate_pipeline();

    assert!(
        source_rx.try_recv().is_err(),
        "source node should NOT receive Start when a node has Stopped"
    );
}
