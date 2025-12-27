// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Control messages for node and engine management.
//!
//! This module defines messages used to control node lifecycle and modify
//! pipeline graphs at runtime:
//!
//! - [`NodeControlMessage`]: Messages sent to individual nodes to update parameters or control execution
//! - [`EngineControlMessage`]: Messages sent to the engine to modify the pipeline graph
//! - [`ConnectionMode`]: How a connection handles backpressure

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A message sent to a specific, running node to tune its parameters or control its lifecycle.
#[derive(Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub enum NodeControlMessage {
    UpdateParams(#[ts(type = "JsonValue")] serde_json::Value),
    /// Start signal for source nodes waiting in Ready state.
    /// Tells the node to begin producing packets.
    Start,
    /// Shutdown signal for graceful termination.
    /// Nodes should clean up resources and exit their run loop when receiving this.
    Shutdown,
}

/// Specifies how a connection handles backpressure from slow consumers.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    /// Normal connection with synchronized backpressure.
    /// If the downstream consumer is slow, the upstream producer will wait.
    /// This ensures no packet loss but can stall the pipeline.
    #[default]
    Reliable,

    /// Best-effort connection that drops packets when the downstream buffer is full.
    /// Useful for observer outputs (metrics, UI, debug taps) that shouldn't stall
    /// the main data flow. Dropped packets are logged and counted in metrics.
    BestEffort,
}

/// A message sent to the central Engine actor to modify the pipeline graph itself.
#[derive(Debug)]
pub enum EngineControlMessage {
    AddNode {
        node_id: String,
        kind: String,
        params: Option<serde_json::Value>,
    },
    RemoveNode {
        node_id: String,
    },
    Connect {
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
        mode: ConnectionMode,
    },
    Disconnect {
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    TuneNode {
        node_id: String,
        message: NodeControlMessage,
    },
    Shutdown,
}
