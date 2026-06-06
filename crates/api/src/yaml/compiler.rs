// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::{ClientSection, Needs, NeedsDependency, Step, UserNode, UserPipeline};
use crate::{Connection, ConnectionMode, EngineMode, Node, Pipeline};
use indexmap::IndexMap;

pub fn compile(pipeline: UserPipeline) -> Result<Pipeline, String> {
    match pipeline {
        UserPipeline::Steps { name, description, mode, attributes, steps, client } => {
            compile_steps(name, description, mode, attributes, steps, client)
        },
        UserPipeline::Dag { name, description, mode, attributes, nodes, client } => {
            compile_dag(name, description, mode, attributes, nodes, client)
        },
    }
}

fn compile_steps(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    attributes: Option<std::collections::BTreeMap<String, String>>,
    steps: Vec<Step>,
    client: Option<ClientSection>,
) -> Result<Pipeline, String> {
    let mut nodes = IndexMap::new();
    let mut connections = Vec::new();

    for (i, step) in steps.into_iter().enumerate() {
        let node_name = format!("step_{i}");

        if let Some(ref p) = step.params {
            if !p.is_object() {
                return Err(format!(
                    "Node '{node_name}' (kind '{}') params must be an object, got {}",
                    step.kind,
                    value_type_name(p),
                ));
            }
        }

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

    Ok(Pipeline {
        name,
        description,
        mode,
        attributes,
        client,
        nodes,
        connections,
        view_data: None,
        runtime_schemas: None,
    })
}

const BIDIRECTIONAL_NODE_KINDS: &[&str] = &["transport::moq::peer"];

fn is_bidirectional_kind(kind: &str) -> bool {
    BIDIRECTIONAL_NODE_KINDS.contains(&kind)
}

/// Rejects circular dependencies. Cycles routing through a bidirectional node
/// (e.g. `transport::moq::peer`) are allowed: such a node's input and output
/// are decoupled by the network round-trip, so the loop is not a local
/// dependency cycle. Edges incident to a bidirectional node are excluded from
/// the graph before running standard cycle detection on the remainder.
fn detect_cycles(user_nodes: &IndexMap<String, UserNode>) -> Result<(), String> {
    let names: Vec<&str> = user_nodes.keys().map(String::as_str).collect();
    let bidirectional: Vec<bool> =
        user_nodes.values().map(|n| is_bidirectional_kind(&n.kind)).collect();

    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); user_nodes.len()];
    for (idx, node_def) in user_nodes.values().enumerate() {
        if bidirectional[idx] {
            continue;
        }

        let dependencies: Vec<&str> = match &node_def.needs {
            Needs::None => vec![],
            Needs::Single(dep) => vec![dep.node_and_pin().0],
            Needs::Multiple(deps) => deps.iter().map(|d| d.node_and_pin().0).collect(),
            Needs::Map(map) => map.values().map(|d| d.node_and_pin().0).collect(),
        };

        for dep_name in dependencies {
            if let Some(dep_idx) = user_nodes.get_index_of(dep_name) {
                if !bidirectional[dep_idx] {
                    adjacency[dep_idx].push(idx);
                }
            }
        }
    }

    let scc = strongly_connected_components(&adjacency);

    for (from, neighbors) in adjacency.iter().enumerate() {
        for &to in neighbors {
            if scc[from] != scc[to] {
                continue;
            }
            let mut cycle = vec![names[from]];
            cycle.extend(path_within_scc(&adjacency, &scc, to, from).iter().map(|&i| names[i]));
            return Err(format!("Circular dependency detected: {}", cycle.join(" -> ")));
        }
    }

    Ok(())
}

/// Iterative Tarjan: returns the SCC id of every vertex without recursing, so
/// arbitrarily deep graphs cannot overflow the stack.
fn strongly_connected_components(adjacency: &[Vec<usize>]) -> Vec<usize> {
    let n = adjacency.len();
    let mut index = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut scc = vec![usize::MAX; n];
    let mut next_index = 0;
    let mut scc_count = 0;

    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        let mut frames: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(v, cursor)) = frames.last() {
            if cursor == 0 {
                index[v] = next_index;
                lowlink[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = adjacency[v].get(cursor) {
                frames.last_mut().expect("frame exists").1 += 1;
                if index[w] == usize::MAX {
                    frames.push((w, 0));
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(index[w]);
                }
            } else {
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    lowlink[parent] = lowlink[parent].min(lowlink[v]);
                }
                if lowlink[v] == index[v] {
                    loop {
                        let w = stack.pop().expect("SCC stack is non-empty until root is popped");
                        on_stack[w] = false;
                        scc[w] = scc_count;
                        if w == v {
                            break;
                        }
                    }
                    scc_count += 1;
                }
            }
        }
    }

    scc
}

/// BFS path from `start` to `target` restricted to `start`'s SCC; both vertices
/// must belong to the same SCC, which guarantees the path exists.
fn path_within_scc(
    adjacency: &[Vec<usize>],
    scc: &[usize],
    start: usize,
    target: usize,
) -> Vec<usize> {
    use std::collections::VecDeque;

    let mut prev = vec![usize::MAX; adjacency.len()];
    let mut seen = vec![false; adjacency.len()];
    let mut queue = VecDeque::from([start]);
    seen[start] = true;

    'bfs: while let Some(v) = queue.pop_front() {
        for &w in &adjacency[v] {
            if !seen[w] && scc[w] == scc[start] {
                seen[w] = true;
                prev[w] = v;
                if w == target {
                    break 'bfs;
                }
                queue.push_back(w);
            }
        }
    }

    let mut path = vec![target];
    let mut cur = target;
    while cur != start {
        cur = prev[cur];
        path.push(cur);
    }
    path.reverse();
    path
}

fn compile_dag(
    name: Option<String>,
    description: Option<String>,
    mode: EngineMode,
    attributes: Option<std::collections::BTreeMap<String, String>>,
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

            if let Some(ref p) = params {
                if !p.is_object() {
                    return Err(format!(
                        "Node '{name}' (kind '{}') params must be an object, got {}",
                        def.kind,
                        value_type_name(p),
                    ));
                }
            }

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
                        } else {
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

            Ok((name, Node { kind: def.kind, params, state: None }))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;

    Ok(Pipeline {
        name,
        description,
        mode,
        attributes,
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
    use crate::yaml::{Needs, NeedsDependency, Step, UserNode, UserPipeline};
    use crate::EngineMode;
    use indexmap::IndexMap;

    fn dag_nodes(entries: Vec<(&str, UserNode)>) -> IndexMap<String, UserNode> {
        entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn dag_pipeline(nodes: IndexMap<String, UserNode>, mode: EngineMode) -> UserPipeline {
        UserPipeline::Dag {
            name: None,
            description: None,
            mode,
            attributes: None,
            nodes,
            client: None,
        }
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
            err.contains("Node 'mixer' (kind 'audio::mixer') params must be an object"),
            "error should mention the node name and requirement: {err}",
        );
        assert!(err.contains("string"), "error should mention the actual type: {err}");
    }

    #[test]
    fn compile_dag_rejects_non_object_params_for_any_kind() {
        let sink = UserNode {
            kind: "core::sink".to_string(),
            params: Some(serde_json::Value::Array(vec![])),
            needs: Needs::Single(simple_dep("a")),
        };
        let nodes = dag_nodes(vec![("a", user_node("core::source", Needs::None)), ("b", sink)]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("non-object params should be rejected for all node kinds");
        assert!(
            err.contains("Node 'b' (kind 'core::sink') params must be an object"),
            "error should mention the node name and kind: {err}",
        );
        assert!(err.contains("array"), "error should mention the actual type: {err}");
    }

    #[test]
    fn compile_dag_accepts_object_params_for_any_kind() {
        let sink = UserNode {
            kind: "core::sink".to_string(),
            params: Some(serde_json::json!({"key": "value"})),
            needs: Needs::Single(simple_dep("a")),
        };
        let nodes = dag_nodes(vec![("a", user_node("core::source", Needs::None)), ("b", sink)]);
        compile(dag_pipeline(nodes, EngineMode::Dynamic)).expect("object params should compile");
    }

    #[test]
    fn detect_cycles_finds_genuine_cycle_masked_by_bidirectional_cycle() {
        // Repro from issue #533: the bidirectional cycle aaa <-> bbb is explored
        // first and must not mask the genuine cycle aaa <-> ccc.
        let nodes = dag_nodes(vec![
            (
                "aaa",
                user_node("test_node", Needs::Multiple(vec![simple_dep("bbb"), simple_dep("ccc")])),
            ),
            ("bbb", user_node("transport::moq::peer", Needs::Single(simple_dep("aaa")))),
            ("ccc", user_node("test_node", Needs::Single(simple_dep("aaa")))),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("genuine cycle should be detected even next to a bidirectional cycle");
        assert!(err.contains("Circular dependency detected"), "unexpected error: {err}");
        assert!(err.contains("aaa") && err.contains("ccc"), "error should name the cycle: {err}");
    }

    #[test]
    fn detect_cycles_rejects_plain_cycle_without_bidirectional_node() {
        let nodes = dag_nodes(vec![
            ("aaa", user_node("test_node", Needs::Single(simple_dep("ccc")))),
            ("ccc", user_node("test_node", Needs::Single(simple_dep("aaa")))),
        ]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("plain cycle should be rejected");
        assert!(err.contains("Circular dependency detected"), "unexpected error: {err}");
    }

    #[test]
    fn detect_cycles_allows_bidirectional_peer_cycle() {
        let nodes = dag_nodes(vec![
            ("aaa", user_node("test_node", Needs::Single(simple_dep("bbb")))),
            ("bbb", user_node("transport::moq::peer", Needs::Single(simple_dep("aaa")))),
        ]);
        compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect("bidirectional peer cycle should compile");
    }

    #[test]
    fn detect_cycles_allows_multi_hop_cycle_routed_through_peer() {
        // a -> b -> peer -> a: the loop is only closed via the peer's network
        // round-trip (e.g. samples/pipelines/dynamic/moq_mixing.yml), so it is
        // not a local dependency cycle.
        let nodes = dag_nodes(vec![
            ("a", user_node("test_node", Needs::Single(simple_dep("peer")))),
            ("b", user_node("test_node", Needs::Single(simple_dep("a")))),
            ("peer", user_node("transport::moq::peer", Needs::Single(simple_dep("b")))),
        ]);
        compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect("cycle routed through a peer should compile");
    }

    #[test]
    fn detect_cycles_rejects_self_reference() {
        let nodes = dag_nodes(vec![("a", user_node("test_node", Needs::Single(simple_dep("a"))))]);
        let err = compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect_err("self-referencing node should be rejected");
        assert!(err.contains("Circular dependency detected"), "unexpected error: {err}");
    }

    #[test]
    fn detect_cycles_handles_deep_linear_chain_without_overflow() {
        let mut entries = vec![("n0".to_string(), user_node("test_node", Needs::None))];
        for i in 1..20_000 {
            entries.push((
                format!("n{i}"),
                user_node("test_node", Needs::Single(simple_dep(&format!("n{}", i - 1)))),
            ));
        }
        let nodes: IndexMap<String, UserNode> = entries.into_iter().collect();
        compile(dag_pipeline(nodes, EngineMode::Dynamic))
            .expect("deep linear chain should compile without stack overflow");
    }

    #[test]
    fn compile_steps_rejects_non_object_params() {
        let steps = vec![
            Step { kind: "core::source".to_string(), params: None },
            Step { kind: "core::sink".to_string(), params: Some(serde_json::Value::Array(vec![])) },
        ];
        let pipeline = UserPipeline::Steps {
            name: None,
            description: None,
            mode: EngineMode::OneShot,
            attributes: None,
            steps,
            client: None,
        };
        let err =
            compile(pipeline).expect_err("non-object params should be rejected in steps format");
        assert!(
            err.contains("Node 'step_1' (kind 'core::sink') params must be an object"),
            "error should mention the step and kind: {err}",
        );
        assert!(err.contains("array"), "error should mention the actual type: {err}");
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
