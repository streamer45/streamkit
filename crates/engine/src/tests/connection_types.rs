// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for connection type validation in the dynamic engine.

use super::super::*;
use crate::dynamic_actor::{DynamicEngine, NodePinMetadata};
use streamkit_core::registry::NodeRegistry;
use streamkit_core::types::{
    AudioCodec, AudioFormat, EncodedAudioFormat, PacketType, SampleFormat,
};
use streamkit_core::{InputPin, OutputPin, PinCardinality};
use tokio::sync::mpsc;

/// Helper to create a minimal DynamicEngine for testing
#[allow(clippy::unwrap_used)] // Tests use unwrap for assertions
fn create_test_engine() -> DynamicEngine {
    let (control_tx, control_rx) = mpsc::channel(32);
    let (query_tx, query_rx) = mpsc::channel(32);
    drop(control_tx);
    drop(query_tx);

    let (node_created_tx, node_created_rx) = mpsc::channel(32);

    let meter = opentelemetry::global::meter("test");
    DynamicEngine {
        registry: std::sync::Arc::new(std::sync::RwLock::new(NodeRegistry::new())),
        control_rx,
        query_rx,
        live_nodes: HashMap::new(),
        node_inputs: HashMap::new(),
        pin_distributors: HashMap::new(),
        pin_management_txs: HashMap::new(),
        dynamic_pin_nodes: std::collections::HashSet::new(),
        node_pin_metadata: HashMap::new(),
        connections: HashMap::new(),
        node_kinds: HashMap::new(),
        batch_size: 32,
        session_id: None,
        audio_pool: std::sync::Arc::new(streamkit_core::FramePool::<f32>::audio_default()),
        video_pool: std::sync::Arc::new(streamkit_core::FramePool::<u8>::video_default()),
        node_input_capacity: 128,
        pin_distributor_capacity: 64,
        node_states: HashMap::new(),
        state_subscribers: Vec::new(),
        node_stats: HashMap::new(),
        stats_subscribers: Vec::new(),
        telemetry_subscribers: Vec::new(),
        node_view_data: HashMap::new(),
        view_data_subscribers: Vec::new(),
        nodes_active_gauge: meter.u64_gauge("test.nodes").build(),
        node_state_transitions_counter: meter.u64_counter("test.transitions").build(),
        engine_operations_counter: meter.u64_counter("test.operations").build(),
        node_packets_received_counter: meter.u64_counter("test.received").build(),
        node_packets_sent_counter: meter.u64_counter("test.sent").build(),
        node_packets_discarded_counter: meter.u64_counter("test.discarded").build(),
        node_packets_errored_counter: meter.u64_counter("test.errored").build(),
        node_state_gauge: meter.u64_gauge("test.state").build(),
        runtime_schemas: HashMap::new(),
        runtime_schema_subscribers: Vec::new(),
        node_created_tx,
        node_created_rx,
        pending_connections: Vec::new(),
        next_creation_id: 0,
        active_creations: std::collections::HashMap::new(),
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

    // Create source node with encoded Opus output
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                }),
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

    // Create source node with encoded Opus output
    engine.node_pin_metadata.insert(
        "source".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                }),
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
                produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                }),
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
                accepts_types: vec![PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                })],
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
    engine.dynamic_pin_nodes.insert("dest".to_string());

    // Should succeed (pin will be created on-demand during connect).
    let result = engine.validate_connection_types("source", "out", "dest", "in_0");
    assert!(result.is_ok());
}

#[test]
fn test_resolve_passthrough_type_direct() {
    let mut engine = create_test_engine();

    let video_type = PacketType::EncodedVideo(streamkit_core::types::EncodedVideoFormat {
        codec: streamkit_core::types::VideoCodec::Vp9,
        bitstream_format: None,
        codec_private: None,
        profile: None,
        level: None,
    });

    // encoder.out -> pacer.in -> pacer.out
    engine.node_pin_metadata.insert(
        "encoder".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: video_type.clone(),
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );
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

    // Record the connection: encoder.out -> pacer.in
    engine.connections.insert(
        ("pacer".to_string(), "in".to_string()),
        ("encoder".to_string(), "out".to_string()),
    );

    // Resolving pacer.out should trace back through pacer.in to encoder.out
    let resolved = engine.resolve_passthrough_type("pacer", "out");
    assert_eq!(resolved, video_type, "Passthrough should resolve to upstream video type");
}

#[test]
fn test_resolve_passthrough_type_chained() {
    let mut engine = create_test_engine();

    let audio_type = PacketType::EncodedAudio(EncodedAudioFormat {
        codec: AudioCodec::Opus,
        codec_private: None,
    });

    // encoder.out -> pacer1.in -> pacer1.out -> pacer2.in -> pacer2.out
    engine.node_pin_metadata.insert(
        "encoder".to_string(),
        NodePinMetadata {
            input_pins: vec![],
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: audio_type.clone(),
                cardinality: PinCardinality::Broadcast,
            }],
        },
    );
    for name in ["pacer1", "pacer2"] {
        engine.node_pin_metadata.insert(
            name.to_string(),
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
    }

    engine.connections.insert(
        ("pacer1".to_string(), "in".to_string()),
        ("encoder".to_string(), "out".to_string()),
    );
    engine.connections.insert(
        ("pacer2".to_string(), "in".to_string()),
        ("pacer1".to_string(), "out".to_string()),
    );

    // Resolving pacer2.out should trace through pacer2 -> pacer1 -> encoder
    let resolved = engine.resolve_passthrough_type("pacer2", "out");
    assert_eq!(resolved, audio_type, "Chained Passthrough should resolve to upstream audio type");
}

#[test]
fn test_resolve_passthrough_type_no_upstream() {
    let mut engine = create_test_engine();

    // Passthrough node with no upstream connection
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

    // No connections recorded — should fall back to Any
    let resolved = engine.resolve_passthrough_type("pacer", "out");
    assert_eq!(resolved, PacketType::Any, "Unresolvable Passthrough should return Any");
}
