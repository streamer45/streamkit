// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Internal message types for the dynamic engine.

use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use tokio::sync::mpsc;

/// Unique identifier for a connection (FromNode, FromPin, ToNode, ToPin).
///
/// This is carried on the control plane (connect/disconnect) and also used as the
/// key for routing state inside [`crate::dynamic_pin_distributor::PinDistributorActor`].
///
/// Performance note: packet fan-out can touch this identifier on error paths; storing
/// the parts as `Arc<str>` makes cloning cheap when needed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConnectionId {
    pub from_node: Arc<str>,
    pub from_pin: Arc<str>,
    pub to_node: Arc<str>,
    pub to_pin: Arc<str>,
}

impl ConnectionId {
    #[must_use]
    pub fn new(from_node: String, from_pin: String, to_node: String, to_pin: String) -> Self {
        Self {
            from_node: Arc::from(from_node),
            from_pin: Arc::from(from_pin),
            to_node: Arc::from(to_node),
            to_pin: Arc::from(to_pin),
        }
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{} -> {}.{}", self.from_node, self.from_pin, self.to_node, self.to_pin)
    }
}

/// Query messages for retrieving information from the engine without modifying state.
pub enum QueryMessage {
    GetNodeStates { response_tx: mpsc::Sender<HashMap<String, NodeState>> },
    GetNodeStats { response_tx: mpsc::Sender<HashMap<String, NodeStats>> },
    SubscribeState { response_tx: mpsc::Sender<mpsc::Receiver<NodeStateUpdate>> },
    SubscribeStats { response_tx: mpsc::Sender<mpsc::Receiver<NodeStatsUpdate>> },
    SubscribeTelemetry { response_tx: mpsc::Sender<mpsc::Receiver<TelemetryEvent>> },
}

// Re-export ConnectionMode from core for use by pin distributor
pub use streamkit_core::control::ConnectionMode;

/// Messages to configure the PinDistributorActor at runtime.
pub enum PinConfigMsg {
    AddConnection {
        id: ConnectionId,
        tx: mpsc::Sender<streamkit_core::types::Packet>,
        mode: ConnectionMode,
    },
    RemoveConnection {
        id: ConnectionId,
    },
    Shutdown,
}
