// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! YAML diff engine for computing pipeline changes.
//!
//! Compares two compiled [`Pipeline`] snapshots and produces a [`DiffPlan`]
//! describing the minimal set of [`BatchOperation`]s needed to reconcile
//! the running session with the new definition.

use std::collections::HashSet;

use streamkit_api::{BatchOperation, Connection, ConnectionMode, Pipeline};

/// Describes how to apply a pipeline change.
#[derive(Debug)]
pub enum DiffPlan {
    /// Changes can be applied in-place via `ApplyBatch`.
    InPlace(Vec<BatchOperation>),
    /// Too different — must destroy and recreate (e.g., mode change).
    FullRebuild { reason: String },
    /// No changes detected.
    NoOp,
}

/// A connection endpoint tuple used for set comparisons.
///
/// `PartialEq`/`Hash` are derived from the four endpoint fields only (not
/// `mode`) so that the sets can identify structural additions/removals.  Mode
/// changes are detected in a separate pass inside [`diff_pipelines`].
#[derive(Debug, Clone)]
struct ConnKey {
    from_node: String,
    from_pin: String,
    to_node: String,
    to_pin: String,
    mode: ConnectionMode,
}

impl PartialEq for ConnKey {
    fn eq(&self, other: &Self) -> bool {
        self.from_node == other.from_node
            && self.from_pin == other.from_pin
            && self.to_node == other.to_node
            && self.to_pin == other.to_pin
    }
}

impl Eq for ConnKey {}

impl std::hash::Hash for ConnKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.from_node.hash(state);
        self.from_pin.hash(state);
        self.to_node.hash(state);
        self.to_pin.hash(state);
    }
}

impl From<&Connection> for ConnKey {
    fn from(c: &Connection) -> Self {
        Self {
            from_node: c.from_node.clone(),
            from_pin: c.from_pin.clone(),
            to_node: c.to_node.clone(),
            to_pin: c.to_pin.clone(),
            mode: c.mode,
        }
    }
}

/// Compare two compiled pipelines and produce a diff plan.
pub fn diff_pipelines(old: &Pipeline, new: &Pipeline) -> DiffPlan {
    // 1. If mode changed (dynamic vs oneshot), return FullRebuild
    if old.mode != new.mode {
        return DiffPlan::FullRebuild {
            reason: format!("Pipeline mode changed from {:?} to {:?}", old.mode, new.mode),
        };
    }

    let mut ops: Vec<BatchOperation> = Vec::new();

    let old_node_ids: HashSet<String> = old.nodes.keys().cloned().collect();
    let new_node_ids: HashSet<String> = new.nodes.keys().cloned().collect();

    // 2. Find nodes that exist in both but have changed kind or params → remove + re-add
    let mut changed_node_ids: HashSet<String> = HashSet::new();
    for node_id in old_node_ids.intersection(&new_node_ids) {
        let old_node = &old.nodes[node_id];
        let new_node = &new.nodes[node_id];

        let kind_changed = old_node.kind != new_node.kind;
        let params_changed = old_node.params != new_node.params;

        if kind_changed || params_changed {
            changed_node_ids.insert(node_id.clone());
        }
    }

    // 3. Compute connection sets for old and new
    let old_conns: HashSet<ConnKey> = old.connections.iter().map(ConnKey::from).collect();
    let new_conns: HashSet<ConnKey> = new.connections.iter().map(ConnKey::from).collect();

    // 4. Disconnect removed connections and connections involving changed/removed nodes
    let removed_nodes: HashSet<String> = old_node_ids.difference(&new_node_ids).cloned().collect();
    for conn in &old_conns {
        let involves_removed =
            removed_nodes.contains(&conn.from_node) || removed_nodes.contains(&conn.to_node);
        let involves_changed =
            changed_node_ids.contains(&conn.from_node) || changed_node_ids.contains(&conn.to_node);
        let is_removed = !new_conns.contains(conn);

        if involves_removed || involves_changed || is_removed {
            ops.push(BatchOperation::Disconnect {
                from_node: conn.from_node.clone(),
                from_pin: conn.from_pin.clone(),
                to_node: conn.to_node.clone(),
                to_pin: conn.to_pin.clone(),
            });
        }
    }

    // 5. Remove deleted nodes
    for node_id in &removed_nodes {
        ops.push(BatchOperation::RemoveNode { node_id: node_id.clone() });
    }

    // 6. Remove changed nodes (kind or params changed)
    for node_id in &changed_node_ids {
        ops.push(BatchOperation::RemoveNode { node_id: node_id.clone() });
    }

    // 7. Add new nodes (in new but not old)
    let added_nodes: HashSet<String> = new_node_ids.difference(&old_node_ids).cloned().collect();
    for node_id in &added_nodes {
        let node = &new.nodes[node_id];
        ops.push(BatchOperation::AddNode {
            node_id: node_id.clone(),
            kind: node.kind.clone(),
            params: node.params.clone(),
        });
    }

    // 8. Re-add changed nodes with new configuration
    for node_id in &changed_node_ids {
        let node = &new.nodes[node_id];
        ops.push(BatchOperation::AddNode {
            node_id: node_id.clone(),
            kind: node.kind.clone(),
            params: node.params.clone(),
        });
    }

    // 9. Add new connections and re-add connections for changed nodes
    for conn in &new_conns {
        let involves_changed =
            changed_node_ids.contains(&conn.from_node) || changed_node_ids.contains(&conn.to_node);
        let involves_added =
            added_nodes.contains(&conn.from_node) || added_nodes.contains(&conn.to_node);
        let is_new = !old_conns.contains(conn);

        if involves_changed || involves_added || is_new {
            ops.push(BatchOperation::Connect {
                from_node: conn.from_node.clone(),
                from_pin: conn.from_pin.clone(),
                to_node: conn.to_node.clone(),
                to_pin: conn.to_pin.clone(),
                mode: conn.mode,
            });
        }
    }

    // 10. Detect mode-only connection changes (same endpoints, different mode).
    //     These are invisible to the set diff above because ConnKey equality
    //     ignores mode.  We emit disconnect + reconnect with the new mode.
    for new_conn in &new_conns {
        if let Some(old_conn) = old_conns.get(new_conn) {
            if old_conn.mode != new_conn.mode {
                let already_handled = removed_nodes.contains(&new_conn.from_node)
                    || removed_nodes.contains(&new_conn.to_node)
                    || changed_node_ids.contains(&new_conn.from_node)
                    || changed_node_ids.contains(&new_conn.to_node)
                    || added_nodes.contains(&new_conn.from_node)
                    || added_nodes.contains(&new_conn.to_node);
                if !already_handled {
                    ops.push(BatchOperation::Disconnect {
                        from_node: new_conn.from_node.clone(),
                        from_pin: new_conn.from_pin.clone(),
                        to_node: new_conn.to_node.clone(),
                        to_pin: new_conn.to_pin.clone(),
                    });
                    ops.push(BatchOperation::Connect {
                        from_node: new_conn.from_node.clone(),
                        from_pin: new_conn.from_pin.clone(),
                        to_node: new_conn.to_node.clone(),
                        to_pin: new_conn.to_pin.clone(),
                        mode: new_conn.mode,
                    });
                }
            }
        }
    }

    if ops.is_empty() {
        DiffPlan::NoOp
    } else {
        DiffPlan::InPlace(ops)
    }
}

/// Summarize a diff plan for human-readable output.
pub fn summarize_diff(ops: &[BatchOperation]) -> String {
    let mut add_nodes = 0usize;
    let mut remove_nodes = 0usize;
    let mut add_conns = 0usize;
    let mut remove_conns = 0usize;
    for op in ops {
        match op {
            BatchOperation::AddNode { .. } => add_nodes += 1,
            BatchOperation::RemoveNode { .. } => remove_nodes += 1,
            BatchOperation::Connect { .. } => add_conns += 1,
            BatchOperation::Disconnect { .. } => remove_conns += 1,
        }
    }
    format!("+{add_nodes} node(s), -{remove_nodes} node(s), +{add_conns} connection(s), -{remove_conns} connection(s)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use streamkit_api::{EngineMode, Node};

    fn make_pipeline(
        mode: EngineMode,
        nodes: Vec<(&str, &str, Option<serde_json::Value>)>,
        connections: Vec<(&str, &str, &str, &str)>,
    ) -> Pipeline {
        let mut node_map = IndexMap::new();
        for (id, kind, params) in nodes {
            node_map.insert(id.to_string(), Node { kind: kind.to_string(), params, state: None });
        }

        let conn_vec = connections
            .into_iter()
            .map(|(fn_, fp, tn, tp)| Connection {
                from_node: fn_.to_string(),
                from_pin: fp.to_string(),
                to_node: tn.to_string(),
                to_pin: tp.to_string(),
                mode: ConnectionMode::default(),
            })
            .collect();

        Pipeline {
            name: None,
            description: None,
            mode,
            client: None,
            nodes: node_map,
            connections: conn_vec,
            view_data: None,
            runtime_schemas: None,
        }
    }

    #[test]
    fn test_noop_identical_pipelines() {
        let p = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", None), ("enc", "audio::encoder", None)],
            vec![("src", "out", "enc", "in")],
        );
        match diff_pipelines(&p, &p) {
            DiffPlan::NoOp => {},
            other => panic!("Expected NoOp, got {other:?}"),
        }
    }

    #[test]
    fn test_add_node() {
        let old = make_pipeline(EngineMode::Dynamic, vec![("src", "audio::source", None)], vec![]);
        let new = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", None), ("enc", "audio::encoder", None)],
            vec![("src", "out", "enc", "in")],
        );
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert!(ops.iter().any(
                    |op| matches!(op, BatchOperation::AddNode { node_id, .. } if node_id == "enc")
                ));
                assert!(ops.iter().any(|op| matches!(op, BatchOperation::Connect { from_node, to_node, .. } if from_node == "src" && to_node == "enc")));
            },
            other => panic!("Expected InPlace, got {other:?}"),
        }
    }

    #[test]
    fn test_remove_node() {
        let old = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", None), ("enc", "audio::encoder", None)],
            vec![("src", "out", "enc", "in")],
        );
        let new = make_pipeline(EngineMode::Dynamic, vec![("src", "audio::source", None)], vec![]);
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert!(ops.iter().any(
                    |op| matches!(op, BatchOperation::RemoveNode { node_id } if node_id == "enc")
                ));
                assert!(ops.iter().any(|op| matches!(op, BatchOperation::Disconnect { from_node, to_node, .. } if from_node == "src" && to_node == "enc")));
            },
            other => panic!("Expected InPlace, got {other:?}"),
        }
    }

    #[test]
    fn test_add_connection() {
        let old = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", None), ("enc", "audio::encoder", None)],
            vec![],
        );
        let new = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", None), ("enc", "audio::encoder", None)],
            vec![("src", "out", "enc", "in")],
        );
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert_eq!(ops.len(), 1);
                assert!(
                    matches!(&ops[0], BatchOperation::Connect { from_node, to_node, .. } if from_node == "src" && to_node == "enc")
                );
            },
            other => panic!("Expected InPlace, got {other:?}"),
        }
    }

    #[test]
    fn test_mode_change_full_rebuild() {
        let old = make_pipeline(EngineMode::Dynamic, vec![("src", "audio::source", None)], vec![]);
        let new = make_pipeline(EngineMode::OneShot, vec![("src", "audio::source", None)], vec![]);
        match diff_pipelines(&old, &new) {
            DiffPlan::FullRebuild { reason } => {
                assert!(reason.contains("mode changed"));
            },
            other => panic!("Expected FullRebuild, got {other:?}"),
        }
    }

    #[test]
    fn test_params_changed() {
        let old = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", Some(serde_json::json!({"gain": 1.0})))],
            vec![],
        );
        let new = make_pipeline(
            EngineMode::Dynamic,
            vec![("src", "audio::source", Some(serde_json::json!({"gain": 2.0})))],
            vec![],
        );
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert!(ops.iter().any(
                    |op| matches!(op, BatchOperation::RemoveNode { node_id } if node_id == "src")
                ));
                assert!(ops.iter().any(
                    |op| matches!(op, BatchOperation::AddNode { node_id, .. } if node_id == "src")
                ));
            },
            other => panic!("Expected InPlace, got {other:?}"),
        }
    }

    #[test]
    fn test_kind_changed() {
        let old = make_pipeline(EngineMode::Dynamic, vec![("enc", "audio::opus", None)], vec![]);
        let new = make_pipeline(EngineMode::Dynamic, vec![("enc", "audio::aac", None)], vec![]);
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert!(ops.iter().any(
                    |op| matches!(op, BatchOperation::RemoveNode { node_id } if node_id == "enc")
                ));
                assert!(ops.iter().any(|op| matches!(op, BatchOperation::AddNode { node_id, kind, .. } if node_id == "enc" && kind == "audio::aac")));
            },
            other => panic!("Expected InPlace, got {other:?}"),
        }
    }

    /// Build a pipeline with explicit connection modes.
    fn make_pipeline_with_modes(
        mode: EngineMode,
        nodes: Vec<(&str, &str, Option<serde_json::Value>)>,
        connections: Vec<(&str, &str, &str, &str, ConnectionMode)>,
    ) -> Pipeline {
        let mut node_map = IndexMap::new();
        for (id, kind, params) in nodes {
            node_map.insert(id.to_string(), Node { kind: kind.to_string(), params, state: None });
        }

        let conn_vec = connections
            .into_iter()
            .map(|(fn_, fp, tn, tp, m)| Connection {
                from_node: fn_.to_string(),
                from_pin: fp.to_string(),
                to_node: tn.to_string(),
                to_pin: tp.to_string(),
                mode: m,
            })
            .collect();

        Pipeline {
            name: None,
            description: None,
            mode,
            client: None,
            nodes: node_map,
            connections: conn_vec,
            view_data: None,
            runtime_schemas: None,
        }
    }

    #[test]
    fn test_connection_mode_change() {
        let nodes = vec![("src", "audio::source", None), ("enc", "audio::encoder", None)];
        let old = make_pipeline_with_modes(
            EngineMode::Dynamic,
            nodes.clone(),
            vec![("src", "out", "enc", "in", ConnectionMode::default())],
        );
        let new = make_pipeline_with_modes(
            EngineMode::Dynamic,
            nodes,
            vec![("src", "out", "enc", "in", ConnectionMode::BestEffort)],
        );
        match diff_pipelines(&old, &new) {
            DiffPlan::InPlace(ops) => {
                assert!(
                    ops.iter().any(|op| matches!(
                        op,
                        BatchOperation::Disconnect { from_node, to_node, .. }
                        if from_node == "src" && to_node == "enc"
                    )),
                    "expected Disconnect for mode change"
                );
                assert!(
                    ops.iter().any(|op| matches!(
                        op,
                        BatchOperation::Connect { from_node, to_node, mode: ConnectionMode::BestEffort, .. }
                        if from_node == "src" && to_node == "enc"
                    )),
                    "expected reconnect with BestEffort mode"
                );
            },
            other => panic!("Expected InPlace for mode change, got {other:?}"),
        }
    }
}
