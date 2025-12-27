// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! YAML pipeline format parsing and compilation.
//!
//! This module provides user-friendly YAML formats that compile to the internal Pipeline representation.
//! Supports two formats:
//! - **Steps**: Linear pipeline (`steps: [...]`)
//! - **DAG**: Directed acyclic graph (`nodes: {...}` with `needs: [...]` dependencies)

use super::{Connection, ConnectionMode, EngineMode, Node, Pipeline};
use indexmap::IndexMap;
use serde::Deserialize;

/// Represents a single step in a linear pipeline definition.
#[derive(Debug, Deserialize)]
pub struct Step {
    pub kind: String,
    pub params: Option<serde_json::Value>,
}

/// Represents a single node in a user-facing DAG pipeline definition.
#[derive(Debug, Deserialize)]
pub struct UserNode {
    pub kind: String,
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub needs: Needs,
}

/// A single dependency with optional connection mode.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NeedsDependency {
    /// Simple string: just the node name (mode defaults to Reliable)
    Simple(String),
    /// Object with node name and optional mode
    WithMode {
        node: String,
        #[serde(default)]
        mode: ConnectionMode,
    },
}

impl NeedsDependency {
    fn node(&self) -> &str {
        match self {
            Self::Simple(s) => s,
            Self::WithMode { node, .. } => node,
        }
    }

    fn mode(&self) -> ConnectionMode {
        match self {
            Self::Simple(_) => ConnectionMode::default(),
            Self::WithMode { mode, .. } => *mode,
        }
    }
}

/// Represents the `needs` field for DAG nodes.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum Needs {
    #[default]
    None,
    Single(NeedsDependency),
    Multiple(Vec<NeedsDependency>),
}

/// The top-level structure for a user-facing pipeline definition.
/// `serde(untagged)` allows it to be parsed as either a steps-based
/// pipeline or a nodes-based (DAG) pipeline.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum UserPipeline {
    Steps {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        steps: Vec<Step>,
    },
    Dag {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        nodes: IndexMap<String, UserNode>,
    },
}

/// "Compiles" the user-facing pipeline format into the explicit format the engine requires.
///
/// # Errors
///
/// Returns an error if a node references a non-existent dependency in its `needs` field.
pub fn compile(pipeline: UserPipeline) -> Result<Pipeline, String> {
    match pipeline {
        UserPipeline::Steps { name, description, mode, steps } => {
            Ok(compile_steps(name, description, mode, steps))
        },
        UserPipeline::Dag { name, description, mode, nodes } => {
            compile_dag(name, description, mode, nodes)
        },
    }
}

/// Compiles the simplified `steps` list into a Pipeline.
fn compile_steps(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    steps: Vec<Step>,
) -> Pipeline {
    let mut nodes = IndexMap::new();
    let mut connections = Vec::new();

    for (i, step) in steps.into_iter().enumerate() {
        let node_name = format!("step_{i}");

        // Create the connection from the previous step.
        if i > 0 {
            connections.push(Connection {
                from_node: format!("step_{}", i - 1),
                from_pin: "out".to_string(),
                to_node: node_name.clone(),
                to_pin: "in".to_string(),
                mode: ConnectionMode::default(),
            });
        }

        nodes.insert(node_name, Node { kind: step.kind, params: step.params, state: None });
    }

    Pipeline { name, description, mode, nodes, connections }
}

/// Known bidirectional node kinds that are allowed to participate in cycles.
/// Bidirectional nodes (like MoQ peer) have separate input/output data paths,
/// so cycles involving them are intentional and safe.
const BIDIRECTIONAL_NODE_KINDS: &[&str] = &["transport::moq::peer"];

/// Check if a node kind is bidirectional
fn is_bidirectional_kind(kind: &str) -> bool {
    BIDIRECTIONAL_NODE_KINDS.contains(&kind)
}

/// Detect cycles in the dependency graph using DFS.
///
/// Returns an error message describing the cycle if one is found.
/// Cycles that involve bidirectional nodes (like `transport::moq::peer`) are allowed,
/// as these nodes have separate input/output data paths.
fn detect_cycles(user_nodes: &IndexMap<String, UserNode>) -> Result<(), String> {
    use std::collections::HashSet;

    // DFS helper function - defined first to satisfy items_after_statements lint
    // Returns Some((cycle_nodes, cycle_description)) if a cycle is found
    fn dfs<'a>(
        node: &'a String,
        adjacency: &IndexMap<&'a String, Vec<&'a String>>,
        visited: &mut HashSet<&'a String>,
        rec_stack: &mut HashSet<&'a String>,
        cycle_path: &mut Vec<&'a String>,
    ) -> Option<(Vec<&'a String>, String)> {
        visited.insert(node);
        rec_stack.insert(node);
        cycle_path.push(node);

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if let Some(cycle) = dfs(neighbor, adjacency, visited, rec_stack, cycle_path) {
                        return Some(cycle);
                    }
                } else if rec_stack.contains(neighbor) {
                    // Found a cycle - collect the nodes in the cycle
                    let cycle_start_idx =
                        cycle_path.iter().position(|&n| n == *neighbor).unwrap_or(0);
                    let cycle_nodes: Vec<&'a String> = cycle_path[cycle_start_idx..].to_vec();
                    let cycle_strs: Vec<&str> = cycle_nodes.iter().map(|s| s.as_str()).collect();
                    let description = format!(
                        "Circular dependency detected: {} -> {}",
                        cycle_strs.join(" -> "),
                        neighbor
                    );
                    return Some((cycle_nodes, description));
                }
            }
        }

        rec_stack.remove(node);
        cycle_path.pop();
        None
    }

    // Build adjacency list (node -> nodes it depends on, i.e., edges from needs to node)
    // For cycle detection, we care about the dependency direction: if A needs B,
    // then there's an edge B -> A in the data flow graph
    let mut adjacency: IndexMap<&String, Vec<&String>> = IndexMap::new();

    for (node_name, node_def) in user_nodes {
        adjacency.entry(node_name).or_default();

        let dependencies: Vec<&str> = match &node_def.needs {
            Needs::None => vec![],
            Needs::Single(dep) => vec![dep.node()],
            Needs::Multiple(deps) => deps.iter().map(NeedsDependency::node).collect(),
        };

        for dep_name in dependencies {
            // Edge: dep_name -> node_name (data flows from dep to node)
            // We need to find the key in user_nodes to get a reference with the right lifetime
            if let Some((key, _)) = user_nodes.get_key_value(dep_name) {
                adjacency.entry(key).or_default().push(node_name);
            }
        }
    }

    // DFS-based cycle detection
    let mut visited: HashSet<&String> = HashSet::new();
    let mut rec_stack: HashSet<&String> = HashSet::new();
    let mut cycle_path: Vec<&String> = Vec::new();

    for node_name in user_nodes.keys() {
        if !visited.contains(node_name) {
            if let Some((cycle_nodes, cycle_error)) =
                dfs(node_name, &adjacency, &mut visited, &mut rec_stack, &mut cycle_path)
            {
                // Check if any node in the cycle is bidirectional
                let has_bidirectional = cycle_nodes.iter().any(|node_name| {
                    user_nodes.get(*node_name).is_some_and(|node| is_bidirectional_kind(&node.kind))
                });

                // Only report error if no bidirectional node is in the cycle
                if !has_bidirectional {
                    return Err(cycle_error);
                }
            }
        }
    }

    Ok(())
}

/// Compiles the more complex `nodes` map (DAG) into a Pipeline.
fn compile_dag(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    user_nodes: IndexMap<String, UserNode>,
) -> Result<Pipeline, String> {
    // First, detect cycles in the dependency graph
    detect_cycles(&user_nodes)?;

    let mut connections = Vec::new();

    for (node_name, node_def) in &user_nodes {
        let dependencies: Vec<&NeedsDependency> = match &node_def.needs {
            Needs::None => vec![],
            Needs::Single(dep) => vec![dep],
            Needs::Multiple(deps) => deps.iter().collect(),
        };

        for (idx, dep) in dependencies.iter().enumerate() {
            let dep_name = dep.node();

            // Validate that the referenced node exists
            if !user_nodes.contains_key(dep_name) {
                return Err(format!(
                    "Node '{node_name}' references non-existent node '{dep_name}' in 'needs' field"
                ));
            }

            // Use numbered input pins (in_0, in_1, etc.) when there are multiple inputs
            let to_pin =
                if dependencies.len() > 1 { format!("in_{idx}") } else { "in".to_string() };

            connections.push(Connection {
                from_node: dep_name.to_string(),
                from_pin: "out".to_string(),
                to_node: node_name.clone(),
                to_pin,
                mode: dep.mode(),
            });
        }
    }

    // Count incoming connections per node for auto-configuring num_inputs
    let mut incoming_counts: IndexMap<String, usize> = IndexMap::new();
    for conn in &connections {
        *incoming_counts.entry(conn.to_node.clone()).or_insert(0) += 1;
    }

    let nodes = user_nodes
        .into_iter()
        .map(|(name, def)| {
            let mut params = def.params;

            // Auto-configure num_inputs for mixer nodes with multiple inputs
            // Skip this for dynamic pipelines - dynamic mixers should handle runtime connections
            if def.kind == "audio::mixer" && mode != EngineMode::Dynamic {
                if let Some(count) = incoming_counts.get(&name) {
                    if *count > 1 {
                        // Inject num_inputs if not already set (or if it's null)
                        if let Some(serde_json::Value::Object(ref mut map)) = params {
                            let should_inject = matches!(
                                map.get("num_inputs"),
                                Some(serde_json::Value::Null) | None
                            );
                            if should_inject {
                                map.insert(
                                    "num_inputs".to_string(),
                                    serde_json::Value::Number((*count).into()),
                                );
                            }
                        } else if params.is_none() {
                            // Create params object with num_inputs
                            let mut map = serde_json::Map::new();
                            map.insert(
                                "num_inputs".to_string(),
                                serde_json::Value::Number((*count).into()),
                            );
                            params = Some(serde_json::Value::Object(map));
                        }
                    }
                }
            }

            (name, Node { kind: def.kind, params, state: None })
        })
        .collect();

    Ok(Pipeline { name, description, mode, nodes, connections })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_self_reference_needs_rejected() {
        let yaml = r"
mode: dynamic
nodes:
  peer:
    kind: test_node
    params: {}
    needs: peer
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should fail with a cycle error
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Circular dependency"),
            "Error should mention circular dependency: {err}"
        );
        assert!(err.contains("peer"), "Error should mention the node name: {err}");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_circular_needs_rejected() {
        let yaml = r"
mode: dynamic
nodes:
  node_a:
    kind: test_node
    needs: node_b
  node_b:
    kind: test_node
    needs: node_a
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should fail with a cycle error
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Circular dependency"),
            "Error should mention circular dependency: {err}"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_invalid_needs_reference() {
        let yaml = r"
mode: dynamic
nodes:
  node_a:
    kind: test_node
    needs: non_existent_node
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should fail with an error message
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("node_a"));
        assert!(err.contains("non_existent_node"));
        assert!(err.contains("needs"));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_bidirectional_transport_not_flagged_as_cycle() {
        // This test verifies that pipelines with bidirectional transport nodes
        // (like MoQ peer) don't get incorrectly flagged as cycles.
        // The bidirectionality is handled at runtime through pub/sub,
        // not through explicit `needs` dependencies.
        let yaml = r"
mode: dynamic
nodes:
  file_reader:
    kind: core::file_reader
    params:
      path: /tmp/test.opus
  ogg_demuxer:
    kind: containers::ogg::demuxer
    needs: file_reader
  pacer:
    kind: core::pacer
    needs: ogg_demuxer
  moq_publisher:
    kind: transport::moq::publisher
    params:
      broadcast: input
    needs: pacer
  moq_peer:
    kind: transport::moq::peer
    params:
      input_broadcast: input
      output_broadcast: output
  ogg_muxer:
    kind: containers::ogg::muxer
    needs: moq_peer
  file_writer:
    kind: core::file_writer
    params:
      path: /tmp/output.opus
    needs: ogg_muxer
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should compile successfully - no cycle in needs graph
        assert!(
            result.is_ok(),
            "Bidirectional transport pattern should not be flagged as a cycle: {:?}",
            result.err()
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_bidirectional_cycle_allowed() {
        // This test verifies that cycles involving bidirectional nodes (like MoQ peer)
        // are allowed. This is the pattern used by moq_transcoder pipelines where:
        // peer -> decoder -> gain -> mixer -> encoder -> peer (cycle!)
        // The cycle is intentional because the peer has separate input/output data paths.
        let yaml = r"
mode: dynamic
nodes:
  decoder:
    kind: audio::opus::decoder
    needs: moq_peer
  encoder:
    kind: audio::opus::encoder
    needs: mixer
  gain:
    kind: audio::gain
    needs: decoder
  mixer:
    kind: audio::mixer
    needs: gain
  moq_peer:
    kind: transport::moq::peer
    params:
      input_broadcast: input
      output_broadcast: output
    needs: encoder
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should compile successfully - cycles with bidirectional nodes are allowed
        assert!(
            result.is_ok(),
            "Cycle with bidirectional node should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_multiple_inputs_numbered_pins() {
        let yaml = r"
mode: dynamic
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    needs:
    - input_a
    - input_b
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 3 nodes
        assert_eq!(pipeline.nodes.len(), 3);

        // Should have 2 connections
        assert_eq!(pipeline.connections.len(), 2);

        // First connection should use in_0
        let conn_a = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "input_a")
            .expect("Should have connection from input_a");
        assert_eq!(conn_a.to_node, "mixer");
        assert_eq!(conn_a.from_pin, "out");
        assert_eq!(conn_a.to_pin, "in_0");

        // Second connection should use in_1
        let conn_b = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "input_b")
            .expect("Should have connection from input_b");
        assert_eq!(conn_b.to_node, "mixer");
        assert_eq!(conn_b.from_pin, "out");
        assert_eq!(conn_b.to_pin, "in_1");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_single_input_uses_in_pin() {
        let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  sink:
    kind: test_sink
    needs: source
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 2 nodes
        assert_eq!(pipeline.nodes.len(), 2);

        // Should have 1 connection
        assert_eq!(pipeline.connections.len(), 1);

        // Single connection should use "in" (not "in_0")
        let conn = &pipeline.connections[0];
        assert_eq!(conn.from_node, "source");
        assert_eq!(conn.to_node, "sink");
        assert_eq!(conn.from_pin, "out");
        assert_eq!(conn.to_pin, "in");
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_mixer_auto_configures_num_inputs() {
        let yaml = r"
mode: oneshot
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    params:
      # num_inputs intentionally omitted
    needs:
    - input_a
    - input_b
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // The mixer node should have num_inputs automatically set to 2 (oneshot mode)
        let mixer_node = pipeline.nodes.get("mixer").expect("mixer node should exist");
        assert_eq!(mixer_node.kind, "audio::mixer");

        // Extract num_inputs from params
        if let Some(serde_json::Value::Object(ref map)) = mixer_node.params {
            let num_inputs_value = map.get("num_inputs").expect("num_inputs should be set");
            if let serde_json::Value::Number(n) = num_inputs_value {
                assert_eq!(n.as_u64(), Some(2));
            } else {
                panic!("num_inputs should be a number");
            }
        } else {
            panic!("mixer params should be an object");
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_steps_format_compilation() {
        let yaml = r"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: audio::gain
    params:
      gain: 2.0
  - kind: streamkit::http_output
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 3 nodes with generated names
        assert_eq!(pipeline.nodes.len(), 3);
        assert!(pipeline.nodes.contains_key("step_0"));
        assert!(pipeline.nodes.contains_key("step_1"));
        assert!(pipeline.nodes.contains_key("step_2"));

        // Should have 2 connections (linear chain)
        assert_eq!(pipeline.connections.len(), 2);

        // First connection: step_0 -> step_1
        let conn0 = &pipeline.connections[0];
        assert_eq!(conn0.from_node, "step_0");
        assert_eq!(conn0.to_node, "step_1");
        assert_eq!(conn0.from_pin, "out");
        assert_eq!(conn0.to_pin, "in");

        // Second connection: step_1 -> step_2
        let conn1 = &pipeline.connections[1];
        assert_eq!(conn1.from_node, "step_1");
        assert_eq!(conn1.to_node, "step_2");

        // Verify params preserved
        let gain_node = pipeline.nodes.get("step_1").unwrap();
        assert!(gain_node.params.is_some());
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_mode_preservation() {
        // Test OneShot mode
        let yaml_oneshot = r"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
";
        let pipeline: UserPipeline = serde_saphyr::from_str(yaml_oneshot).unwrap();
        let compiled = compile(pipeline).unwrap();
        assert_eq!(compiled.mode, EngineMode::OneShot);

        // Test Dynamic mode
        let yaml_dynamic = r"
mode: dynamic
steps:
  - kind: core::passthrough
";
        let pipeline: UserPipeline = serde_saphyr::from_str(yaml_dynamic).unwrap();
        let compiled = compile(pipeline).unwrap();
        assert_eq!(compiled.mode, EngineMode::Dynamic);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_default_mode_is_dynamic() {
        let yaml = r"
# mode not specified
steps:
  - kind: core::passthrough
";
        let pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();
        assert_eq!(compiled.mode, EngineMode::Dynamic);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_name_and_description_preservation() {
        let yaml = r"
name: Test Pipeline
description: A test pipeline for validation
mode: dynamic
steps:
  - kind: core::passthrough
";
        let pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();

        assert_eq!(compiled.name, Some("Test Pipeline".to_string()));
        assert_eq!(compiled.description, Some("A test pipeline for validation".to_string()));
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_connection_mode_in_needs() {
        let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  main_sink:
    kind: test_sink
    needs: source
  metrics:
    kind: test_metrics
    needs:
      node: source
      mode: best_effort
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 3 nodes
        assert_eq!(pipeline.nodes.len(), 3);

        // Should have 2 connections
        assert_eq!(pipeline.connections.len(), 2);

        // Connection to main_sink should be Reliable (default)
        let main_conn = pipeline
            .connections
            .iter()
            .find(|c| c.to_node == "main_sink")
            .expect("Should have connection to main_sink");
        assert_eq!(main_conn.mode, ConnectionMode::Reliable);

        // Connection to metrics should be BestEffort
        let metrics_conn = pipeline
            .connections
            .iter()
            .find(|c| c.to_node == "metrics")
            .expect("Should have connection to metrics");
        assert_eq!(metrics_conn.mode, ConnectionMode::BestEffort);
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_connection_mode_in_needs_list() {
        let yaml = r"
mode: dynamic
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    needs:
      - input_a
      - node: input_b
        mode: best_effort
";

        let user_pipeline: UserPipeline = serde_saphyr::from_str(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 3 nodes
        assert_eq!(pipeline.nodes.len(), 3);

        // Should have 2 connections
        assert_eq!(pipeline.connections.len(), 2);

        // Connection from input_a should be Reliable (default, simple string syntax)
        let conn_a = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "input_a")
            .expect("Should have connection from input_a");
        assert_eq!(conn_a.mode, ConnectionMode::Reliable);
        assert_eq!(conn_a.to_pin, "in_0");

        // Connection from input_b should be BestEffort (object syntax)
        let conn_b = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "input_b")
            .expect("Should have connection from input_b");
        assert_eq!(conn_b.mode, ConnectionMode::BestEffort);
        assert_eq!(conn_b.to_pin, "in_1");
    }
}
