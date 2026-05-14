// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Per-node view data channel for emitting UI-relevant structured data.
//!
//! This module provides types and helpers for nodes to emit view data
//! (e.g., resolved compositor layout) that the frontend can consume.
//! View data is best-effort and follows the same pattern as stats/telemetry.

use std::time::SystemTime;

/// A view data update message sent by a node to report structured UI-relevant data.
///
/// Unlike stats (which are numeric counters), view data carries arbitrary JSON
/// that the frontend interprets per-node-type. For example, the compositor emits
/// its resolved layout so the frontend can render overlays at server-computed positions.
#[derive(Debug, Clone)]
pub struct NodeViewDataUpdate {
    /// The unique identifier of the node reporting the view data
    pub node_id: String,
    /// Structured view data payload (node-type-specific)
    pub data: serde_json::Value,
    /// When this update was produced
    pub timestamp: SystemTime,
}

/// Helper functions for emitting node view data updates.
/// These functions reduce boilerplate when sending view data from nodes.
pub mod view_data_helpers {
    use super::{NodeViewDataUpdate, SystemTime};
    use tokio::sync::mpsc;

    /// Emits a view data update to the provided channel.
    ///
    /// Best-effort: uses `try_send` so the node never blocks on view data emission.
    /// Failures are silently ignored as view data is informational only.
    ///
    /// The `data` closure is only called when a sender is present, avoiding
    /// unnecessary `serde_json::Value` construction when there are no subscribers.
    #[inline]
    pub fn emit_view_data(
        tx: &Option<mpsc::Sender<NodeViewDataUpdate>>,
        node_id: &str,
        data: impl FnOnce() -> serde_json::Value,
    ) {
        if let Some(tx) = tx {
            let _ = tx.try_send(NodeViewDataUpdate {
                node_id: node_id.to_string(),
                data: data(),
                timestamp: SystemTime::now(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn node_view_data_update_construction() {
        let ts = SystemTime::now();
        let update = NodeViewDataUpdate {
            node_id: "compositor_0".into(),
            data: serde_json::json!({"layers": 3}),
            timestamp: ts,
        };
        assert_eq!(update.node_id, "compositor_0");
        assert_eq!(update.data["layers"], 3);
        assert_eq!(update.timestamp, ts);
    }

    #[test]
    fn node_view_data_update_clone() {
        let update = NodeViewDataUpdate {
            node_id: "node1".into(),
            data: serde_json::json!({"key": "value"}),
            timestamp: SystemTime::now(),
        };
        let cloned = update.clone();
        assert_eq!(cloned.node_id, update.node_id);
        assert_eq!(cloned.data, update.data);
    }

    #[test]
    fn emit_view_data_with_sender() {
        let (tx, mut rx) = mpsc::channel(4);
        let sender = Some(tx);
        view_data_helpers::emit_view_data(
            &sender,
            "test_node",
            || serde_json::json!({"width": 1920}),
        );
        let update = rx.try_recv().unwrap();
        assert_eq!(update.node_id, "test_node");
        assert_eq!(update.data["width"], 1920);
    }

    #[test]
    fn emit_view_data_without_sender_is_noop() {
        let sender: Option<mpsc::Sender<NodeViewDataUpdate>> = None;
        view_data_helpers::emit_view_data(&sender, "test_node", || {
            panic!("closure should not be called when sender is None");
        });
    }

    #[test]
    fn emit_view_data_full_channel_does_not_panic() {
        let (tx, _rx) = mpsc::channel(1);
        let sender = Some(tx);
        view_data_helpers::emit_view_data(&sender, "n", || serde_json::json!(1));
        view_data_helpers::emit_view_data(&sender, "n", || serde_json::json!(2));
    }
}
