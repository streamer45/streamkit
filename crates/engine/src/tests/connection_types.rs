// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for connection type validation in the dynamic engine.

use super::super::*;
use crate::dynamic_actor::{DynamicEngine, NodePinMetadata};
use streamkit_core::types::{AudioFormat, PacketType, SampleFormat};
use streamkit_core::{InputPin, OutputPin, PinCardinality};
use tokio::sync::mpsc;

/// Helper to create a minimal DynamicEngine for testing
#[allow(clippy::unwrap_used)] // Tests use unwrap for assertions
fn create_test_engine() -> DynamicEngine {
    let (control_tx, control_rx) = mpsc::channel(32);
    let (query_tx, query_rx) = mpsc::channel(32);
    drop(control_tx);
    drop(query_tx);

    let meter = opentelemetry::global::meter("test");
    DynamicEngine {
        registry: NodeRegistry::new(),
        control_rx,
        query_rx,
        live_nodes: HashMap::new(),
        node_inputs: HashMap::new(),
        pin_distributors: HashMap::new(),
        pin_management_txs: HashMap::new(),
        node_pin_metadata: HashMap::new(),
        batch_size: 32,
        session_id: None,
        audio_pool: std::sync::Arc::new(streamkit_core::FramePool::<f32>::audio_default()),
        node_input_capacity: 128,
        pin_distributor_capacity: 64,
        node_states: HashMap::new(),
        state_subscribers: Vec::new(),
        node_stats: HashMap::new(),
        stats_subscribers: Vec::new(),
        telemetry_subscribers: Vec::new(),
        nodes_active_gauge: meter.u64_gauge("test.nodes").build(),
        node_state_transitions_counter: meter.u64_counter("test.transitions").build(),
        engine_operations_counter: meter.u64_counter("test.operations").build(),
        node_packets_received_gauge: meter.u64_gauge("test.received").build(),
        node_packets_sent_gauge: meter.u64_gauge("test.sent").build(),
        node_packets_discarded_gauge: meter.u64_gauge("test.discarded").build(),
        node_packets_errored_gauge: meter.u64_gauge("test.errored").build(),
        node_state_gauge: meter.u64_gauge("test.state").build(),
    }
}

#[test]
fn test_validate_connection_types_compatible() {
    let mut engine = create_test_engine();

    let audio_format =
        AudioFormat { sample_rate: 48000, channels: 2, sample_format: SampleFormat::F32 };

    // Create source node with RawAudio output
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::RawAudio(audio_format.clone()),
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Create destination node that accepts RawAudio
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::RawAudio(audio_format)],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );

    // Should succeed
    let result = engine.validate_connection_types("source", "out", "dest", "in");
    assert!(result.is_ok());
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_validate_connection_types_incompatible() {
    let mut engine = create_test_engine();

    let audio_format =
        AudioFormat { sample_rate: 48000, channels: 2, sample_format: SampleFormat::F32 };

    // Create source node with OpusAudio output
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::OpusAudio,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Create destination node that only accepts RawAudio
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::RawAudio(audio_format)],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );

    // Should fail
    let result = engine.validate_connection_types("source", "out", "dest", "in");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Type mismatch"));
}

#[test]
fn test_validate_connection_types_passthrough_source() {
    let mut engine = create_test_engine();

    let audio_format =
        AudioFormat { sample_rate: 48000, channels: 2, sample_format: SampleFormat::F32 };

    // Create source node with Passthrough output (like pacer)
    engine.node_pin_metadata.insert(
        "pacer".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Any],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Passthrough,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Create destination node that accepts RawAudio
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::RawAudio(audio_format)],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );

    // Should succeed - Passthrough is allowed in dynamic pipelines
    let result = engine.validate_connection_types("pacer", "out", "dest", "in");
    assert!(result.is_ok());
}

#[test]
fn test_validate_connection_types_any_destination() {
    let mut engine = create_test_engine();

    // Create source node with OpusAudio output
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::OpusAudio,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Create destination node that accepts Any
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Any],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );

    // Should succeed - Any accepts everything
    let result = engine.validate_connection_types("source", "out", "dest", "in");
    assert!(result.is_ok());
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_validate_connection_types_node_not_found() {
    let engine = create_test_engine();

    // Try to validate connection for non-existent nodes
    let result = engine.validate_connection_types("nonexistent", "out", "dest", "in");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Source node 'nonexistent' not found"));
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_validate_connection_types_pin_not_found() {
    let mut engine = create_test_engine();

    // Create source node
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::OpusAudio,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Create destination node
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::OpusAudio],
                cardinality: PinCardinality::One,
            }],
            output_pins: vec![],
        },
    );

    // Try to validate connection with non-existent source pin
    let result = engine.validate_connection_types("source", "nonexistent", "dest", "in");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Source pin 'nonexistent' not found"));
}

#[test]
fn test_validate_connection_types_dynamic_pin_prefix_match() {
    let mut engine = create_test_engine();

    // Create source node with Binary output (simple unit variant)
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Binary,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Destination declares a dynamic pin family template with prefix "in"
    engine.node_pin_metadata.insert(
        "dest".to_string(),
        NodePinMetadata {
            input_pins: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Binary],
                cardinality: PinCardinality::Dynamic { prefix: "in".to_string() },
            }],
            output_pins: vec![],
        },
    );

    // Should succeed for any concrete pin name in that family.
    let result = engine.validate_connection_types("source", "out", "dest", "in_0");
    assert!(result.is_ok());
}

#[test]
fn test_validate_connection_types_missing_pin_allowed_for_dynamic_pin_nodes() {
    let mut engine = create_test_engine();

    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Binary,
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );

    // Destination metadata does not list the pin, but the node supports dynamic pins.
    engine
        .node_pin_metadata
        .insert("dest".to_string(), NodePinMetadata { input_pins: vec![], output_pins: vec![] });
    let (tx, _rx) = mpsc::channel(1);
    engine.pin_management_txs.insert("dest".to_string(), tx);

    // Should succeed (pin will be created on-demand during connect).
    let result = engine.validate_connection_types("source", "out", "dest", "in_0");
    assert!(result.is_ok());
}
