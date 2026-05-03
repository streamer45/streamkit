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
#[derive(Debug, Deserialize, Serialize, TS, schemars::JsonSchema)]
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
#[derive(
    Debug,
    Deserialize,
    Serialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    TS,
    schemars::JsonSchema,
)]
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

/// Strip transient sync metadata (`_sender`, `_rev`) from a params JSON value.
///
/// The UI injects these fields for causal-consistency echo suppression.
/// Node config structs that use `#[serde(deny_unknown_fields)]` will reject
/// them during deserialization, so they must be stripped before calling
/// `serde_json::from_value`.
///
/// Nodes that need the metadata for echo suppression (e.g. the compositor)
/// should read `_sender`/`_rev` from the raw params *before* calling this
/// function.
pub fn strip_sync_metadata(params: &mut serde_json::Value) {
    if let Some(obj) = params.as_object_mut() {
        obj.remove("_sender");
        obj.remove("_rev");
    }
}

#[cfg(test)]
mod strip_sync_metadata_tests {
    use super::*;

    /// Regression test: `_sender` and `_rev` must be removed so that
    /// node config structs with `deny_unknown_fields` can deserialize
    /// successfully.
    #[test]
    fn strips_sender_and_rev() {
        let mut params = serde_json::json!({
            "_sender": "client-abc",
            "_rev": 7,
            "width": 1920,
            "height": 1080
        });

        strip_sync_metadata(&mut params);

        let obj = params.as_object().unwrap();
        assert!(!obj.contains_key("_sender"), "_sender should be stripped");
        assert!(!obj.contains_key("_rev"), "_rev should be stripped");
        assert_eq!(obj.get("width").unwrap(), 1920);
        assert_eq!(obj.get("height").unwrap(), 1080);
    }

    #[test]
    fn no_op_without_metadata() {
        let mut params = serde_json::json!({ "width": 1280 });

        strip_sync_metadata(&mut params);

        let obj = params.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj.get("width").unwrap(), 1280);
    }

    #[test]
    fn no_op_on_non_object() {
        let mut params = serde_json::json!(42);
        strip_sync_metadata(&mut params);
        assert_eq!(params, serde_json::json!(42));
    }
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
