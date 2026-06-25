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
//!      Creating
//!          ↓
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
///      Creating
///          ↓
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
/// - `Creating` → `Initializing` (node factory completed successfully)
/// - `Creating` → `Failed` (node factory returned an error)
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
    /// Node is being created by the factory (e.g., loading ONNX models via FFI).
    /// This state is set immediately when `AddNode` is received, before the
    /// (potentially slow) constructor runs in a background task.
    Creating,

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

#[derive(Debug, Clone)]
pub struct NodeStateUpdate {
    pub node_id: String,
    pub state: NodeState,
    pub timestamp: SystemTime,
    /// Per-incarnation epoch assigned by the engine when a node instance is
    /// created. Stamped onto every emission via [`NodeStateSender`] so a stale
    /// update enqueued by an old instance can be discarded rather than applied
    /// to a new node reusing the same id (#606). `0` for emissions not bound to
    /// a generation (oneshot, direct construction in tests).
    pub generation: u64,
}

impl NodeStateUpdate {
    #[inline]
    pub fn new(node_id: String, state: NodeState) -> Self {
        Self { node_id, state, timestamp: SystemTime::now(), generation: 0 }
    }

    #[inline]
    #[must_use]
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }
}

/// A node-bound state sender that stamps every [`NodeStateUpdate`] with the
/// node instance's `generation` (epoch).
///
/// The engine assigns a fresh generation to each node incarnation and stores
/// it alongside the live node, so `handle_state_update` can discard updates an
/// older instance enqueued before a same-id node replaced it. Wrapping the raw
/// channel guarantees *every* emission site — node-internal transitions and the
/// engine's terminal backstop alike — carries the correct generation without
/// threading it through every call (#606).
#[derive(Debug, Clone)]
pub struct NodeStateSender {
    tx: tokio::sync::mpsc::Sender<NodeStateUpdate>,
    generation: u64,
}

impl NodeStateSender {
    #[inline]
    #[must_use]
    pub fn new(tx: tokio::sync::mpsc::Sender<NodeStateUpdate>, generation: u64) -> Self {
        Self { tx, generation }
    }

    #[inline]
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Stamps `update` with this sender's generation and forwards it.
    ///
    /// # Errors
    /// Returns the (stamped) update if the receiver has been dropped.
    #[inline]
    pub async fn send(
        &self,
        update: NodeStateUpdate,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<NodeStateUpdate>> {
        self.tx.send(update.with_generation(self.generation)).await
    }

    /// Stamps `update` with this sender's generation and forwards it without
    /// blocking.
    ///
    /// # Errors
    /// Returns the (stamped) update if the channel is full or closed.
    #[inline]
    pub fn try_send(
        &self,
        update: NodeStateUpdate,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<NodeStateUpdate>> {
        self.tx.try_send(update.with_generation(self.generation))
    }
}

pub mod state_helpers {
    use super::{NodeState, NodeStateSender, NodeStateUpdate, StopReason};

    /// Best-effort: failures are silently ignored.
    #[inline]
    pub fn emit_state(state_tx: &NodeStateSender, node_id: &str, state: NodeState) {
        let _ = state_tx.try_send(NodeStateUpdate::new(node_id.to_string(), state));
    }

    #[inline]
    pub fn emit_initializing(state_tx: &NodeStateSender, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Initializing);
    }

    #[inline]
    pub fn emit_ready(state_tx: &NodeStateSender, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Ready);
    }

    #[inline]
    pub fn emit_running(state_tx: &NodeStateSender, node_id: &str) {
        emit_state(state_tx, node_id, NodeState::Running);
    }

    #[inline]
    pub fn emit_stopped(state_tx: &NodeStateSender, node_id: &str, reason: impl Into<StopReason>) {
        emit_state(state_tx, node_id, NodeState::Stopped { reason: reason.into() });
    }

    #[inline]
    pub fn emit_failed(state_tx: &NodeStateSender, node_id: &str, error: impl Into<String>) {
        emit_state(state_tx, node_id, NodeState::Failed { reason: error.into() });
    }

    #[inline]
    pub fn emit_recovering(
        state_tx: &NodeStateSender,
        node_name: &str,
        reason: impl Into<String>,
        details: Option<serde_json::Value>,
    ) {
        emit_state(state_tx, node_name, NodeState::Recovering { reason: reason.into(), details });
    }

    /// Recovering state with retry attempt tracking.
    ///
    /// # Example
    /// ```no_run
    /// # use streamkit_core::state::state_helpers::emit_recovering_with_retry;
    /// # use streamkit_core::state::NodeStateSender;
    /// # use tokio::sync::mpsc;
    /// # let state_tx = NodeStateSender::new(mpsc::channel(1).0, 0);
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
        state_tx: &NodeStateSender,
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

    #[inline]
    pub fn emit_degraded(
        state_tx: &NodeStateSender,
        node_name: &str,
        reason: impl Into<String>,
        details: Option<serde_json::Value>,
    ) {
        emit_state(state_tx, node_name, NodeState::Degraded { reason: reason.into(), details });
    }
}
