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
    AddedInputPin { pin: InputPin, channel: tokio::sync::mpsc::Receiver<Packet> },

    /// Remove an input pin (e.g., connection deleted).
    RemoveInputPin { pin_name: String },

    /// Request to create a new output pin (less common).
    RequestAddOutputPin {
        suggested_name: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<Result<OutputPin, StreamKitError>>,
    },

    /// Engine has created the pin and channel, node should start sending.
    AddedOutputPin { pin: OutputPin, channel: tokio::sync::mpsc::Sender<Packet> },

    /// Remove an output pin.
    RemoveOutputPin { pin_name: String },
}
