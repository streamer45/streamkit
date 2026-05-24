// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for connection type validation in the dynamic engine.

use super::create_test_engine;
use crate::dynamic_actor::NodePinMetadata;
use streamkit_core::types::{
    AudioCodec, AudioFormat, EncodedAudioFormat, PacketType, SampleFormat,
};
use streamkit_core::{InputPin, OutputPin, PinCardinality};
use tokio::sync::mpsc;

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
