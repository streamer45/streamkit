// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::{ClientSection, Needs, NeedsDependency, Step, UserNode, UserPipeline};
use crate::{Connection, ConnectionMode, EngineMode, Node, Pipeline};
use indexmap::IndexMap;

/// "Compiles" the user-facing pipeline format into the explicit format the engine requires.
///
/// # Errors
///
/// Returns an error if a node references a non-existent dependency in its `needs` field.
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

/// Detect cycles in the dependency graph using DFS.
///
/// Cycles that involve bidirectional nodes (like `transport::moq::peer`) are allowed,
/// as these nodes have separate input/output data paths.
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

            if def.kind == "audio::mixer" && mode != EngineMode::Dynamic {
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

            (name, Node { kind: def.kind, params, state: None })
        })
        .collect();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::{ClientSection, Needs, NeedsDependency, Step, UserNode, UserPipeline};
    use crate::{ConnectionMode, EngineMode};
    use indexmap::IndexMap;

    fn steps_pipeline(steps: Vec<Step>, client: Option<ClientSection>) -> UserPipeline {
        UserPipeline::Steps {
            name: None,
            description: None,
            mode: EngineMode::Dynamic,
            steps,
            client,
        }
    }

    fn dag_pipeline(
        nodes: IndexMap<String, UserNode>,
        client: Option<ClientSection>,
        mode: EngineMode,
    ) -> UserPipeline {
        UserPipeline::Dag { name: None, description: None, mode, nodes, client }
    }

    fn step(kind: &str) -> Step {
        Step { kind: kind.to_string(), params: None }
    }

    fn user_node(kind: &str, needs: Needs) -> UserNode {
        UserNode { kind: kind.to_string(), params: None, needs }
    }

    fn simple_dep(node: &str) -> NeedsDependency {
        NeedsDependency::Simple(node.to_string())
    }

    fn dag_nodes(entries: &[(&str, UserNode)]) -> IndexMap<String, UserNode> {
        let mut map = IndexMap::new();
        for (name, node) in entries {
            map.insert(
                (*name).to_string(),
                UserNode {
                    kind: node.kind.clone(),
                    params: node.params.clone(),
                    needs: match &node.needs {
                        Needs::None => Needs::None,
                        Needs::Single(d) => Needs::Single(clone_dep(d)),
                        Needs::Multiple(v) => Needs::Multiple(v.iter().map(clone_dep).collect()),
                        Needs::Map(m) => {
                            Needs::Map(m.iter().map(|(k, v)| (k.clone(), clone_dep(v))).collect())
                        },
                    },
                },
            );
        }
        map
    }

    fn clone_dep(d: &NeedsDependency) -> NeedsDependency {
        match d {
            NeedsDependency::Simple(s) => NeedsDependency::Simple(s.clone()),
            NeedsDependency::WithMode { node, mode } => {
                NeedsDependency::WithMode { node: node.clone(), mode: *mode }
            },
        }
    }

    #[test]
    fn compile_dispatches_steps_variant() {
        let pipeline = steps_pipeline(vec![step("a"), step("b")], None);
        let compiled = compile(pipeline).expect("steps pipeline should compile");
        assert_eq!(compiled.nodes.len(), 2);
        assert!(compiled.nodes.contains_key("step_0"));
        assert!(compiled.nodes.contains_key("step_1"));
    }

    #[test]
    fn compile_dispatches_dag_variant() {
        let nodes = dag_nodes(&[
            ("source", user_node("core::file_reader", Needs::None)),
            ("sink", user_node("core::sink", Needs::Single(simple_dep("source")))),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic))
            .expect("dag pipeline should compile");
        assert!(compiled.nodes.contains_key("source"));
        assert!(compiled.nodes.contains_key("sink"));
        assert_eq!(compiled.connections.len(), 1);
    }

    #[test]
    fn compile_steps_uses_sequential_step_naming() {
        let steps = vec![step("a"), step("b"), step("c")];
        let compiled = compile(steps_pipeline(steps, None)).expect("compile");
        let names: Vec<&str> = compiled.nodes.keys().map(String::as_str).collect();
        assert_eq!(names, ["step_0", "step_1", "step_2"]);
    }

    #[test]
    fn compile_steps_inserts_edges_between_consecutive_steps() {
        let steps = vec![step("a"), step("b"), step("c")];
        let compiled = compile(steps_pipeline(steps, None)).expect("compile");
        assert_eq!(compiled.connections.len(), 2);
        for (i, conn) in compiled.connections.iter().enumerate() {
            assert_eq!(conn.from_node, format!("step_{i}"));
            assert_eq!(conn.to_node, format!("step_{}", i + 1));
            assert_eq!(conn.from_pin, "out");
            assert_eq!(conn.to_pin, "in");
            assert_eq!(conn.mode, ConnectionMode::default());
        }
    }

    #[test]
    fn compile_steps_empty_yields_no_connections() {
        let compiled = compile(steps_pipeline(vec![], None)).expect("compile");
        assert!(compiled.nodes.is_empty());
        assert!(compiled.connections.is_empty());
    }

    #[test]
    fn compile_steps_single_step_has_no_connections() {
        let compiled = compile(steps_pipeline(vec![step("solo")], None)).expect("compile");
        assert_eq!(compiled.nodes.len(), 1);
        assert!(compiled.connections.is_empty());
    }

    #[test]
    fn compile_steps_propagates_client_section() {
        let client = ClientSection { gateway_path: Some("/gw".into()), ..ClientSection::default() };
        let compiled = compile(steps_pipeline(vec![step("a")], Some(client))).expect("compile");
        let propagated = compiled.client.expect("client section preserved");
        assert_eq!(propagated.gateway_path.as_deref(), Some("/gw"));
    }

    #[test]
    fn compile_steps_propagates_metadata() {
        let pipeline = UserPipeline::Steps {
            name: Some("demo".into()),
            description: Some("desc".into()),
            mode: EngineMode::OneShot,
            steps: vec![step("a")],
            client: None,
        };
        let compiled = compile(pipeline).expect("compile");
        assert_eq!(compiled.name.as_deref(), Some("demo"));
        assert_eq!(compiled.description.as_deref(), Some("desc"));
        assert_eq!(compiled.mode, EngineMode::OneShot);
    }

    #[test]
    fn compile_dag_resolves_single_needs_to_default_pin() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::sink", Needs::Single(simple_dep("a")))),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
        assert_eq!(compiled.connections.len(), 1);
        let conn = &compiled.connections[0];
        assert_eq!(conn.from_node, "a");
        assert_eq!(conn.from_pin, "out");
        assert_eq!(conn.to_node, "b");
        assert_eq!(conn.to_pin, "in");
    }

    #[test]
    fn compile_dag_resolves_vec_needs_to_numbered_pins() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            (
                "c",
                user_node("core::merge", Needs::Multiple(vec![simple_dep("a"), simple_dep("b")])),
            ),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
        let to_c: Vec<&Connection> =
            compiled.connections.iter().filter(|c| c.to_node == "c").collect();
        assert_eq!(to_c.len(), 2);
        let pins: Vec<&str> = to_c.iter().map(|c| c.to_pin.as_str()).collect();
        assert!(pins.contains(&"in_0"));
        assert!(pins.contains(&"in_1"));
    }

    #[test]
    fn compile_dag_unknown_needs_target_errors() {
        let nodes =
            dag_nodes(&[("consumer", user_node("core::sink", Needs::Single(simple_dep("ghost"))))]);
        let err = compile(dag_pipeline(nodes, None, EngineMode::Dynamic))
            .expect_err("missing target should error");
        assert!(err.contains("consumer"), "error mentions consumer: {err}");
        assert!(err.contains("ghost"), "error mentions ghost: {err}");
        assert!(err.contains("needs"), "error mentions needs: {err}");
    }

    #[test]
    fn compile_dag_propagates_client_section() {
        let client = ClientSection {
            relay_url: Some("https://relay.example".into()),
            ..ClientSection::default()
        };
        let nodes = dag_nodes(&[("a", user_node("core::source", Needs::None))]);
        let compiled =
            compile(dag_pipeline(nodes, Some(client), EngineMode::Dynamic)).expect("compile");
        let propagated = compiled.client.expect("client section preserved");
        assert_eq!(propagated.relay_url.as_deref(), Some("https://relay.example"));
    }

    #[test]
    fn compile_dag_preserves_needs_dependency_mode() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            (
                "b",
                user_node(
                    "core::sink",
                    Needs::Single(NeedsDependency::WithMode {
                        node: "a".into(),
                        mode: ConnectionMode::BestEffort,
                    }),
                ),
            ),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
        assert_eq!(compiled.connections[0].mode, ConnectionMode::BestEffort);
    }

    #[test]
    fn compile_dag_parses_pin_specifier_in_dep_label() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::demuxer", Needs::None)),
            ("b", user_node("core::sink", Needs::Single(simple_dep("a.video")))),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
        assert_eq!(compiled.connections[0].from_node, "a");
        assert_eq!(compiled.connections[0].from_pin, "video");
    }

    #[test]
    fn compile_dag_named_needs_map_uses_pin_names() {
        let mut map: IndexMap<String, NeedsDependency> = IndexMap::new();
        map.insert("video".into(), simple_dep("vsrc"));
        map.insert("audio".into(), simple_dep("asrc"));
        let nodes = dag_nodes(&[
            ("vsrc", user_node("core::video_source", Needs::None)),
            ("asrc", user_node("core::audio_source", Needs::None)),
            ("mux", user_node("core::muxer", Needs::Map(map))),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
        let to_mux: Vec<&Connection> =
            compiled.connections.iter().filter(|c| c.to_node == "mux").collect();
        assert_eq!(to_mux.len(), 2);
        let pins: Vec<&str> = to_mux.iter().map(|c| c.to_pin.as_str()).collect();
        assert!(pins.contains(&"video"));
        assert!(pins.contains(&"audio"));
    }

    #[test]
    fn compile_dag_rejects_node_key_in_needs_map() {
        let mut map: IndexMap<String, NeedsDependency> = IndexMap::new();
        map.insert("node".into(), simple_dep("a"));
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::sink", Needs::Map(map))),
        ]);
        let err = compile(dag_pipeline(nodes, None, EngineMode::Dynamic))
            .expect_err("`node` pin key should be rejected");
        assert!(err.contains("'node'"), "error mentions the reserved key: {err}");
    }

    #[test]
    fn compile_dag_oneshot_audio_mixer_auto_injects_num_inputs() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            (
                "mixer",
                user_node("audio::mixer", Needs::Multiple(vec![simple_dep("a"), simple_dep("b")])),
            ),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::OneShot)).expect("compile");
        let mixer = compiled.nodes.get("mixer").expect("mixer present");
        let params = mixer.params.as_ref().expect("params auto-injected");
        assert_eq!(params.get("num_inputs").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn compile_dag_dynamic_audio_mixer_does_not_inject_num_inputs() {
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            (
                "mixer",
                user_node("audio::mixer", Needs::Multiple(vec![simple_dep("a"), simple_dep("b")])),
            ),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::Dynamic)).expect("compile");
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
            needs: Needs::Multiple(vec![simple_dep("a"), simple_dep("b")]),
        };
        let nodes = dag_nodes(&[
            ("a", user_node("core::source", Needs::None)),
            ("b", user_node("core::source", Needs::None)),
            ("mixer", mixer),
        ]);
        let compiled = compile(dag_pipeline(nodes, None, EngineMode::OneShot)).expect("compile");
        let mixer = compiled.nodes.get("mixer").expect("mixer present");
        let n = mixer.params.as_ref().and_then(|p| p.get("num_inputs")).and_then(|v| v.as_u64());
        assert_eq!(n, Some(7), "user-provided num_inputs should win");
    }
}
