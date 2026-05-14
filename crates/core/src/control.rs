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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_control_update_params_serialization_roundtrip() {
        let msg = NodeControlMessage::UpdateParams(serde_json::json!({"gain": 0.5}));
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: NodeControlMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            NodeControlMessage::UpdateParams(v) => {
                assert_eq!(v["gain"], 0.5);
            },
            _ => panic!("expected UpdateParams"),
        }
    }

    #[test]
    fn node_control_start_serialization_roundtrip() {
        let msg = NodeControlMessage::Start;
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: NodeControlMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, NodeControlMessage::Start));
    }

    #[test]
    fn node_control_shutdown_serialization_roundtrip() {
        let msg = NodeControlMessage::Shutdown;
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: NodeControlMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, NodeControlMessage::Shutdown));
    }

    #[test]
    fn connection_mode_default_is_reliable() {
        assert_eq!(ConnectionMode::default(), ConnectionMode::Reliable);
    }

    #[test]
    fn connection_mode_serialization_roundtrip() {
        for mode in [ConnectionMode::Reliable, ConnectionMode::BestEffort] {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: ConnectionMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }

    #[test]
    fn connection_mode_serde_uses_snake_case() {
        let json = serde_json::to_string(&ConnectionMode::BestEffort).unwrap();
        assert_eq!(json, "\"best_effort\"");
    }

    #[test]
    fn engine_control_add_node() {
        let msg = EngineControlMessage::AddNode {
            node_id: "node1".into(),
            kind: "gain".into(),
            params: Some(serde_json::json!({"gain": 1.0})),
        };
        match msg {
            EngineControlMessage::AddNode { node_id, kind, params } => {
                assert_eq!(node_id, "node1");
                assert_eq!(kind, "gain");
                assert!(params.is_some());
            },
            _ => panic!("expected AddNode"),
        }
    }

    #[test]
    fn engine_control_remove_node() {
        let msg = EngineControlMessage::RemoveNode { node_id: "node1".into() };
        assert!(matches!(msg, EngineControlMessage::RemoveNode { node_id } if node_id == "node1"));
    }

    #[test]
    fn engine_control_connect() {
        let msg = EngineControlMessage::Connect {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: ConnectionMode::BestEffort,
        };
        match msg {
            EngineControlMessage::Connect { from_node, from_pin, to_node, to_pin, mode } => {
                assert_eq!(from_node, "a");
                assert_eq!(from_pin, "out");
                assert_eq!(to_node, "b");
                assert_eq!(to_pin, "in");
                assert_eq!(mode, ConnectionMode::BestEffort);
            },
            _ => panic!("expected Connect"),
        }
    }

    #[test]
    fn engine_control_disconnect() {
        let msg = EngineControlMessage::Disconnect {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
        };
        assert!(matches!(msg, EngineControlMessage::Disconnect { .. }));
    }

    #[test]
    fn engine_control_tune_node() {
        let msg = EngineControlMessage::TuneNode {
            node_id: "node1".into(),
            message: NodeControlMessage::UpdateParams(serde_json::json!({"rate": 44100})),
        };
        match msg {
            EngineControlMessage::TuneNode { node_id, message } => {
                assert_eq!(node_id, "node1");
                assert!(matches!(message, NodeControlMessage::UpdateParams(_)));
            },
            _ => panic!("expected TuneNode"),
        }
    }

    #[test]
    fn engine_control_shutdown() {
        let msg = EngineControlMessage::Shutdown;
        assert!(matches!(msg, EngineControlMessage::Shutdown));
    }
}
