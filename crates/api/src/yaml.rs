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
use serde::{Deserialize, Serialize};
use ts_rs::TS;

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

    /// Returns (node, from_pin) where from_pin is parsed from "node.pin" syntax if present.
    fn node_and_pin(&self) -> (&str, Option<&str>) {
        let label = self.node();
        if let Some((node, pin)) = label.split_once('.') {
            (node, Some(pin))
        } else {
            (label, None)
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
    /// Map variant: keys are **target input pin names**.
    /// Enables explicit pin targeting, e.g.
    /// ```yaml
    /// needs:
    ///   video: vp9_encoder
    ///   audio: opus_encoder
    /// ```
    ///
    /// **Note:** Because `Needs` uses `#[serde(untagged)]`, a single-entry
    /// map whose key is `"node"` (with an optional `"mode"` key matching a
    /// valid [`ConnectionMode`]) will be parsed as `Single(WithMode)` rather
    /// than `Map`.  Avoid using `node` as a pin name.
    Map(IndexMap<String, NeedsDependency>),
}

// ---------------------------------------------------------------------------
// Declarative `client` section — UI metadata for pipeline rendering
// ---------------------------------------------------------------------------

/// Top-level `client` section in pipeline YAML.
///
/// Declares what the browser UI should do when rendering this pipeline.
/// Dynamic pipelines use `relay_url`/`gateway_path`/`publish`/`watch`;
/// oneshot pipelines use `input`/`output`.  The two sets are mutually
/// exclusive by mode (enforced by the lint pass, not at parse time).
#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ClientSection {
    /// Direct relay URL for external MoQ relay pattern.
    pub relay_url: Option<String>,
    /// Gateway path for gateway-managed MoQ pattern.
    pub gateway_path: Option<String>,
    /// Browser-side publish configuration (dynamic pipelines).
    pub publish: Option<PublishConfig>,
    /// Browser-side watch configuration (dynamic pipelines).
    pub watch: Option<WatchConfig>,
    /// Input UX configuration (oneshot pipelines).
    pub input: Option<InputConfig>,
    /// Output rendering configuration (oneshot pipelines).
    pub output: Option<OutputConfig>,
}

/// Browser-side publish configuration for dynamic pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct PublishConfig {
    /// Broadcast name the browser publishes to.
    pub broadcast: String,
    /// Whether the pipeline consumes audio from the browser.
    #[serde(default)]
    pub audio: bool,
    /// Whether the pipeline consumes video from the browser.
    #[serde(default)]
    pub video: bool,
    /// Whether the browser should use screen capture (getDisplayMedia)
    /// instead of the default camera (getUserMedia).
    #[serde(default)]
    pub screen: bool,
}

/// Browser-side watch configuration for dynamic pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct WatchConfig {
    /// Broadcast name the browser subscribes to.
    pub broadcast: String,
    /// Whether the pipeline outputs audio to subscribers.
    #[serde(default)]
    pub audio: bool,
    /// Whether the pipeline outputs video to subscribers.
    #[serde(default)]
    pub video: bool,
}

/// Input UX configuration for oneshot pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct InputConfig {
    /// The kind of input UX to present.
    #[serde(rename = "type")]
    pub input_type: InputType,
    /// MIME filter for file pickers (e.g. `audio/*`).
    pub accept: Option<String>,
    /// Tags for filtering the asset picker (e.g. `["speech"]`).
    pub asset_tags: Option<Vec<String>>,
    /// Placeholder text for text inputs.
    pub placeholder: Option<String>,
    /// Per-field UI hints keyed by `http_input` field name.
    #[ts(type = "Record<string, FieldHint> | null")]
    pub field_hints: Option<IndexMap<String, FieldHint>>,
}

/// The kind of input UX a oneshot pipeline expects.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    /// File upload with optional MIME filter.
    FileUpload,
    /// Free-form text input.
    Text,
    /// Has `http_input` but the body is irrelevant (trigger only).
    Trigger,
    /// No `http_input` node — pipeline generates its own input.
    None,
}

/// Output rendering configuration for oneshot pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct OutputConfig {
    /// The media kind the pipeline produces.
    #[serde(rename = "type")]
    pub output_type: OutputType,
}

/// The media kind a oneshot pipeline produces.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    /// Transcription segments (JSON stream).
    Transcription,
    /// Generic JSON stream.
    Json,
    /// Audio output.
    Audio,
    /// Video output.
    Video,
}

/// Per-field input type discriminator for `field_hints`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// File upload field.
    File,
    /// Text input field.
    Text,
}

/// Per-field UI hint within `InputConfig.field_hints`.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct FieldHint {
    /// Override the field's input type (default is file upload).
    #[serde(rename = "type")]
    pub field_type: Option<FieldType>,
    /// MIME filter for file picker.
    pub accept: Option<String>,
    /// Placeholder text for text inputs.
    pub placeholder: Option<String>,
}

// ---------------------------------------------------------------------------
// User-facing pipeline definition
// ---------------------------------------------------------------------------

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
        /// Declarative UI metadata (optional — required only for UI rendering).
        client: Option<ClientSection>,
    },
    Dag {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        nodes: IndexMap<String, UserNode>,
        /// Declarative UI metadata (optional — required only for UI rendering).
        client: Option<ClientSection>,
    },
}

/// Parse a YAML string into a [`UserPipeline`].
///
/// Uses a two-step approach (YAML → `serde_json::Value` → `UserPipeline`)
/// to work around a `serde_saphyr` limitation where deeply nested
/// structures fail to deserialize inside `#[serde(untagged)]` enums.
///
/// # Errors
///
/// Returns an error if the YAML is malformed or doesn't match the
/// `UserPipeline` schema.
pub fn parse_yaml(yaml: &str) -> Result<UserPipeline, String> {
    let json_value: serde_json::Value =
        serde_saphyr::from_str(yaml).map_err(|e| format!("Invalid YAML: {e}"))?;

    // Pre-validate `client` if present so that enum/type errors produce
    // actionable messages instead of collapsing into the generic
    // "did not match any variant" error from the untagged `UserPipeline`.
    if let Some(client_val) = json_value.get("client") {
        let _: ClientSection = serde_json::from_value(client_val.clone())
            .map_err(|e| format!("Invalid client section: {e}"))?;
    }

    serde_json::from_value(json_value).map_err(|e| format!("Invalid pipeline: {e}"))
}

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

/// Compiles the simplified `steps` list into a Pipeline.
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

    Pipeline { name, description, mode, client, nodes, connections, view_data: None }
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
                        // Ensure we unwind recursion state even when returning early.
                        rec_stack.remove(node);
                        cycle_path.pop();
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
                    // Ensure we unwind recursion state even when returning early.
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

    // Build adjacency list (node -> nodes it depends on, i.e., edges from needs to node)
    // For cycle detection, we care about the dependency direction: if A needs B,
    // then there's an edge B -> A in the data flow graph
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
    client: Option<ClientSection>,
) -> Result<Pipeline, String> {
    // First, detect cycles in the dependency graph
    detect_cycles(&user_nodes)?;

    let mut connections = Vec::new();

    for (node_name, node_def) in &user_nodes {
        // Collect dependencies and resolve target pin names.
        // For Map variant, the map key is the explicit target pin name.
        // For Single/Multiple, pin names are auto-generated ("in" / "in_N").
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
                // Reject "node" as a pin name because it collides with the
                // NeedsDependency::WithMode struct key and would be silently
                // mis-parsed as Single(WithMode) instead of Map.
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

            // Validate that the referenced node exists
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

    Ok(Pipeline { name, description, mode, client, nodes, connections, view_data: None })
}

// ---------------------------------------------------------------------------
// Client section lint pass — semantic validation
// ---------------------------------------------------------------------------

/// A single lint warning produced by [`lint_client_section`].
#[derive(Debug, Clone)]
pub struct ClientLintWarning {
    /// Machine-readable rule identifier (e.g. `"mode-mismatch"`).
    pub rule: &'static str,
    /// Human-readable description of the problem.
    pub message: String,
}

/// Validates the `client` section against the compiled pipeline, returning
/// any semantic warnings.
///
/// This is a *lint* pass — it never prevents compilation, but surfaces
/// likely authoring mistakes so tooling (CLI, editor integrations) can
/// flag them.
///
/// # Rules
///
///  1. **`mode-mismatch-dynamic`** — Dynamic pipeline declares oneshot-only
///     fields (`input` / `output`).
///  2. **`mode-mismatch-oneshot`** — Oneshot pipeline declares dynamic-only
///     fields (`publish` / `watch` / `gateway_path` / `relay_url`).
///  3. **`missing-gateway`** — Dynamic pipeline has `publish` or `watch`
///     but no `gateway_path` or `relay_url`.
///  4. **`publish-no-media`** — `publish` block sets both `audio` and
///     `video` to false.
///  5. **`watch-no-media`** — `watch` block sets both `audio` and `video`
///     to false.
///  6. **`input-none-with-accept`** — `input.type` is `none` but `accept`
///     is set (accept is meaningless without a file picker).
///  7. **`input-trigger-with-accept`** — `input.type` is `trigger` but
///     `accept` is set.
///  8. **`field-hints-no-input`** — `field_hints` is present but
///     `input.type` is `none`.
///  9. **`asset-tags-no-input`** — `asset_tags` is present but
///     `input.type` is `none` or `text`.
/// 10. **`text-no-placeholder`** — `input.type` is `text` but no
///     `placeholder` is provided (best-practice hint).
/// 11. **`empty-broadcast`** — `publish.broadcast` or `watch.broadcast`
///     is an empty string.
/// 12. **`duplicate-broadcast`** — `publish.broadcast` equals
///     `watch.broadcast` (would cause a loop).
/// 13. **`screen-source-no-video`** — `publish.screen` is `true`
///     but `video` is `false` (screen sharing requires video).
pub fn lint_client_section(client: &ClientSection, mode: EngineMode) -> Vec<ClientLintWarning> {
    let mut warnings = Vec::new();

    let has_dynamic_fields = client.gateway_path.is_some()
        || client.relay_url.is_some()
        || client.publish.is_some()
        || client.watch.is_some();

    let has_oneshot_fields = client.input.is_some() || client.output.is_some();

    // Rule 1: dynamic pipeline with oneshot-only fields
    if mode == EngineMode::Dynamic && has_oneshot_fields {
        warnings.push(ClientLintWarning {
            rule: "mode-mismatch-dynamic",
            message: "Dynamic pipeline declares `input` or `output` — these are oneshot-only \
                      fields and will be ignored."
                .into(),
        });
    }

    // Rule 2: oneshot pipeline with dynamic-only fields
    if mode == EngineMode::OneShot && has_dynamic_fields {
        warnings.push(ClientLintWarning {
            rule: "mode-mismatch-oneshot",
            message: "Oneshot pipeline declares `publish`, `watch`, `gateway_path`, or \
                      `relay_url` — these are dynamic-only fields and will be ignored."
                .into(),
        });
    }

    // Rule 3: missing gateway
    if (client.publish.is_some() || client.watch.is_some())
        && client.gateway_path.is_none()
        && client.relay_url.is_none()
    {
        warnings.push(ClientLintWarning {
            rule: "missing-gateway",
            message: "Pipeline has `publish` or `watch` but no `gateway_path` or `relay_url` — \
                      the browser won't know where to connect."
                .into(),
        });
    }

    // Rule 4: publish with no media
    if let Some(ref publish) = client.publish {
        if !publish.audio && !publish.video {
            warnings.push(ClientLintWarning {
                rule: "publish-no-media",
                message: "publish block sets both `audio` and `video` to false — nothing will be \
                          sent from the browser."
                    .into(),
            });
        }

        // Rule 4b: screen is true but video is false
        if publish.screen && !publish.video {
            warnings.push(ClientLintWarning {
                rule: "screen-source-no-video",
                message: "publish.screen is `true` but `video` is false — screen sharing \
                          requires video to be enabled."
                    .into(),
            });
        }

        // Rule 11a: empty broadcast
        if publish.broadcast.is_empty() {
            warnings.push(ClientLintWarning {
                rule: "empty-broadcast",
                message: "publish.broadcast is an empty string.".into(),
            });
        }
    }

    // Rule 5: watch with no media
    if let Some(ref watch) = client.watch {
        if !watch.audio && !watch.video {
            warnings.push(ClientLintWarning {
                rule: "watch-no-media",
                message: "watch block sets both `audio` and `video` to false — nothing will be \
                          received by the browser."
                    .into(),
            });
        }

        // Rule 11b: empty broadcast
        if watch.broadcast.is_empty() {
            warnings.push(ClientLintWarning {
                rule: "empty-broadcast",
                message: "watch.broadcast is an empty string.".into(),
            });
        }
    }

    // Rule 12: duplicate broadcast
    if let (Some(ref publish), Some(ref watch)) = (&client.publish, &client.watch) {
        if !publish.broadcast.is_empty() && publish.broadcast == watch.broadcast {
            warnings.push(ClientLintWarning {
                rule: "duplicate-broadcast",
                message: format!(
                    "publish.broadcast and watch.broadcast are both '{}' — this would \
                     cause a feedback loop.",
                    publish.broadcast
                ),
            });
        }
    }

    // Input-related rules
    if let Some(ref input) = client.input {
        // Rule 6: input none with accept
        if matches!(input.input_type, InputType::None) && input.accept.is_some() {
            warnings.push(ClientLintWarning {
                rule: "input-none-with-accept",
                message: "input.type is `none` but `accept` is set — accept is meaningless \
                          without a file picker."
                    .into(),
            });
        }

        // Rule 7: input trigger with accept
        if matches!(input.input_type, InputType::Trigger) && input.accept.is_some() {
            warnings.push(ClientLintWarning {
                rule: "input-trigger-with-accept",
                message: "input.type is `trigger` but `accept` is set — accept is meaningless \
                          for trigger inputs."
                    .into(),
            });
        }

        // Rule 8: field_hints with no input
        if matches!(input.input_type, InputType::None)
            && input.field_hints.as_ref().is_some_and(|h| !h.is_empty())
        {
            warnings.push(ClientLintWarning {
                rule: "field-hints-no-input",
                message: "field_hints is present but input.type is `none` — hints are unused \
                          without an input."
                    .into(),
            });
        }

        // Rule 9: asset_tags with no input or text input
        if matches!(input.input_type, InputType::None | InputType::Text)
            && input.asset_tags.as_ref().is_some_and(|t| !t.is_empty())
        {
            warnings.push(ClientLintWarning {
                rule: "asset-tags-no-input",
                message: "asset_tags is present but input.type is `none` or `text` — tags are \
                          only useful for file_upload inputs."
                    .into(),
            });
        }

        // Rule 10: text input without placeholder
        if matches!(input.input_type, InputType::Text) && input.placeholder.is_none() {
            warnings.push(ClientLintWarning {
                rule: "text-no-placeholder",
                message: "input.type is `text` but no `placeholder` is provided — consider \
                          adding one for a better UX."
                    .into(),
            });
        }
    }

    warnings
}

/// A lightweight view of a pipeline's nodes used by
/// [`lint_client_against_nodes`] for cross-validation.
///
/// Callers construct this from either `UserPipeline::Dag` nodes or
/// `UserPipeline::Steps` steps.
pub struct NodeInfo<'a> {
    pub kind: &'a str,
    pub params: Option<&'a serde_json::Value>,
}

/// Cross-validates the `client` section against the pipeline's node graph.
///
/// This is a second lint layer that complements [`lint_client_section`]
/// (which checks `client` in isolation).  The rules here require knowledge
/// of which nodes exist and their params.
///
/// # Rules
///
/// 13. **`input-requires-http-input`** — `input.type` is `file_upload`,
///     `text`, or `trigger` but no `streamkit::http_input` node exists.
/// 14. **`input-none-has-http-input`** — `input.type` is `none` but an
///     `streamkit::http_input` node exists (should be `trigger`).
/// 15. **`field-hint-unknown-field`** — `field_hints` references a field
///     name not found in any `streamkit::http_input` node's `fields` param.
/// 16. **`publish-no-transport`** — `publish` is declared but no MoQ
///     transport node (`transport::moq::peer` or
///     `transport::moq::subscriber`) exists.
/// 17. **`watch-no-transport`** — `watch` is declared but no MoQ transport
///     node (`transport::moq::peer` or `transport::moq::publisher`) exists.
/// 18. **`gateway-path-mismatch`** — `client.gateway_path` does not match
///     the `gateway_path` param on a `transport::moq::peer` node.
/// 19. **`relay-url-mismatch`** — `client.relay_url` does not match the
///     `url` param on a `transport::moq::publisher` or
///     `transport::moq::subscriber` node.
/// 20. **`broadcast-mismatch`** — `publish.broadcast` or `watch.broadcast`
///     does not match any broadcast name configured on MoQ transport nodes.
pub fn lint_client_against_nodes(
    client: &ClientSection,
    _mode: EngineMode,
    nodes: &[NodeInfo<'_>],
) -> Vec<ClientLintWarning> {
    let mut warnings = Vec::new();

    // Collect node kinds and params for efficient lookup.
    let has_http_input = nodes.iter().any(|n| n.kind == "streamkit::http_input");
    let has_moq_peer = nodes.iter().any(|n| n.kind == "transport::moq::peer");
    let has_moq_subscriber = nodes.iter().any(|n| n.kind == "transport::moq::subscriber");
    let has_moq_publisher = nodes.iter().any(|n| n.kind == "transport::moq::publisher");

    // Rule 13: input requires http_input node
    if let Some(ref input) = client.input {
        let needs_http_input = matches!(
            input.input_type,
            InputType::FileUpload | InputType::Text | InputType::Trigger
        );
        if needs_http_input && !has_http_input {
            warnings.push(ClientLintWarning {
                rule: "input-requires-http-input",
                message: format!(
                    "input.type is `{}` but no `streamkit::http_input` node exists.",
                    match input.input_type {
                        InputType::FileUpload => "file_upload",
                        InputType::Text => "text",
                        InputType::Trigger => "trigger",
                        InputType::None => "none",
                    }
                ),
            });
        }

        // Rule 14: input.type is none but http_input exists
        if matches!(input.input_type, InputType::None) && has_http_input {
            warnings.push(ClientLintWarning {
                rule: "input-none-has-http-input",
                message: "input.type is `none` but a `streamkit::http_input` node exists — \
                          consider using `trigger` instead."
                    .into(),
            });
        }

        // Rule 15: field_hints references unknown field names
        if let Some(ref hints) = input.field_hints {
            let mut declared_fields: Vec<String> = Vec::new();
            for node in nodes.iter().filter(|n| n.kind == "streamkit::http_input") {
                if let Some(params) = node.params {
                    // Single field: { field: { name: "foo" } }
                    if let Some(name) =
                        params.get("field").and_then(|f| f.get("name")).and_then(|n| n.as_str())
                    {
                        declared_fields.push(name.to_string());
                    }
                    // Multi field: { fields: [{ name: "foo" }, { name: "bar" }] }
                    if let Some(fields_arr) = params.get("fields").and_then(|f| f.as_array()) {
                        for f in fields_arr {
                            if let Some(name) = f.get("name").and_then(|n| n.as_str()) {
                                declared_fields.push(name.to_string());
                            }
                        }
                    }
                }
                // http_input with no params has a single default field named "media"
                if (node.params.is_none()
                    || node
                        .params
                        .is_none_or(|p| p.get("field").is_none() && p.get("fields").is_none()))
                    && !declared_fields.contains(&"media".to_string())
                {
                    declared_fields.push("media".to_string());
                }
            }

            if !declared_fields.is_empty() {
                for hint_name in hints.keys() {
                    if !declared_fields.iter().any(|f| f == hint_name) {
                        warnings.push(ClientLintWarning {
                            rule: "field-hint-unknown-field",
                            message: format!(
                                "field_hints references `{hint_name}` but no `streamkit::http_input` \
                                 node declares a field with that name. Known fields: {}.",
                                declared_fields.join(", ")
                            ),
                        });
                    }
                }
            }
        }
    }

    // Rule 16: publish but no MoQ subscriber/peer
    // Browser publish = server subscribes → need moq::peer or moq::subscriber
    if client.publish.is_some() && !has_moq_peer && !has_moq_subscriber {
        warnings.push(ClientLintWarning {
            rule: "publish-no-transport",
            message: "client declares `publish` but no `transport::moq::peer` or \
                      `transport::moq::subscriber` node exists."
                .into(),
        });
    }

    // Rule 17: watch but no MoQ publisher/peer
    // Browser watch = server publishes → need moq::peer or moq::publisher
    if client.watch.is_some() && !has_moq_peer && !has_moq_publisher {
        warnings.push(ClientLintWarning {
            rule: "watch-no-transport",
            message: "client declares `watch` but no `transport::moq::peer` or \
                      `transport::moq::publisher` node exists."
                .into(),
        });
    }

    // Rule 18: gateway_path mismatch with moq::peer node
    if let Some(ref client_gw) = client.gateway_path {
        let peer_gateway_paths: Vec<&str> = nodes
            .iter()
            .filter(|n| n.kind == "transport::moq::peer")
            .filter_map(|n| n.params.and_then(|p| p.get("gateway_path")).and_then(|v| v.as_str()))
            .collect();

        if !peer_gateway_paths.is_empty() && !peer_gateway_paths.iter().any(|gw| gw == client_gw) {
            warnings.push(ClientLintWarning {
                rule: "gateway-path-mismatch",
                message: format!(
                    "client.gateway_path is `{client_gw}` but moq::peer node(s) declare: {}.",
                    peer_gateway_paths.join(", ")
                ),
            });
        }
    }

    // Rule 19: relay_url mismatch with publisher/subscriber nodes
    if let Some(ref client_url) = client.relay_url {
        let node_urls: Vec<&str> = nodes
            .iter()
            .filter(|n| {
                n.kind == "transport::moq::publisher" || n.kind == "transport::moq::subscriber"
            })
            .filter_map(|n| n.params.and_then(|p| p.get("url")).and_then(|v| v.as_str()))
            .collect();

        if !node_urls.is_empty() && !node_urls.iter().any(|u| u == client_url) {
            warnings.push(ClientLintWarning {
                rule: "relay-url-mismatch",
                message: format!(
                    "client.relay_url is `{client_url}` but transport node(s) declare: {}.",
                    node_urls.join(", ")
                ),
            });
        }
    }

    // Rule 20: broadcast name mismatch
    // Collect all broadcast names from MoQ transport nodes.
    let mut node_broadcasts: Vec<&str> = Vec::new();
    for node in nodes {
        if let Some(params) = node.params {
            match node.kind {
                "transport::moq::peer" => {
                    if let Some(b) = params.get("input_broadcast").and_then(|v| v.as_str()) {
                        node_broadcasts.push(b);
                    }
                    if let Some(b) = params.get("output_broadcast").and_then(|v| v.as_str()) {
                        node_broadcasts.push(b);
                    }
                },
                "transport::moq::publisher" | "transport::moq::subscriber" => {
                    if let Some(b) = params.get("broadcast").and_then(|v| v.as_str()) {
                        node_broadcasts.push(b);
                    }
                },
                _ => {},
            }
        }
    }

    if !node_broadcasts.is_empty() {
        if let Some(ref publish) = client.publish {
            if !publish.broadcast.is_empty()
                && !node_broadcasts.iter().any(|b| *b == publish.broadcast)
            {
                warnings.push(ClientLintWarning {
                    rule: "broadcast-mismatch",
                    message: format!(
                        "publish.broadcast is `{}` but no MoQ transport node declares \
                         that broadcast name. Node broadcasts: {}.",
                        publish.broadcast,
                        node_broadcasts.join(", ")
                    ),
                });
            }
        }
        if let Some(ref watch) = client.watch {
            if !watch.broadcast.is_empty() && !node_broadcasts.iter().any(|b| *b == watch.broadcast)
            {
                warnings.push(ClientLintWarning {
                    rule: "broadcast-mismatch",
                    message: format!(
                        "watch.broadcast is `{}` but no MoQ transport node declares \
                         that broadcast name. Node broadcasts: {}.",
                        watch.broadcast,
                        node_broadcasts.join(", ")
                    ),
                });
            }
        }
    }

    warnings
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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
    needs:
      in: moq_peer.audio/data
  file_writer:
    kind: core::file_writer
    params:
      path: /tmp/output.opus
    needs: ogg_muxer
";

        let user_pipeline = parse_yaml(yaml).unwrap();
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
    needs:
      in: moq_peer.audio/data
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

        let user_pipeline = parse_yaml(yaml).unwrap();
        let result = compile(user_pipeline);

        // Should compile successfully - cycles with bidirectional nodes are allowed
        assert!(
            result.is_ok(),
            "Cycle with bidirectional node should be allowed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_sample_moq_mixing_compiles() {
        let yaml = include_str!("../../../samples/pipelines/dynamic/moq_mixing.yml");
        let user_pipeline = parse_yaml(yaml).unwrap();
        let result = compile(user_pipeline);

        assert!(
            result.is_ok(),
            "Sample pipeline moq_mixing.yml should compile: {:?}",
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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
        let pipeline = parse_yaml(yaml_oneshot).unwrap();
        let compiled = compile(pipeline).unwrap();
        assert_eq!(compiled.mode, EngineMode::OneShot);

        // Test Dynamic mode
        let yaml_dynamic = r"
mode: dynamic
steps:
  - kind: core::passthrough
";
        let pipeline = parse_yaml(yaml_dynamic).unwrap();
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
        let pipeline = parse_yaml(yaml).unwrap();
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
        let pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

        let user_pipeline = parse_yaml(yaml).unwrap();
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

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_needs_map_explicit_pin_targeting() {
        let yaml = r"
mode: dynamic
nodes:
  vp9_encoder:
    kind: video::vp9_encoder
  opus_encoder:
    kind: audio::opus_encoder
  muxer:
    kind: containers::webm_muxer
    needs:
      video: vp9_encoder
      audio: opus_encoder
";

        let user_pipeline = parse_yaml(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        // Should have 3 nodes
        assert_eq!(pipeline.nodes.len(), 3);

        // Should have 2 connections
        assert_eq!(pipeline.connections.len(), 2);

        // Connection from vp9_encoder should target the "video" pin
        let video_conn = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "vp9_encoder")
            .expect("Should have connection from vp9_encoder");
        assert_eq!(video_conn.to_node, "muxer");
        assert_eq!(video_conn.to_pin, "video");
        assert_eq!(video_conn.from_pin, "out");

        // Connection from opus_encoder should target the "audio" pin
        let audio_conn = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "opus_encoder")
            .expect("Should have connection from opus_encoder");
        assert_eq!(audio_conn.to_node, "muxer");
        assert_eq!(audio_conn.to_pin, "audio");
        assert_eq!(audio_conn.from_pin, "out");
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn test_needs_map_with_output_pin_specifier() {
        let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  sink:
    kind: test_sink
    needs:
      my_input: source.alt_out
";

        let user_pipeline = parse_yaml(yaml).unwrap();
        let pipeline = compile(user_pipeline).unwrap();

        assert_eq!(pipeline.connections.len(), 1);
        let conn = &pipeline.connections[0];
        assert_eq!(conn.from_node, "source");
        assert_eq!(conn.from_pin, "alt_out");
        assert_eq!(conn.to_node, "sink");
        assert_eq!(conn.to_pin, "my_input");
    }

    // -----------------------------------------------------------------------
    // Client section tests
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_client_section_parsed_in_steps() {
        let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: transcription
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();

        let client = compiled.client.expect("client section should be present");
        let input = client.input.expect("input config should be present");
        assert!(matches!(input.input_type, InputType::FileUpload));
        assert_eq!(input.accept.as_deref(), Some("audio/*"));

        let output = client.output.expect("output config should be present");
        assert!(matches!(output.output_type, OutputType::Transcription));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_client_section_parsed_in_dag() {
        let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/test
      input_broadcast: camera
      output_broadcast: output
client:
  gateway_path: /moq/test
  publish:
    broadcast: camera
    audio: true
    video: true
  watch:
    broadcast: output
    audio: true
    video: true
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();

        let client = compiled.client.expect("client section should be present");
        assert_eq!(client.gateway_path.as_deref(), Some("/moq/test"));

        let publish = client.publish.expect("publish config should be present");
        assert_eq!(publish.broadcast, "camera");
        assert!(publish.audio);
        assert!(publish.video);

        let watch = client.watch.expect("watch config should be present");
        assert_eq!(watch.broadcast, "output");
        assert!(watch.audio);
        assert!(watch.video);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_client_section_optional() {
        let yaml = r"
mode: oneshot
steps:
  - kind: core::passthrough
";
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();
        assert!(compiled.client.is_none());
    }

    #[test]
    fn test_invalid_client_section_rejected() {
        let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
client:
  input:
    type: invalid_type
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid client section"),
            "Error should mention client section: {err}"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_client_section_with_field_hints() {
        let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
    field_hints:
      text:
        type: text
        placeholder: "Enter your prompt"
      reference:
        type: file
        accept: "audio/*"
  output:
    type: audio
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();

        let client = compiled.client.expect("client section should be present");
        let input = client.input.expect("input config should be present");
        let hints = input.field_hints.expect("field_hints should be present");

        assert_eq!(hints.len(), 2);

        let text_hint = hints.get("text").expect("text hint should exist");
        assert!(matches!(text_hint.field_type, Some(FieldType::Text)));
        assert_eq!(text_hint.placeholder.as_deref(), Some("Enter your prompt"));

        let ref_hint = hints.get("reference").expect("reference hint should exist");
        assert!(matches!(ref_hint.field_type, Some(FieldType::File)));
        assert_eq!(ref_hint.accept.as_deref(), Some("audio/*"));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_client_section_with_asset_tags() {
        let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
    asset_tags:
      - speech
      - voice
  output:
    type: transcription
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();

        let client = compiled.client.expect("client section should be present");
        let input = client.input.expect("input config should be present");
        let tags = input.asset_tags.expect("asset_tags should be present");
        assert_eq!(tags, vec!["speech", "voice"]);
    }

    // -----------------------------------------------------------------------
    // Client section lint tests
    // -----------------------------------------------------------------------

    /// Helper to build a minimal valid dynamic client section.
    fn dynamic_client() -> ClientSection {
        ClientSection {
            relay_url: None,
            gateway_path: Some("/moq/test".into()),
            publish: Some(PublishConfig {
                broadcast: "input".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            watch: Some(WatchConfig { broadcast: "output".into(), audio: true, video: true }),
            input: None,
            output: None,
        }
    }

    /// Helper to build a minimal valid oneshot client section.
    fn oneshot_client() -> ClientSection {
        ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::FileUpload,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
        }
    }

    #[test]
    fn test_lint_clean_dynamic() {
        let warnings = lint_client_section(&dynamic_client(), EngineMode::Dynamic);
        assert!(warnings.is_empty(), "Expected no warnings: {warnings:?}");
    }

    #[test]
    fn test_lint_clean_oneshot() {
        let warnings = lint_client_section(&oneshot_client(), EngineMode::OneShot);
        assert!(warnings.is_empty(), "Expected no warnings: {warnings:?}");
    }

    #[test]
    fn test_lint_mode_mismatch_dynamic_with_oneshot_fields() {
        let mut c = dynamic_client();
        c.input = Some(InputConfig {
            input_type: InputType::FileUpload,
            accept: None,
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "mode-mismatch-dynamic"));
    }

    #[test]
    fn test_lint_mode_mismatch_oneshot_with_dynamic_fields() {
        let mut c = oneshot_client();
        c.gateway_path = Some("/moq/test".into());
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "mode-mismatch-oneshot"));
    }

    #[test]
    fn test_lint_missing_gateway() {
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: Some(PublishConfig {
                broadcast: "x".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            watch: None,
            input: None,
            output: None,
        };
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "missing-gateway"));
    }

    #[test]
    fn test_lint_publish_no_media() {
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: "x".into(),
            audio: false,
            video: false,
            screen: false,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "publish-no-media"));
    }

    #[test]
    fn test_lint_watch_no_media() {
        let mut c = dynamic_client();
        c.watch = Some(WatchConfig { broadcast: "x".into(), audio: false, video: false });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "watch-no-media"));
    }

    #[test]
    fn test_lint_empty_broadcast() {
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: String::new(),
            audio: true,
            video: false,
            screen: false,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "empty-broadcast"));
    }

    #[test]
    fn test_lint_duplicate_broadcast() {
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: "same".into(),
            audio: true,
            video: false,
            screen: false,
        });
        c.watch = Some(WatchConfig { broadcast: "same".into(), audio: true, video: true });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(warnings.iter().any(|w| w.rule == "duplicate-broadcast"));
    }

    #[test]
    fn test_lint_input_none_with_accept() {
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Video }),
        };
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "input-none-with-accept"));
    }

    #[test]
    fn test_lint_input_trigger_with_accept() {
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::Trigger,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
        };
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "input-trigger-with-accept"));
    }

    #[test]
    fn test_lint_field_hints_no_input() {
        let mut hints = IndexMap::new();
        hints.insert(
            "x".into(),
            FieldHint { field_type: Some(FieldType::File), accept: None, placeholder: None },
        );
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: None,
                asset_tags: None,
                placeholder: None,
                field_hints: Some(hints),
            }),
            output: Some(OutputConfig { output_type: OutputType::Video }),
        };
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "field-hints-no-input"));
    }

    #[test]
    fn test_lint_asset_tags_text_input() {
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::Text,
                accept: None,
                asset_tags: Some(vec!["speech".into()]),
                placeholder: Some("Enter text".into()),
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
        };
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "asset-tags-no-input"));
    }

    #[test]
    fn test_lint_text_no_placeholder() {
        let c = ClientSection {
            relay_url: None,
            gateway_path: None,
            publish: None,
            watch: None,
            input: Some(InputConfig {
                input_type: InputType::Text,
                accept: None,
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
        };
        let warnings = lint_client_section(&c, EngineMode::OneShot);
        assert!(warnings.iter().any(|w| w.rule == "text-no-placeholder"));
    }

    // -----------------------------------------------------------------------
    // Client-vs-nodes cross-validation tests (rules 13–20)
    // -----------------------------------------------------------------------

    /// Helper: a `streamkit::http_input` node with no params.
    fn http_input_node() -> serde_json::Value {
        serde_json::Value::Null // represents "no params object"
    }

    fn node<'a>(kind: &'a str, params: Option<&'a serde_json::Value>) -> NodeInfo<'a> {
        NodeInfo { kind, params }
    }

    // Rule 13 — input-requires-http-input
    #[test]
    fn test_lint_input_requires_http_input() {
        let c = oneshot_client(); // input.type = file_upload
        let nodes: Vec<NodeInfo<'_>> = vec![]; // no http_input
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(warnings.iter().any(|w| w.rule == "input-requires-http-input"));
    }

    #[test]
    fn test_lint_input_requires_http_input_clean() {
        let c = oneshot_client();
        let null = http_input_node();
        let nodes = vec![node("streamkit::http_input", Some(&null))];
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "input-requires-http-input"),
            "Should not warn when http_input exists: {warnings:?}"
        );
    }

    // Rule 14 — input-none-has-http-input
    #[test]
    fn test_lint_input_none_has_http_input() {
        let c = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: None,
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Video }),
            ..Default::default()
        };
        let null = http_input_node();
        let nodes = vec![node("streamkit::http_input", Some(&null))];
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(warnings.iter().any(|w| w.rule == "input-none-has-http-input"));
    }

    // Rule 15 — field-hint-unknown-field
    #[test]
    fn test_lint_field_hint_unknown_field() {
        let mut hints = IndexMap::new();
        hints.insert(
            "nonexistent".into(),
            FieldHint { field_type: Some(FieldType::Text), accept: None, placeholder: None },
        );
        let c = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::FileUpload,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: Some(hints),
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
            ..Default::default()
        };
        // http_input with no explicit field/fields → default field is "media"
        let null = http_input_node();
        let nodes = vec![node("streamkit::http_input", Some(&null))];
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(
            warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
            "Should warn for unknown field hint name: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_field_hint_known_field_clean() {
        let mut hints = IndexMap::new();
        hints.insert(
            "media".into(),
            FieldHint {
                field_type: Some(FieldType::File),
                accept: Some("audio/*".into()),
                placeholder: None,
            },
        );
        let c = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::FileUpload,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: Some(hints),
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
            ..Default::default()
        };
        let null = http_input_node();
        let nodes = vec![node("streamkit::http_input", Some(&null))];
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
            "Should not warn for default 'media' field: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_field_hint_explicit_fields_array() {
        let mut hints = IndexMap::new();
        hints.insert(
            "prompt".into(),
            FieldHint {
                field_type: Some(FieldType::Text),
                accept: None,
                placeholder: Some("Enter text".into()),
            },
        );
        let c = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::FileUpload,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: Some(hints),
            }),
            output: Some(OutputConfig { output_type: OutputType::Audio }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "fields": [
                { "name": "media" },
                { "name": "prompt" }
            ]
        });
        let nodes = vec![node("streamkit::http_input", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
            "Should not warn when hint matches declared field: {warnings:?}"
        );
    }

    // Rule 16 — publish-no-transport
    #[test]
    fn test_lint_publish_no_transport() {
        let c = dynamic_client();
        let nodes: Vec<NodeInfo<'_>> = vec![]; // no MoQ nodes
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(warnings.iter().any(|w| w.rule == "publish-no-transport"));
    }

    #[test]
    fn test_lint_publish_with_peer_clean() {
        let c = dynamic_client();
        let params = serde_json::json!({
            "gateway_path": "/moq/test",
            "input_broadcast": "input",
            "output_broadcast": "output"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "publish-no-transport"),
            "Should not warn when peer exists: {warnings:?}"
        );
    }

    // Rule 17 — watch-no-transport
    #[test]
    fn test_lint_watch_no_transport() {
        let c = ClientSection {
            gateway_path: Some("/moq/test".into()),
            watch: Some(WatchConfig { broadcast: "output".into(), audio: true, video: true }),
            ..Default::default()
        };
        let nodes: Vec<NodeInfo<'_>> = vec![]; // no MoQ nodes
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(warnings.iter().any(|w| w.rule == "watch-no-transport"));
    }

    // Rule 18 — gateway-path-mismatch
    #[test]
    fn test_lint_gateway_path_mismatch() {
        let c = ClientSection {
            gateway_path: Some("/moq/wrong".into()),
            publish: Some(PublishConfig {
                broadcast: "input".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "gateway_path": "/moq/correct",
            "input_broadcast": "input"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            warnings.iter().any(|w| w.rule == "gateway-path-mismatch"),
            "Should warn when gateway_path differs: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_gateway_path_match_clean() {
        let c = dynamic_client(); // gateway_path = /moq/test
        let params = serde_json::json!({
            "gateway_path": "/moq/test",
            "input_broadcast": "input",
            "output_broadcast": "output"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "gateway-path-mismatch"),
            "Should not warn when gateway_path matches: {warnings:?}"
        );
    }

    // Rule 19 — relay-url-mismatch
    #[test]
    fn test_lint_relay_url_mismatch() {
        let c = ClientSection {
            relay_url: Some("https://relay.example.com".into()),
            publish: Some(PublishConfig {
                broadcast: "input".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "url": "https://other-relay.example.com",
            "broadcast": "input"
        });
        let nodes = vec![node("transport::moq::publisher", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            warnings.iter().any(|w| w.rule == "relay-url-mismatch"),
            "Should warn when relay_url differs: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_relay_url_match_clean() {
        let c = ClientSection {
            relay_url: Some("https://relay.example.com".into()),
            publish: Some(PublishConfig {
                broadcast: "input".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "url": "https://relay.example.com",
            "broadcast": "input"
        });
        let nodes = vec![node("transport::moq::subscriber", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "relay-url-mismatch"),
            "Should not warn when relay_url matches: {warnings:?}"
        );
    }

    // Rule 20 — broadcast-mismatch
    #[test]
    fn test_lint_broadcast_mismatch_publish() {
        let c = ClientSection {
            gateway_path: Some("/moq/test".into()),
            publish: Some(PublishConfig {
                broadcast: "wrong_name".into(),
                audio: true,
                video: false,
                screen: false,
            }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "gateway_path": "/moq/test",
            "input_broadcast": "camera",
            "output_broadcast": "output"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
            "Should warn when publish.broadcast doesn't match any node broadcast: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_broadcast_mismatch_watch() {
        let c = ClientSection {
            gateway_path: Some("/moq/test".into()),
            watch: Some(WatchConfig { broadcast: "wrong_name".into(), audio: true, video: true }),
            ..Default::default()
        };
        let params = serde_json::json!({
            "gateway_path": "/moq/test",
            "input_broadcast": "camera",
            "output_broadcast": "output"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
            "Should warn when watch.broadcast doesn't match any node broadcast: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_broadcast_match_clean() {
        let c = dynamic_client(); // publish=input, watch=output
        let params = serde_json::json!({
            "gateway_path": "/moq/test",
            "input_broadcast": "input",
            "output_broadcast": "output"
        });
        let nodes = vec![node("transport::moq::peer", Some(&params))];
        let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
        assert!(
            !warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
            "Should not warn when broadcast names match: {warnings:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Screen capture boolean tests
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_screen_defaults_to_false() {
        let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    audio: true
    video: true
  watch:
    broadcast: output
    audio: true
    video: true
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();
        let client = compiled.client.expect("client section should be present");
        let publish = client.publish.expect("publish config should be present");
        assert!(!publish.screen);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_screen_true_parsed() {
        let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    audio: true
    video: true
    screen: true
  watch:
    broadcast: output
    audio: true
    video: true
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();
        let client = compiled.client.expect("client section should be present");
        let publish = client.publish.expect("publish config should be present");
        assert!(publish.screen);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_screen_false_explicit() {
        let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    audio: true
    video: true
    screen: false
  watch:
    broadcast: output
    audio: true
    video: true
"#;
        let pipeline = parse_yaml(yaml).unwrap();
        let compiled = compile(pipeline).unwrap();
        let client = compiled.client.expect("client section should be present");
        let publish = client.publish.expect("publish config should be present");
        assert!(!publish.screen);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn test_screen_roundtrip() {
        // Verify serde round-trip: serialize → deserialize preserves the value.
        let config =
            PublishConfig { broadcast: "test".into(), audio: true, video: true, screen: true };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"screen\":true"));

        let deserialized: PublishConfig = serde_json::from_str(&json).unwrap();
        assert!(deserialized.screen);
    }

    #[test]
    fn test_lint_screen_source_no_video() {
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: "input".into(),
            audio: true,
            video: false,
            screen: true,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(
            warnings.iter().any(|w| w.rule == "screen-source-no-video"),
            "Should warn when screen is true but video is false: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_screen_source_with_video_clean() {
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: "input".into(),
            audio: true,
            video: true,
            screen: true,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(
            !warnings.iter().any(|w| w.rule == "screen-source-no-video"),
            "Should not warn when screen is true and video is true: {warnings:?}"
        );
    }

    #[test]
    fn test_lint_camera_source_no_video_no_warning() {
        // screen: false with video: false should NOT trigger screen-source-no-video
        let mut c = dynamic_client();
        c.publish = Some(PublishConfig {
            broadcast: "input".into(),
            audio: true,
            video: false,
            screen: false,
        });
        let warnings = lint_client_section(&c, EngineMode::Dynamic);
        assert!(
            !warnings.iter().any(|w| w.rule == "screen-source-no-video"),
            "Should not warn for camera source without video: {warnings:?}"
        );
    }
}
