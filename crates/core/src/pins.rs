// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pin system for graph validation and type checking.

use crate::types::{Packet, PacketType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::StreamKitError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[ts(export)]
pub enum PinCardinality {
    One,
    /// Output-only: packet is cloned to all connected destinations.
    Broadcast,
    /// Pins created on demand; `prefix` generates names (e.g. "in" → "in_0", "in_1").
    Dynamic {
        prefix: String,
    },
}

impl PinCardinality {
    /// Matches `prefix` exactly, or `prefix_<suffix>` (e.g. "in", "in_0").
    pub fn is_dynamic_pin_match(prefix: &str, pin: &str) -> bool {
        if pin == prefix {
            return true;
        }
        pin.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('_'))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct InputPin {
    pub name: String,
    pub accepts_types: Vec<PacketType>,
    pub cardinality: PinCardinality,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct OutputPin {
    pub name: String,
    pub produces_type: PacketType,
    pub cardinality: PinCardinality,
}

pub enum PinUpdate {
    NoChange,
    Updated { inputs: Vec<InputPin>, outputs: Vec<OutputPin> },
}

#[derive(Debug)]
pub enum PinManagementMessage {
    RequestAddInputPin {
        suggested_name: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<Result<InputPin, StreamKitError>>,
    },

    AddedInputPin {
        pin: InputPin,
        channel: tokio::sync::mpsc::Receiver<Packet>,
        hint_tx: Option<tokio::sync::mpsc::Sender<crate::UpstreamHint>>,
    },

    RemoveInputPin {
        pin_name: String,
    },

    /// Sent by the engine after `connect_nodes` for both pre-existing
    /// and dynamically created input pins.
    InputTypeResolved {
        pin_name: String,
        packet_type: PacketType,
    },

    RequestAddOutputPin {
        suggested_name: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<Result<OutputPin, StreamKitError>>,
    },

    AddedOutputPin {
        pin: OutputPin,
        channel: tokio::sync::mpsc::Sender<Packet>,
    },

    RemoveOutputPin {
        pin_name: String,
    },

    OutputHintChannel {
        pin_name: String,
        hint_rx: tokio::sync::mpsc::Receiver<crate::UpstreamHint>,
    },

    /// For pre-existing input pins (dynamic pins get `hint_tx` via `AddedInputPin`).
    AttachHintSender {
        pin_name: String,
        hint_tx: tokio::sync::mpsc::Sender<crate::UpstreamHint>,
    },
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
