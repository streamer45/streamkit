// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::{ClientSection, Needs, NeedsDependency, Step, UserNode, UserPipeline};
use crate::{Connection, ConnectionMode, EngineMode, Node, Pipeline};
use indexmap::IndexMap;

pub fn compile(pipeline: UserPipeline) -> Result<Pipeline, String> {
    match pipeline {
        UserPipeline::Steps { name, description, mode, steps, client } => {
            Ok(compile_steps(name, description, mode, steps, client))
        },
        UserPipeline::Dag { name, description, mode, nodes, client } => {
            compile_dag(name, description, mode, nodes, client)
        },
    }
}

fn compile_steps(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    steps: Vec<Step>,
    client: Option<ClientSection>,
) -> Pipeline {
    let mut nodes = IndexMap::new();
    let mut connections = Vec::new();

    for (i, step) in steps.into_iter().enumerate() {
        let node_name = format!("step_{i}");

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

    Pipeline {
        name,
        description,
        mode,
        client,
        nodes,
        connections,
        view_data: None,
        runtime_schemas: None,
    }
}

const BIDIRECTIONAL_NODE_KINDS: &[&str] = &["transport::moq::peer"];

fn is_bidirectional_kind(kind: &str) -> bool {
    BIDIRECTIONAL_NODE_KINDS.contains(&kind)
}

/// Cycles involving bidirectional nodes (e.g. `transport::moq::peer`) are allowed.
fn detect_cycles(user_nodes: &IndexMap<String, UserNode>) -> Result<(), String> {
    use std::collections::HashSet;

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
                        rec_stack.remove(node);
                        cycle_path.pop();
                        return Some(cycle);
                    }
                } else if rec_stack.contains(neighbor) {
                    let cycle_start_idx =
                        cycle_path.iter().position(|&n| n == *neighbor).unwrap_or(0);
                    let cycle_nodes: Vec<&'a String> = cycle_path[cycle_start_idx..].to_vec();
                    let cycle_strs: Vec<&str> = cycle_nodes.iter().map(|s| s.as_str()).collect();
                    let description = format!(
                        "Circular dependency detected: {} -> {}",
                        cycle_strs.join(" -> "),
                        neighbor
                    );
                    rec_stack.remove(node);
                    cycle_path.pop();
                    return Some((cycle_nodes, description));
                }
            }
        }

        rec_stack.remove(node);
        cycle_path.pop();
        None
    }

    let mut adjacency: IndexMap<&String, Vec<&String>> = IndexMap::new();

    for (node_name, node_def) in user_nodes {
        adjacency.entry(node_name).or_default();

        let dependencies: Vec<&str> = match &node_def.needs {
            Needs::None => vec![],
            Needs::Single(dep) => vec![dep.node_and_pin().0],
            Needs::Multiple(deps) => deps.iter().map(|d| d.node_and_pin().0).collect(),
            Needs::Map(map) => map.values().map(|d| d.node_and_pin().0).collect(),
        };

        for dep_name in dependencies {
            if let Some((key, _)) = user_nodes.get_key_value(dep_name) {
                adjacency.entry(key).or_default().push(node_name);
            }
        }
    }

    let mut visited: HashSet<&String> = HashSet::new();
    let mut rec_stack: HashSet<&String> = HashSet::new();
    let mut cycle_path: Vec<&String> = Vec::new();

    for node_name in user_nodes.keys() {
        if !visited.contains(node_name) {
            if let Some((cycle_nodes, cycle_error)) =
                dfs(node_name, &adjacency, &mut visited, &mut rec_stack, &mut cycle_path)
            {
                let has_bidirectional = cycle_nodes.iter().any(|node_name| {
                    user_nodes.get(*node_name).is_some_and(|node| is_bidirectional_kind(&node.kind))
                });

                if !has_bidirectional {
                    return Err(cycle_error);
                }
            }
        }
    }

    Ok(())
}

fn compile_dag(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    user_nodes: IndexMap<String, UserNode>,
    client: Option<ClientSection>,
) -> Result<Pipeline, String> {
    detect_cycles(&user_nodes)?;

    let mut connections = Vec::new();

    for (node_name, node_def) in &user_nodes {
        enum DepEntry<'a> {
            Auto { idx: usize, total: usize, dep: &'a NeedsDependency },
            Named { pin: &'a str, dep: &'a NeedsDependency },
        }

        let entries: Vec<DepEntry<'_>> = match &node_def.needs {
            Needs::None => vec![],
            Needs::Single(dep) => vec![DepEntry::Auto { idx: 0, total: 1, dep }],
            Needs::Multiple(deps) => deps
                .iter()
                .enumerate()
                .map(|(idx, dep)| DepEntry::Auto { idx, total: deps.len(), dep })
                .collect(),
            Needs::Map(map) => {
                if map.contains_key("node") {
                    return Err(format!(
                        "Node '{node_name}': 'node' cannot be used as a pin name in a needs map \
                         (it collides with the WithMode dependency syntax)"
                    ));
                }
                map.iter().map(|(pin, dep)| DepEntry::Named { pin: pin.as_str(), dep }).collect()
            },
        };

        for entry in &entries {
            let (dep, to_pin) = match entry {
                DepEntry::Auto { idx, total, dep } => {
                    let pin = if *total > 1 { format!("in_{idx}") } else { "in".to_string() };
                    (*dep, pin)
                },
                DepEntry::Named { pin, dep } => (*dep, (*pin).to_string()),
            };
            let (dep_name, from_pin) = dep.node_and_pin();

            if !user_nodes.contains_key(dep_name) {
                return Err(format!(
                    "Node '{node_name}' references non-existent node '{dep_name}' in 'needs' field"
                ));
            }

            connections.push(Connection {
                from_node: dep_name.to_string(),
                from_pin: from_pin.unwrap_or("out").to_string(),
                to_node: node_name.clone(),
                to_pin,
                mode: dep.mode(),
            });
        }
    }

    let mut incoming_counts: IndexMap<String, usize> = IndexMap::new();
    for conn in &connections {
        *incoming_counts.entry(conn.to_node.clone()).or_insert(0) += 1;
    }

    let nodes = user_nodes
        .into_iter()
        .map(|(name, def)| {
            let mut params = def.params;

            if def.kind == "audio::mixer" {
                if let Some(ref p) = params {
                    if !p.is_object() {
                        return Err(format!(
                            "audio::mixer node '{name}' params must be an object, got {}",
                            value_type_name(p),
                        ));
                    }
                }

                if mode != EngineMode::Dynamic {
                    if let Some(count) = incoming_counts.get(&name) {
                        if *count > 1 {
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
            }

            Ok((name, Node { kind: def.kind, params, state: None }))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;

    Ok(Pipeline {
        name,
        description,
        mode,
        client,
        nodes,
        connections,
        view_data: None,
        runtime_schemas: None,
    })
}

fn value_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::{Needs, NeedsDependency, UserNode, UserPipeline};
    use crate::EngineMode;
    use indexmap::IndexMap;

    fn dag_nodes(entries: Vec<(&str, UserNode)>) -> IndexMap<String, UserNode> {
        entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn dag_pipeline(nodes: IndexMap<String, UserNode>, mode: EngineMode) -> UserPipeline {
        UserPipeline::Dag { name: None, description: None, mode, nodes, client: None }
    }

    fn user_node(kind: &str, needs: Needs) -> UserNode {
        UserNode { kind: kind.to_string(), params: None, needs }
    }

    fn simple_dep(node: &str) -> NeedsDependency {
        NeedsDependency::Simple(node.to_string())
    }

    fn mixer_needs() -> Needs {
        Needs::Multiple(vec![simple_dep("a"), simple_dep("b")])
    }

    #[test]
    fn compile_dag_rejects_node_key_in_needs_map() {
        let mut map: IndexMap<String, NeedsDependency> = IndexMap::new();
        map.insert("node".into(), simple_dep("a"));
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::sink", Needs::Map(map))),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("`node` pin key should be rejected");
        assert!(err.contains("'node'"), "error mentions the reserved key: {err}");
    }

    #[test]
    fn compile_dag_dynamic_audio_mixer_does_not_inject_num_inputs() {
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", user_node("audio::mixer", mixer_needs())),
        ]);
        let compiled = compile(dag_pipeline(nodes, EngineMode::Dynamic)).expect("compile");
        let mixer = compiled.nodes.get("mixer").expect("mixer present");
        assert!(mixer.params.is_none(), "dynamic mode skips auto-injection: {:?}", mixer.params);
    }

    #[test]
    fn compile_dag_audio_mixer_preserves_explicit_num_inputs() {
        let mut params = serde_json::Map::new();
        params.insert("num_inputs".into(), serde_json::Value::Number(7.into()));
        let mixer = UserNode {
            kind: "audio::mixer".to_string(),
            params: Some(serde_json::Value::Object(params)),
            needs: mixer_needs(),
        };
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", mixer),
        ]);
        let compiled = compile(dag_pipeline(nodes, EngineMode::OneShot)).expect("compile");
        let mixer = compiled.nodes.get("mixer").expect("mixer present");
        let n = mixer.params.as_ref().and_then(|p| p.get("num_inputs")).and_then(|v| v.as_u64());
        assert_eq!(n, Some(7), "user-provided num_inputs should win");
    }

    #[test]
    fn compile_dag_audio_mixer_rejects_non_object_params() {
        let mixer = UserNode {
            kind: "audio::mixer".to_string(),
            params: Some(serde_json::Value::String("scalar".into())),
            needs: mixer_needs(),
        };
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", mixer),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::OneShot))
            .expect_err("non-object params should be rejected");
        assert!(
            err.contains("audio::mixer node 'mixer' params must be an object"),
            "error should mention the node name and requirement: {err}",
        );
        assert!(err.contains("string"), "error should mention the actual type: {err}");
    }

    #[test]
    fn compile_dag_dynamic_audio_mixer_also_rejects_non_object_params() {
        let mixer = UserNode {
            kind: "audio::mixer".to_string(),
            params: Some(serde_json::Value::Number(42.into())),
            needs: mixer_needs(),
        };
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", mixer),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("non-object params should be rejected even in dynamic mode");
        assert!(err.contains("number"), "error should mention the actual type: {err}");
    }

    #[test]
    fn compile_dag_audio_mixer_rejects_null_params() {
        let mixer = UserNode {
            kind: "audio::mixer".to_string(),
            params: Some(serde_json::Value::Null),
            needs: mixer_needs(),
        };
        let nodes = dag_nodes(vec![
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", mixer),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::OneShot))
            .expect_err("null params should be rejected");
        assert!(err.contains("got null"), "error should mention null type: {err}");
    }
}
