// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Internal message types for the dynamic engine.

use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::view_data::NodeViewDataUpdate;
use tokio::sync::mpsc;

/// Unique identifier for a connection (FromNode, FromPin, ToNode, ToPin).
/// Parts stored as `Arc<str>` for cheap cloning on fan-out error paths.
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

/// Notification emitted when a node's runtime param schema is discovered
/// after initialization (e.g. Slint component properties).
#[derive(Clone, Debug)]
pub struct RuntimeSchemaUpdate {
    pub node_id: String,
    pub schema: serde_json::Value,
}

/// Emitted once a node's constructor and `initialize_node` both succeed.
/// Failures are reported via `NodeStateUpdate { state: Failed }`.
#[derive(Clone, Debug)]
pub struct NodeAddedNotification {
    pub node_id: String,
    pub kind: String,
    pub params: Option<serde_json::Value>,
    /// Incarnation epoch of the added instance (see #606), carried for
    /// observability and to correlate an add with its later removal.
    pub generation: u64,
}

/// Emitted once a node is actually torn down by the engine actor.
///
/// Its run task is aborted and its bookkeeping removed, then the session prunes
/// its pipeline snapshot from this authoritative teardown rather than
/// optimistically on the RemoveNode request, which races a confirmed-add
/// re-insert and can leave a durable orphan (#607).
///
/// `generation` is the torn-down incarnation's epoch (see #606), carried for
/// observability and to correlate a removal with the add that created it.
#[derive(Clone, Debug)]
pub struct NodeRemovedNotification {
    pub node_id: String,
    pub generation: u64,
}

/// Ordered node topology change emitted by the engine actor.
///
/// Added and removed events share ONE stream so a single session forwarder
/// applies them to the pipeline snapshot in the exact order the engine mutated
/// `live_nodes`. Separate channels would let an `Added(b)` and a later
/// `Removed(b)` be drained out of order by independent tasks, re-inserting a
/// node the engine has already torn down — the durable-orphan race in #607.
#[derive(Clone, Debug)]
pub enum NodeLifecycleNotification {
    Added(NodeAddedNotification),
    Removed(NodeRemovedNotification),
}

/// Query messages for retrieving information from the engine without modifying state.
pub enum QueryMessage {
    GetNodeStates {
        response_tx: mpsc::Sender<Arc<HashMap<String, NodeState>>>,
    },
    GetNodeStats {
        response_tx: mpsc::Sender<Arc<HashMap<String, NodeStats>>>,
    },
    SubscribeState {
        response_tx: mpsc::Sender<mpsc::Receiver<NodeStateUpdate>>,
    },
    SubscribeStats {
        response_tx: mpsc::Sender<mpsc::Receiver<NodeStatsUpdate>>,
    },
    SubscribeTelemetry {
        response_tx: mpsc::Sender<mpsc::Receiver<TelemetryEvent>>,
    },
    SubscribeViewData {
        response_tx: mpsc::Sender<mpsc::Receiver<NodeViewDataUpdate>>,
    },
    GetNodeViewData {
        response_tx: mpsc::Sender<Arc<HashMap<String, serde_json::Value>>>,
    },
    GetRuntimeSchemas {
        response_tx: mpsc::Sender<HashMap<String, serde_json::Value>>,
    },
    SubscribeRuntimeSchemas {
        response_tx: mpsc::Sender<mpsc::UnboundedReceiver<RuntimeSchemaUpdate>>,
    },
    SubscribeNodeLifecycle {
        response_tx: mpsc::Sender<mpsc::UnboundedReceiver<NodeLifecycleNotification>>,
    },
}

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
