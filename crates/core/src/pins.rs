// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pin system for graph validation and type checking.
//!
//! This module defines the pin interface used by the graph builder to validate
//! pipeline connections before execution. Pins declare what types they accept
//! or produce, enabling pre-flight type checking.
//!
//! ## Key concepts:
//! - [`InputPin`]: Declares accepted packet types for incoming connections
//! - [`OutputPin`]: Declares the single packet type produced
//! - [`PinCardinality`]: Defines connection multiplicity (One, Broadcast, Dynamic)
//! - [`PinUpdate`]: Allows nodes to update pins during initialization
//! - [`PinManagementMessage`]: Runtime pin management for dynamic pipelines

use crate::types::{Packet, PacketType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// Re-export NodeError for PinManagementMessage
use crate::error::StreamKitError;

/// Describes the connection cardinality of a pin.
///
/// Cardinality defines how many connections a pin can have and the semantics
/// of those connections.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[ts(export)]
pub enum PinCardinality {
    /// Exactly one connection allowed.
    /// Used for simple one-to-one data flow.
    One,

    /// Multiple connections allowed (broadcast to all).
    /// Only valid for output pins.
    /// The same packet is cloned and sent to all connected destinations.
    Broadcast,

    /// Dynamic pin family - pins created on demand.
    /// The `prefix` is used to generate pin names (e.g., "in" -> "in_0", "in_1", ...).
    /// Typically used for input pins on nodes like mixers or routers.
    Dynamic { prefix: String },
}

impl PinCardinality {
    /// Returns `true` when `pin` belongs to a `Dynamic { prefix }` pin family.
    ///
    /// A match occurs when the pin name equals the prefix exactly, or when it
    /// equals the prefix followed by `_` and a suffix (e.g. prefix `"in"` matches
    /// `"in"`, `"in_0"`, `"in_foo"` but not `"inside"` or `"internal"`).
    pub fn is_dynamic_pin_match(prefix: &str, pin: &str) -> bool {
        if pin == prefix {
            return true;
        }
        pin.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('_'))
    }
}

/// Describes an input pin and the packet types it can accept.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct InputPin {
    pub name: String,
    pub accepts_types: Vec<PacketType>,
    pub cardinality: PinCardinality,
}

/// Describes an output pin and the single packet type it produces.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct OutputPin {
    pub name: String,
    pub produces_type: PacketType,
    pub cardinality: PinCardinality,
}

/// Result of node initialization.
///
/// Nodes can update their pin definitions during initialization if they need
/// to discover types or counts from external sources.
pub enum PinUpdate {
    /// Pins are unchanged from the initial definition
    NoChange,
    /// Pins have been updated (e.g., discovered from external source)
    Updated { inputs: Vec<InputPin>, outputs: Vec<OutputPin> },
}

/// Dynamic pin management messages for runtime graph modifications.
///
/// These messages allow the engine to add/remove pins dynamically in response
/// to connection changes in dynamic pipelines.
#[derive(Debug)]
pub enum PinManagementMessage {
    /// Request to create a new input pin.
    /// Node responds via oneshot channel with pin definition or error.
    RequestAddInputPin {
        suggested_name: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<Result<InputPin, StreamKitError>>,
    },

    /// Engine has created the pin and channel, node should start receiving.
    AddedInputPin {
        pin: InputPin,
        channel: tokio::sync::mpsc::Receiver<Packet>,
        /// Optional sender for upstream hints.  Present only in dynamic
        /// pipelines; the destination node stores this to send advisory
        /// hints back to the source.
        hint_tx: Option<tokio::sync::mpsc::Sender<crate::UpstreamHint>>,
    },

    /// Remove an input pin (e.g., connection deleted).
    RemoveInputPin { pin_name: String },

    /// The upstream output pin's type has been resolved for a connected
    /// input pin.  Sent by the engine after `connect_nodes` wires a
    /// connection — for both pre-existing input pins (created at node
    /// spawn time) and dynamically created ones (via `RequestAddInputPin`).
    /// This is the single mechanism for delivering upstream type info to
    /// all nodes in dynamic pipelines.
    InputTypeResolved { pin_name: String, packet_type: PacketType },

    /// Request to create a new output pin (less common).
    RequestAddOutputPin {
        suggested_name: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<Result<OutputPin, StreamKitError>>,
    },

    /// Engine has created the pin and channel, node should start sending.
    AddedOutputPin { pin: OutputPin, channel: tokio::sync::mpsc::Sender<Packet> },

    /// Remove an output pin.
    RemoveOutputPin { pin_name: String },

    /// Deliver a hint receiver to a source node's output pin.
    /// Created by the engine during `connect_nodes` and sent to the
    /// upstream (source) node so it can receive advisory hints from
    /// the downstream consumer.
    OutputHintChannel {
        pin_name: String,
        hint_rx: tokio::sync::mpsc::Receiver<crate::UpstreamHint>,
    },

    /// Attach a hint sender to a pre-existing input pin.
    ///
    /// For dynamically created pins, `hint_tx` is delivered via
    /// [`AddedInputPin`].  For pre-existing pins (declared at node
    /// definition time), the engine sends this message after
    /// `connect_nodes` so the destination node can send advisory
    /// hints back to the source.
    AttachHintSender { pin_name: String, hint_tx: tokio::sync::mpsc::Sender<crate::UpstreamHint> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioFormat, PacketType, SampleFormat};

    #[test]
    fn input_pin_construction_and_clone() {
        let pin = InputPin {
            name: "audio_in".into(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        };
        let cloned = pin.clone();
        assert_eq!(cloned.name, "audio_in");
        assert_eq!(cloned.cardinality, PinCardinality::One);
    }

    #[test]
    fn output_pin_construction_and_clone() {
        let pin = OutputPin {
            name: "audio_out".into(),
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: 48000,
                channels: 2,
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        };
        let cloned = pin.clone();
        assert_eq!(cloned.name, "audio_out");
        assert_eq!(cloned.cardinality, PinCardinality::Broadcast);
        assert_eq!(cloned.produces_type, pin.produces_type);
    }

    #[test]
    fn pin_cardinality_equality() {
        assert_eq!(PinCardinality::One, PinCardinality::One);
        assert_eq!(PinCardinality::Broadcast, PinCardinality::Broadcast);
        assert_eq!(
            PinCardinality::Dynamic { prefix: "in".into() },
            PinCardinality::Dynamic { prefix: "in".into() }
        );
        assert_ne!(PinCardinality::One, PinCardinality::Broadcast);
        assert_ne!(
            PinCardinality::Dynamic { prefix: "in".into() },
            PinCardinality::Dynamic { prefix: "out".into() }
        );
    }

    #[test]
    fn dynamic_pin_match_exact_prefix() {
        assert!(PinCardinality::is_dynamic_pin_match("in", "in"));
    }

    #[test]
    fn dynamic_pin_match_with_suffix() {
        assert!(PinCardinality::is_dynamic_pin_match("in", "in_0"));
        assert!(PinCardinality::is_dynamic_pin_match("in", "in_foo"));
    }

    #[test]
    fn dynamic_pin_no_match_partial_prefix() {
        assert!(!PinCardinality::is_dynamic_pin_match("in", "inside"));
        assert!(!PinCardinality::is_dynamic_pin_match("in", "internal"));
    }

    #[test]
    fn dynamic_pin_no_match_unrelated() {
        assert!(!PinCardinality::is_dynamic_pin_match("in", "out"));
        assert!(!PinCardinality::is_dynamic_pin_match("in", "output_0"));
    }

    #[test]
    fn pin_cardinality_serialization_roundtrip() {
        let variants = vec![
            PinCardinality::One,
            PinCardinality::Broadcast,
            PinCardinality::Dynamic { prefix: "layer".into() },
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let deserialized: PinCardinality = serde_json::from_str(&json).unwrap();
            assert_eq!(v, deserialized);
        }
    }

    #[test]
    fn pin_update_no_change_variant() {
        let update = PinUpdate::NoChange;
        assert!(matches!(update, PinUpdate::NoChange));
    }

    #[test]
    fn pin_update_updated_variant() {
        let update = PinUpdate::Updated {
            inputs: vec![InputPin {
                name: "in".into(),
                accepts_types: vec![PacketType::Text],
                cardinality: PinCardinality::One,
            }],
            outputs: vec![],
        };
        match update {
            PinUpdate::Updated { inputs, outputs } => {
                assert_eq!(inputs.len(), 1);
                assert!(outputs.is_empty());
            },
            PinUpdate::NoChange => panic!("expected Updated"),
        }
    }
}
