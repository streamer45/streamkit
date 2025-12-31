// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Node state management and lifecycle tracking.
//!
//! This module defines the state machine for node execution and provides
//! helper functions for emitting state updates.
//!
//! ## State Machine
//!
//! Nodes transition through these states during their lifecycle:
//!
//! ```text
//!     Initializing
//!          ↓
//!        Ready ──────────┐
//!          ↓             │
//!       Running ←──┐     │
//!          ↓       │     │
//!     Recovering ──┘     │
//!          ↓             │
//!       Degraded         │
//!          ↓             │
//!       Failed ←─────────┘
//!          ↓
//!       Stopped
//! ```

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use ts_rs::TS;

/// Why a node entered the `Stopped` state.
///
/// Serialized as a snake_case string for ergonomic client handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Expected end of a finite stream (typical for stateless/oneshot pipelines).
    Completed,
    /// Upstream closed, no more data to process.
    InputClosed,
    /// Downstream closed, cannot deliver outputs.
    OutputClosed,
    /// Shutdown was requested (user action or coordinated cancellation).
    Shutdown,
    /// Node cannot proceed due to missing required inputs.
    NoInputs,
    /// A reason not recognized by this client/version.
    Unknown,
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from(value.as_str()))
    }
}

impl From<&str> for StopReason {
    fn from(value: &str) -> Self {
        match value {
            "completed" => Self::Completed,
            "input_closed" => Self::InputClosed,
            "output_closed" => Self::OutputClosed,
            "shutdown" | "shutdown_requested" => Self::Shutdown,
            "no_inputs" => Self::NoInputs,
            _ => Self::Unknown,
        }
    }
}

impl From<String> for StopReason {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// Represents the runtime state of a node in the pipeline.
///
/// ## State Machine
///
/// Nodes transition through these states during their lifecycle:
///
/// ```text
///     Initializing
///          ↓
///        Ready ──────────┐
///          ↓             │
///       Running ←──┐     │
///          ↓       │     │
///     Recovering ──┘     │
///          ↓             │
///       Degraded         │
///          ↓             │
///       Failed ←─────────┘
///          ↓
///       Stopped
/// ```
///
/// ### Valid Transitions:
/// - `Initializing` → `Ready` (source nodes) or `Running` (processing nodes)
/// - `Ready` → `Running` (when pipeline is ready)
/// - `Running` → `Recovering` (temporary issues, will retry)
/// - `Running` → `Degraded` (persistent issues, no retry)
/// - `Running` → `Failed` (fatal error)
/// - `Running` → `Stopped` (graceful shutdown)
/// - `Recovering` → `Running` (recovery succeeded)
/// - `Recovering` → `Degraded` (recovery partially succeeded, quality reduced)
/// - `Recovering` → `Failed` (recovery exhausted, giving up)
/// - `Degraded` → `Failed` (conditions worsened)
/// - `Ready` → `Failed` (initialization timeout or external failure)
/// - Any state → `Stopped` (external shutdown request)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NodeState {
    /// Node is starting up and performing initialization.
    /// Examples: Opening connections, loading resources, validating configuration.
    Initializing,

    /// Node has completed initialization and is ready to process data.
    /// Source nodes (nodes with no inputs) wait in this state until all downstream
    /// nodes are also ready, preventing packet loss during pipeline startup.
    /// Non-source nodes typically skip this state and go directly to Running.
    Ready,

    /// Node is operating normally and processing data.
    /// This is the expected steady state for a healthy node.
    Running,

    /// Node encountered an issue but is actively attempting to recover automatically.
    /// The node is still running but may not be processing data during recovery.
    ///
    /// Examples:
    /// - Transport node reconnecting after connection loss
    /// - Decoder resyncing after corrupted data
    /// - Node waiting for stalled input to resume
    ///
    /// The `reason` field provides a human-readable explanation.
    /// The optional `details` field can contain node-specific structured information
    /// (e.g., retry attempt numbers, affected resources).
    Recovering {
        reason: String,
        #[ts(type = "JsonValue")]
        details: Option<serde_json::Value>,
    },

    /// Node is operational but experiencing persistent issues that affect quality or performance.
    /// Unlike `Recovering`, the node is not actively attempting automatic recovery.
    ///
    /// Examples:
    /// - High latency or packet loss in transport
    /// - Resource constraints (CPU, memory pressure)
    /// - Partial functionality (some features unavailable)
    ///
    /// The node continues processing but users should be aware of reduced quality.
    Degraded {
        reason: String,
        #[ts(type = "JsonValue")]
        details: Option<serde_json::Value>,
    },

    /// Node has encountered a fatal error and stopped processing.
    /// Manual intervention is required to restart the node.
    ///
    /// Examples:
    /// - Max reconnection attempts exhausted
    /// - Invalid configuration detected at runtime
    /// - Unrecoverable protocol error
    Failed { reason: String },

    /// Node has stopped processing and shut down.
    /// The `reason` field indicates why the node stopped:
    /// - "completed" - Expected end of finite data stream (stateless pipelines)
    /// - "input_closed" - Upstream node closed, no more data to process
    /// - "shutdown" - Graceful shutdown was requested
    ///
    /// In live/dynamic pipelines, this state often indicates an issue (unexpected stop).
    /// In stateless pipelines, "completed" is the expected end state.
    Stopped { reason: StopReason },
}

/// A state update message sent by a node to report its current state.
/// These updates are used for monitoring, debugging, and UI visualization.
#[derive(Debug, Clone)]
pub struct NodeStateUpdate {
    /// The unique identifier of the node reporting the state
    pub node_id: String,
    /// The new state of the node
    pub state: NodeState,
    /// When this state change occurred
    pub timestamp: SystemTime,
}

impl NodeStateUpdate {
    /// Creates a new state update with the current timestamp.
    #[inline]
    pub fn new(node_id: String, state: NodeState) -> Self {
        Self { node_id, state, timestamp: SystemTime::now() }
    }
}

/// Helper functions for emitting node state updates.
/// These functions reduce boilerplate when sending state updates from nodes.
pub mod state_helpers {
    use super::{NodeState, NodeStateUpdate, StopReason};
    use tokio::sync::mpsc;

    /// Emits a state update to the provided channel.
    /// Failures are silently ignored as state tracking is best-effort.
    #[inline]
    pub fn emit_state(state_tx: &mpsc::Sender<NodeStateUpdate>, node_id: &str, state: NodeState) {
        let _ = state_tx.try_send(NodeStateUpdate::new(node_id.to_string(), state));
    }

    /// Emits an Initializing state.
    #[inline]
    pub fn emit_initializing(state_tx: &mpsc::Sender<NodeStateUpdate>, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Initializing);
    }

    /// Emits a Ready state.
    #[inline]
    pub fn emit_ready(state_tx: &mpsc::Sender<NodeStateUpdate>, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Ready);
    }

    /// Emits a Running state.
    #[inline]
    pub fn emit_running(state_tx: &mpsc::Sender<NodeStateUpdate>, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Running);
    }

    /// Emits a Stopped state with the given reason.
    #[inline]
    pub fn emit_stopped(
        state_tx: &mpsc::Sender<NodeStateUpdate>,
        node_id: &str,
        reason: impl Into<StopReason>,
    ) {
        emit_state(state_tx, node_id, NodeState::Stopped { reason: reason.into() });
    }

    /// Emits a Failed state with the given error.
    #[inline]
    pub fn emit_failed(
        state_tx: &mpsc::Sender<NodeStateUpdate>,
        node_id: &str,
        error: impl Into<String>,
    ) {
        emit_state(state_tx, node_id, NodeState::Failed { reason: error.into() });
    }

    /// Emits a Recovering state with the given reason and optional details.
    #[inline]
    pub fn emit_recovering(
        state_tx: &mpsc::Sender<NodeStateUpdate>,
        node_name: &str,
        reason: impl Into<String>,
        details: Option<serde_json::Value>,
    ) {
        emit_state(state_tx, node_name, NodeState::Recovering { reason: reason.into(), details });
    }

    /// Emits a Recovering state with retry attempt tracking.
    ///
    /// This is a convenience helper for nodes implementing retry logic.
    /// The attempt count and max attempts are included in the details field
    /// for monitoring and debugging.
    ///
    /// # Example
    /// ```no_run
    /// # use streamkit_core::state::state_helpers::emit_recovering_with_retry;
    /// # use tokio::sync::mpsc;
    /// # let state_tx = mpsc::channel(1).0;
    /// emit_recovering_with_retry(
    ///     &state_tx,
    ///     "websocket_client",
    ///     "Connection lost, reconnecting",
    ///     2,
    ///     5
    /// );
    /// // Emits: Recovering { reason: "Connection lost, reconnecting",
    /// //                     details: { "attempt": 2, "max_attempts": 5 } }
    /// ```
    #[inline]
    pub fn emit_recovering_with_retry(
        state_tx: &mpsc::Sender<NodeStateUpdate>,
        node_name: &str,
        reason: impl Into<String>,
        attempt: u32,
        max_attempts: u32,
    ) {
        let details = serde_json::json!({
            "attempt": attempt,
            "max_attempts": max_attempts,
        });
        emit_recovering(state_tx, node_name, reason, Some(details));
    }

    /// Emits a Degraded state with the given reason.
    #[inline]
    pub fn emit_degraded(
        state_tx: &mpsc::Sender<NodeStateUpdate>,
        node_name: &str,
        reason: impl Into<String>,
        details: Option<serde_json::Value>,
    ) {
        emit_state(state_tx, node_name, NodeState::Degraded { reason: reason.into(), details });
    }
}
