// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Bounded metric attributes carried through the engine.
//!
//! The attributes are resolved once at the pipeline boundary (the oneshot
//! handler or dynamic session creation) and passed in as opaque label sets.
//! The engine never sees the operator policy that produced them — it only
//! merges these `KeyValue`s into the labels of the metrics it emits.

use opentelemetry::KeyValue;
use std::collections::HashMap;

/// Resolved, cardinality-bounded attributes for a pipeline run.
///
/// Merge precedence is resource ⊂ pipeline/session ⊂ node (node wins on key
/// conflict). `per_node` is the seam for future per-node declarations; it is
/// defined now but left empty.
#[derive(Debug, Clone, Default)]
pub struct ResolvedAttributes {
    /// Applied to every node's metrics (and, for oneshot, the pipeline metric).
    pub pipeline: Vec<KeyValue>,
    /// Per-node attributes, overriding pipeline attributes on key conflict.
    pub per_node: HashMap<String, Vec<KeyValue>>,
}

impl ResolvedAttributes {
    /// Labels for a node: pipeline attributes plus any per-node attributes,
    /// with per-node winning on key conflict.
    #[must_use]
    pub fn for_node(&self, node_id: &str) -> Vec<KeyValue> {
        let Some(node_attrs) = self.per_node.get(node_id) else {
            return self.pipeline.clone();
        };
        let mut merged: Vec<KeyValue> = self
            .pipeline
            .iter()
            .filter(|kv| !node_attrs.iter().any(|n| n.key == kv.key))
            .cloned()
            .collect();
        merged.extend(node_attrs.iter().cloned());
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(kvs: &[KeyValue]) -> Vec<(String, String)> {
        kvs.iter().map(|kv| (kv.key.to_string(), kv.value.to_string())).collect()
    }

    #[test]
    fn for_node_returns_pipeline_attributes_when_no_per_node() {
        let attrs = ResolvedAttributes {
            pipeline: vec![KeyValue::new("service", "tts")],
            per_node: HashMap::new(),
        };
        assert_eq!(keys(&attrs.for_node("any")), vec![("service".into(), "tts".into())]);
    }

    #[test]
    fn for_node_merges_and_lets_per_node_win_on_conflict() {
        let mut per_node = HashMap::new();
        per_node.insert(
            "n1".to_string(),
            vec![KeyValue::new("service", "stt"), KeyValue::new("stage", "decode")],
        );
        let attrs =
            ResolvedAttributes { pipeline: vec![KeyValue::new("service", "tts")], per_node };

        let mut got = keys(&attrs.for_node("n1"));
        got.sort();
        assert_eq!(got, vec![("service".into(), "stt".into()), ("stage".into(), "decode".into())]);
    }
}
