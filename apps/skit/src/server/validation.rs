// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::file_security;
use crate::state::AppState;
use streamkit_api::yaml::compile;
use streamkit_api::Pipeline;

use super::{read_registry, AppError};

// ---------------------------------------------------------------------------
// POST /api/v1/validate — stateless pipeline dry-run
// ---------------------------------------------------------------------------

/// A single node in the validated graph.
#[derive(Serialize)]
pub struct ValidateGraphNode {
    id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// A single connection in the validated graph.
#[derive(Serialize)]
pub struct ValidateGraphConnection {
    from_node: String,
    from_pin: String,
    to_node: String,
    to_pin: String,
}

/// The parsed graph structure — always returned so the UI can highlight nodes.
#[derive(Serialize)]
pub struct ValidateGraph {
    nodes: Vec<ValidateGraphNode>,
    connections: Vec<ValidateGraphConnection>,
}

/// Diagnostic category.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    Parse,
    Schema,
    Connection,
    Permission,
    Security,
}

/// A single validation diagnostic.
#[derive(Debug, Serialize)]
pub struct ValidateDiagnostic {
    kind: DiagnosticKind,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_id: Option<String>,
}

/// Top-level response for `POST /api/v1/validate` and the MCP
/// `validate_pipeline` tool.
#[derive(Serialize)]
pub struct ValidateResponse {
    pub valid: bool,
    errors: Vec<ValidateDiagnostic>,
    warnings: Vec<ValidateDiagnostic>,
    graph: Option<ValidateGraph>,
}

/// Pipeline mode for validation — determines which synthetic-node rules apply.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PipelineMode {
    Dynamic,
    Oneshot,
}

/// Request body for `POST /api/v1/validate`.
#[derive(Deserialize)]
pub(super) struct ValidateRequest {
    yaml: String,
    /// Optional pipeline mode.
    /// When `Dynamic`, synthetic nodes (`streamkit::http_input`/`http_output`)
    /// are rejected — matching `create_session_handler` behaviour.
    #[serde(default)]
    mode: Option<PipelineMode>,
}

/// Synthetic oneshot-only node kinds, derived from `synthetic_node_definitions()`
/// to prevent drift.  `LazyLock` avoids rebuilding the list on every call.
static SYNTHETIC_KINDS: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| synthetic_node_definitions().into_iter().map(|d| d.kind).collect());

/// Returns `true` for node kinds that are synthetic oneshot-only markers.
///
/// Used by both the HTTP and MCP `create_session` paths to reject
/// oneshot-only nodes in dynamic pipelines.
pub fn is_synthetic_kind(kind: &str) -> bool {
    SYNTHETIC_KINDS.iter().any(|k| k == kind)
}

/// Build synthetic `NodeDefinition`s for oneshot-only virtual nodes that are not
/// registered in the `NodeRegistry` (`streamkit::http_input`, `streamkit::http_output`).
///
/// Used by both `list_node_definitions_handler` and the validate endpoint so
/// there is a single source of truth for these definitions.
pub fn synthetic_node_definitions() -> Vec<streamkit_core::NodeDefinition> {
    use streamkit_core::types::PacketType;
    use streamkit_core::{InputPin, NodeDefinition, OutputPin, PinCardinality};

    vec![
        NodeDefinition {
            kind: "streamkit::http_input".to_string(),
            description: Some(
                "Synthetic input node for oneshot HTTP pipelines. \
                 Receives binary data from the HTTP request body."
                    .to_string(),
            ),
            param_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "field": {
                        "type": "string",
                        "description": "Multipart field name to bind to this input. Defaults to 'media' when only one http_input node exists; otherwise defaults to the node id."
                    },
                    "fields": {
                        "type": "array",
                        "description": "Optional list of multipart fields for this node. When set, the node exposes one output pin per entry (pin name matches the field name). Entries may be strings or objects with { name, required }.",
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": {
                                        "name": { "type": "string" },
                                        "required": { "type": "boolean", "default": true }
                                    },
                                    "required": ["name"]
                                }
                            ]
                        }
                    },
                    "required": {
                        "type": "boolean",
                        "description": "If true (default), the request must include this field.",
                        "default": true
                    }
                }
            }),
            inputs: vec![],
            outputs: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Binary,
                cardinality: PinCardinality::Broadcast,
            }],
            categories: vec!["transport".to_string(), "oneshot".to_string()],
            bidirectional: false,
        },
        NodeDefinition {
            kind: "streamkit::http_output".to_string(),
            description: Some(
                "Synthetic output node for oneshot HTTP pipelines. \
                 Sends binary data as the HTTP response body."
                    .to_string(),
            ),
            param_schema: serde_json::json!({}),
            inputs: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Binary],
                cardinality: PinCardinality::One,
            }],
            outputs: vec![],
            categories: vec!["transport".to_string(), "oneshot".to_string()],
            bidirectional: false,
        },
    ]
}

/// Validate node kinds and params against the registry, returning resolved definitions.
///
/// When `perms` is `Some`, per-node permission filtering is applied (matching
/// `list_node_definitions_handler` and `create_session_handler`).  In unit tests
/// `None` is passed to skip permission checks.
pub(super) fn validate_nodes(
    pipeline: &Pipeline,
    registry: &streamkit_core::NodeRegistry,
    perms: Option<&crate::permissions::Permissions>,
    errors: &mut Vec<ValidateDiagnostic>,
    warnings: &mut Vec<ValidateDiagnostic>,
) -> HashMap<String, streamkit_core::NodeDefinition> {
    let mut node_defs: HashMap<String, streamkit_core::NodeDefinition> = HashMap::new();
    let synthetics: HashMap<String, streamkit_core::NodeDefinition> =
        synthetic_node_definitions().into_iter().map(|d| (d.kind.clone(), d)).collect();

    for (node_id, node) in &pipeline.nodes {
        debug!(node_id = %node_id, kind = %node.kind, "Validating node kind");

        let def =
            registry.get_definition(&node.kind).or_else(|| synthetics.get(&node.kind).cloned());

        let Some(def) = def else {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Schema,
                message: format!("Unknown node kind '{}'", node.kind),
                node_id: Some(node_id.clone()),
                connection_id: None,
            });
            continue;
        };

        // Synthetic oneshot nodes bypass per-node permission checks,
        // matching the oneshot handler which never filters them.
        if let Some(perms) = perms.filter(|_| !is_synthetic_kind(&node.kind)) {
            if !perms.is_node_allowed(&node.kind) {
                errors.push(ValidateDiagnostic {
                    kind: DiagnosticKind::Permission,
                    message: format!("Permission denied: node kind '{}' not allowed", node.kind),
                    node_id: Some(node_id.clone()),
                    connection_id: None,
                });
                continue;
            }
            if node.kind.starts_with("plugin::") && !perms.is_plugin_allowed(&node.kind) {
                errors.push(ValidateDiagnostic {
                    kind: DiagnosticKind::Permission,
                    message: format!("Permission denied: plugin '{}' not allowed", node.kind),
                    node_id: Some(node_id.clone()),
                    connection_id: None,
                });
                continue;
            }
        }

        // Param schema validation (best-effort, report as warnings).
        if let Some(schema_obj) = def.param_schema.as_object() {
            if !schema_obj.is_empty() {
                if let Some(schema_props) =
                    def.param_schema.get("properties").and_then(|v| v.as_object())
                {
                    let params_obj = node.params.as_ref().and_then(|p| p.as_object());

                    // Warn on unknown parameters.
                    if let Some(params_obj) = params_obj {
                        for key in params_obj.keys() {
                            if !schema_props.contains_key(key) {
                                warnings.push(ValidateDiagnostic {
                                    kind: DiagnosticKind::Schema,
                                    message: format!(
                                        "Unknown parameter '{key}' for node kind '{}'",
                                        def.kind
                                    ),
                                    node_id: Some(node_id.clone()),
                                    connection_id: None,
                                });
                            }
                        }
                    }

                    // Warn on missing required parameters.
                    if let Some(required) =
                        def.param_schema.get("required").and_then(|v| v.as_array())
                    {
                        for req in required {
                            if let Some(req_name) = req.as_str() {
                                let is_present =
                                    params_obj.is_some_and(|p| p.contains_key(req_name));
                                if !is_present {
                                    warnings.push(ValidateDiagnostic {
                                        kind: DiagnosticKind::Schema,
                                        message: format!(
                                            "Missing required parameter '{req_name}' for node kind '{}'",
                                            def.kind
                                        ),
                                        node_id: Some(node_id.clone()),
                                        connection_id: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        node_defs.insert(node_id.clone(), def);
    }

    node_defs
}

/// Validate all connections and collect diagnostics for missing pins and type mismatches.
pub(super) fn validate_connections(
    pipeline: &Pipeline,
    node_defs: &HashMap<String, streamkit_core::NodeDefinition>,
    errors: &mut Vec<ValidateDiagnostic>,
) {
    let packet_type_registry = streamkit_core::packet_meta::packet_type_registry();

    for conn in &pipeline.connections {
        let conn_id = format!("{}->{}", conn.from_node, conn.to_node);

        // Check that referenced nodes exist in the pipeline definition.
        if !pipeline.nodes.contains_key(&conn.from_node) {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Connection,
                message: format!("Source node '{}' does not exist", conn.from_node),
                node_id: None,
                connection_id: Some(conn_id.clone()),
            });
        }
        if !pipeline.nodes.contains_key(&conn.to_node) {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Connection,
                message: format!("Destination node '{}' does not exist", conn.to_node),
                node_id: None,
                connection_id: Some(conn_id.clone()),
            });
        }

        let Some(src_def) = node_defs.get(&conn.from_node) else { continue };
        let Some(dst_def) = node_defs.get(&conn.to_node) else { continue };

        let src_pin = find_output_pin(&src_def.outputs, &conn.from_pin);
        let dst_pin = find_input_pin(&dst_def.inputs, &conn.to_pin);

        if src_pin.is_none() {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Connection,
                message: format!(
                    "Output pin '{}' not found on node '{}' (kind '{}')",
                    conn.from_pin, conn.from_node, src_def.kind
                ),
                node_id: Some(conn.from_node.clone()),
                connection_id: Some(conn_id.clone()),
            });
        }
        if dst_pin.is_none() {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Connection,
                message: format!(
                    "Input pin '{}' not found on node '{}' (kind '{}')",
                    conn.to_pin, conn.to_node, dst_def.kind
                ),
                node_id: Some(conn.to_node.clone()),
                connection_id: Some(conn_id.clone()),
            });
        }

        if let (Some(src), Some(dst)) = (src_pin, dst_pin) {
            validate_pin_types(src, dst, conn, &conn_id, packet_type_registry, errors);
        }
    }
}

/// Find an output pin by exact name or dynamic-prefix match.
///
/// Uses `PinCardinality::is_dynamic_pin_match` from `streamkit_core` — the
/// same matching logic the dynamic engine applies at runtime.
///
/// Exact-name matches are preferred: if a static pin with name == `name` exists
/// it wins even when a dynamic-prefix pin could also match.
fn find_output_pin<'a>(
    pins: &'a [streamkit_core::OutputPin],
    name: &str,
) -> Option<&'a streamkit_core::OutputPin> {
    pins.iter().find(|p| p.name == name).or_else(|| {
        pins.iter().find(|p| {
            matches!(
                &p.cardinality,
                streamkit_core::PinCardinality::Dynamic { prefix }
                    if streamkit_core::PinCardinality::is_dynamic_pin_match(prefix, name)
            )
        })
    })
}

/// Find an input pin by exact name or dynamic-prefix match.
///
/// Uses `PinCardinality::is_dynamic_pin_match` from `streamkit_core` — the
/// same matching logic the dynamic engine applies at runtime.
///
/// Exact-name matches are preferred: if a static pin with name == `name` exists
/// it wins even when a dynamic-prefix pin could also match.
fn find_input_pin<'a>(
    pins: &'a [streamkit_core::InputPin],
    name: &str,
) -> Option<&'a streamkit_core::InputPin> {
    pins.iter().find(|p| p.name == name).or_else(|| {
        pins.iter().find(|p| {
            matches!(
                &p.cardinality,
                streamkit_core::PinCardinality::Dynamic { prefix }
                    if streamkit_core::PinCardinality::is_dynamic_pin_match(prefix, name)
            )
        })
    })
}

/// Reject synthetic nodes when the requested mode is `Dynamic`.
pub(super) fn check_mode(
    pipeline: &Pipeline,
    mode: Option<PipelineMode>,
    errors: &mut Vec<ValidateDiagnostic>,
) {
    if mode != Some(PipelineMode::Dynamic) {
        return;
    }
    for (node_id, node) in &pipeline.nodes {
        if is_synthetic_kind(&node.kind) {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Schema,
                message: format!("Node kind '{}' is only valid in oneshot pipelines", node.kind),
                node_id: Some(node_id.clone()),
                connection_id: None,
            });
        }
    }
}

/// Check type compatibility between a source output pin and destination input pin.
pub(super) fn validate_pin_types(
    src: &streamkit_core::OutputPin,
    dst: &streamkit_core::InputPin,
    conn: &streamkit_api::Connection,
    conn_id: &str,
    pt_registry: &[streamkit_core::packet_meta::PacketTypeMeta],
    errors: &mut Vec<ValidateDiagnostic>,
) {
    if matches!(src.produces_type, streamkit_core::types::PacketType::Passthrough) {
        return;
    }
    if dst.accepts_types.iter().any(|t| matches!(t, streamkit_core::types::PacketType::Passthrough))
    {
        return;
    }
    if !streamkit_core::packet_meta::can_connect_any(
        &src.produces_type,
        &dst.accepts_types,
        pt_registry,
    ) {
        errors.push(ValidateDiagnostic {
            kind: DiagnosticKind::Connection,
            message: format!(
                "Type mismatch: '{}' output pin '{}' produces {:?}, \
                 but '{}' input pin '{}' accepts {:?}",
                conn.from_node,
                conn.from_pin,
                src.produces_type,
                conn.to_node,
                conn.to_pin,
                dst.accepts_types
            ),
            node_id: None,
            connection_id: Some(conn_id.to_string()),
        });
    }
}

/// Axum handler for stateless pipeline validation.
///
/// Parses the supplied YAML, compiles it into an internal `Pipeline`, and
/// validates every node kind, pin existence, pin-type compatibility, and
/// file-path security — all without instantiating any nodes.
pub(super) async fn validate_pipeline_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ValidateRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    if !perms.create_sessions {
        return Err((StatusCode::FORBIDDEN, "Permission denied: create_sessions required".into()));
    }

    let response = validate_pipeline_yaml(&app_state, &perms, &payload.yaml, payload.mode)
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;

    debug!(
        valid = response.valid,
        error_count = response.errors.len(),
        warning_count = response.warnings.len(),
        "Pipeline validation completed"
    );

    Ok(Json(response))
}

/// Extract a human-readable message from an `AppError`.
///
/// `AppError` does not implement `Display` (only `IntoResponse`), so we
/// pattern-match to pull out the inner message/error.
fn app_error_message(err: AppError) -> String {
    match err {
        AppError::BadRequest(msg)
        | AppError::PipelineCompilation(msg)
        | AppError::Forbidden(msg) => msg,
        AppError::Engine(e) => format!("{e}"),
        AppError::Multipart(e) => format!("{e}"),
        AppError::Serde(e) => format!("{e}"),
    }
}

/// Run file-path security checks by delegating to the existing
/// `validate_file_reader_paths` / `validate_file_writer_paths` / `validate_script_paths`
/// helpers.  This keeps a single implementation so that new checks in those
/// helpers automatically apply to the validate endpoint.
fn collect_file_path_errors(
    pipeline: &Pipeline,
    security_config: &crate::config::SecurityConfig,
    errors: &mut Vec<ValidateDiagnostic>,
) {
    for result in [
        validate_file_reader_paths(pipeline, security_config),
        validate_file_writer_paths(pipeline, security_config),
        validate_script_paths(pipeline, security_config),
    ] {
        if let Err(e) = result {
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Security,
                message: app_error_message(e),
                node_id: None,
                connection_id: None,
            });
        }
    }
}
/// Validate that the pipeline has the required nodes for oneshot processing.
/// Returns (has_http_input, has_file_read, has_http_output) for logging purposes.
///
/// Pipelines must have `streamkit::http_output`. For input, they must have at least one of:
/// - `streamkit::http_input` (HTTP streaming mode)
/// - `core::file_reader` (file-based mode)
/// - Neither (generator mode — the pipeline produces its own data, e.g. video::colorbars)
pub(super) fn validate_pipeline_nodes(
    pipeline_def: &Pipeline,
) -> Result<(bool, bool, bool), AppError> {
    let has_http_input =
        pipeline_def.nodes.values().any(|node| node.kind == "streamkit::http_input");
    let has_http_output =
        pipeline_def.nodes.values().any(|node| node.kind == "streamkit::http_output");
    let has_file_read = pipeline_def.nodes.values().any(|node| node.kind == "core::file_reader");

    if !has_http_output {
        return Err(AppError::BadRequest(
            "Pipeline must contain one 'streamkit::http_output' node for oneshot processing"
                .to_string(),
        ));
    }

    // Generator mode: no http_input or file_reader, but there must be at
    // least one other node that can produce data.
    if !has_http_input && !has_file_read {
        let non_output_count =
            pipeline_def.nodes.values().filter(|n| n.kind != "streamkit::http_output").count();
        if non_output_count == 0 {
            return Err(AppError::BadRequest(
                "Generator-mode pipeline must contain at least one node besides 'streamkit::http_output'"
                    .to_string(),
            ));
        }
    }

    Ok((has_http_input, has_file_read, has_http_output))
}

/// Validate file paths in all file_reader nodes to prevent path traversal attacks.
pub(super) fn validate_file_reader_paths(
    pipeline_def: &Pipeline,
    security_config: &crate::config::SecurityConfig,
) -> Result<(), AppError> {
    for (node_id, node_def) in &pipeline_def.nodes {
        if node_def.kind == "core::file_reader" {
            if let Some(params) = &node_def.params {
                if let Some(path_value) = params.get("path") {
                    if let Some(path_str) = path_value.as_str() {
                        file_security::validate_file_path(path_str, security_config).map_err(
                            |e| {
                                AppError::BadRequest(format!(
                                    "Invalid file path in node '{node_id}': {e}"
                                ))
                            },
                        )?;
                    }
                }
            }
        }
    }
    tracing::info!("File path validation passed");
    Ok(())
}

/// Validate write paths in all file_writer nodes to prevent arbitrary file writes.
pub(super) fn validate_file_writer_paths(
    pipeline_def: &Pipeline,
    security_config: &crate::config::SecurityConfig,
) -> Result<(), AppError> {
    for (node_id, node_def) in &pipeline_def.nodes {
        if node_def.kind == "core::file_writer" {
            let Some(params) = &node_def.params else {
                return Err(AppError::BadRequest(format!(
                    "Invalid file_writer params in node '{node_id}': expected params.path"
                )));
            };

            let Some(path_str) = params.get("path").and_then(serde_json::Value::as_str) else {
                return Err(AppError::BadRequest(format!(
                    "Invalid file_writer params in node '{node_id}': expected params.path to be a string"
                )));
            };

            crate::file_security::validate_write_path(path_str, security_config).map_err(|e| {
                AppError::BadRequest(format!("Invalid write path in node '{node_id}': {e}"))
            })?;
        }
    }
    Ok(())
}

/// Validate script file paths in all core::script nodes to prevent path traversal attacks.
pub(super) fn validate_script_paths(
    pipeline_def: &Pipeline,
    security_config: &crate::config::SecurityConfig,
) -> Result<(), AppError> {
    for (node_id, node_def) in &pipeline_def.nodes {
        if node_def.kind == "core::script" {
            if let Some(params) = &node_def.params {
                if let Some(path_value) = params.get("script_path") {
                    if let Some(path_str) = path_value.as_str() {
                        if !path_str.trim().is_empty() {
                            crate::file_security::validate_file_path(path_str, security_config)
                                .map_err(|e| {
                                    AppError::BadRequest(format!(
                                        "Invalid script_path in node '{node_id}': {e}"
                                    ))
                                })?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Load secrets from environment variables based on server configuration.
///
/// Returns a HashMap mapping secret names to their values loaded from the environment.
/// Secrets that are configured but not found in the environment are logged as warnings.
#[cfg(feature = "script")]
pub(super) fn load_script_secrets(
    secret_configs: &std::collections::HashMap<String, crate::config::SecretConfig>,
) -> std::collections::HashMap<String, streamkit_nodes::core::script::ScriptSecret> {
    let mut secrets = std::collections::HashMap::new();

    for (name, config) in secret_configs {
        match std::env::var(&config.env) {
            Ok(value) => {
                info!(
                    secret_name = %name,
                    env_var = %config.env,
                    "Loaded secret from environment variable"
                );
                secrets.insert(
                    name.clone(),
                    streamkit_nodes::core::script::ScriptSecret {
                        value,
                        allowed_fetch_urls: config.allowed_fetch_urls.clone(),
                    },
                );
            },
            Err(_) => {
                warn!(
                    secret_name = %name,
                    env_var = %config.env,
                    "Secret configured but environment variable not found"
                );
            },
        }
    }

    if secrets.is_empty() && !secret_configs.is_empty() {
        warn!("No secrets loaded from environment (all environment variables missing)");
    } else if !secrets.is_empty() {
        info!(count = secrets.len(), "Successfully loaded secrets from environment");
    }

    secrets
}

// ---------------------------------------------------------------------------
// Shared helpers — used by both HTTP handlers and crate::mcp
// ---------------------------------------------------------------------------

/// Validate a pipeline YAML string with optional mode.
///
/// Shared implementation behind `POST /api/v1/validate` and the MCP
/// `validate_pipeline` tool.
///
/// # Errors
///
/// Returns an error string only if the node registry lock is poisoned.
pub fn validate_pipeline_yaml(
    app_state: &Arc<AppState>,
    perms: &crate::permissions::Permissions,
    yaml: &str,
    mode: Option<PipelineMode>,
) -> Result<ValidateResponse, String> {
    let mut errors: Vec<ValidateDiagnostic> = Vec::new();
    let mut warnings: Vec<ValidateDiagnostic> = Vec::new();

    let user_pipeline = match streamkit_api::yaml::parse_yaml(yaml) {
        Ok(p) => p,
        Err(e) => {
            debug!(error = %e, "Pipeline YAML parse error");
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Parse,
                message: e,
                node_id: None,
                connection_id: None,
            });
            return Ok(ValidateResponse { valid: false, errors, warnings, graph: None });
        },
    };

    let pipeline = match compile(user_pipeline) {
        Ok(p) => p,
        Err(e) => {
            debug!(error = %e, "Pipeline compilation error");
            errors.push(ValidateDiagnostic {
                kind: DiagnosticKind::Parse,
                message: e,
                node_id: None,
                connection_id: None,
            });
            return Ok(ValidateResponse { valid: false, errors, warnings, graph: None });
        },
    };

    if pipeline.nodes.is_empty() {
        errors.push(ValidateDiagnostic {
            kind: DiagnosticKind::Schema,
            message: "Pipeline is empty. Add some nodes before validating.".into(),
            node_id: None,
            connection_id: None,
        });
        return Ok(ValidateResponse { valid: false, errors, warnings, graph: None });
    }

    let registry_guard =
        read_registry(app_state).map_err(|_| "Failed to read node registry".to_string())?;
    let node_defs =
        validate_nodes(&pipeline, &registry_guard, Some(perms), &mut errors, &mut warnings);
    drop(registry_guard);

    check_mode(&pipeline, mode, &mut errors);
    validate_connections(&pipeline, &node_defs, &mut errors);
    collect_file_path_errors(&pipeline, &app_state.config.security, &mut errors);

    let graph = Some(ValidateGraph {
        nodes: pipeline
            .nodes
            .iter()
            .map(|(id, n)| ValidateGraphNode {
                id: id.clone(),
                kind: n.kind.clone(),
                params: n.params.clone(),
            })
            .collect(),
        connections: pipeline
            .connections
            .iter()
            .map(|c| ValidateGraphConnection {
                from_node: c.from_node.clone(),
                from_pin: c.from_pin.clone(),
                to_node: c.to_node.clone(),
                to_pin: c.to_pin.clone(),
            })
            .collect(),
    });

    let valid = errors.is_empty();
    Ok(ValidateResponse { valid, errors, warnings, graph })
}

/// Run all file-path security checks against a pipeline.
///
/// # Errors
///
/// Returns a human-readable error message if any path violates the security
/// policy.
pub fn check_file_path_security(
    pipeline: &Pipeline,
    security_config: &crate::config::SecurityConfig,
) -> Result<(), String> {
    let mut msgs = Vec::new();
    for result in [
        validate_file_reader_paths(pipeline, security_config),
        validate_file_writer_paths(pipeline, security_config),
        validate_script_paths(pipeline, security_config),
    ] {
        if let Err(e) = result {
            msgs.push(app_error_message(e));
        }
    }
    if msgs.is_empty() {
        Ok(())
    } else {
        Err(msgs.join("; "))
    }
}

#[cfg(test)]
// test fixtures use expect for explicit setup failures
#[allow(clippy::expect_used)]
mod validate_pipeline_tests {
    use super::*;

    fn make_registry() -> streamkit_core::NodeRegistry {
        streamkit_core::NodeRegistry::new()
    }

    fn minimal_pipeline(yaml: &str) -> Result<Pipeline, String> {
        let user = streamkit_api::yaml::parse_yaml(yaml)?;
        compile(user)
    }

    fn make_restricted_perms() -> crate::permissions::Permissions {
        crate::permissions::Permissions {
            list_nodes: true,
            create_sessions: true,
            ..Default::default()
        }
    }

    #[test]
    fn synthetic_http_nodes_are_recognised() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let registry = make_registry();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);

        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
        assert!(node_defs.contains_key("input"), "expected input in node_defs");
        assert!(node_defs.contains_key("output"), "expected output in node_defs");
    }

    #[test]
    fn unknown_node_kind_reported() {
        let yaml = "\
nodes:
  bad:
    kind: audio::nonexistent_node
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let registry = make_registry();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Unknown node kind"));
        assert!(errors[0].message.contains("audio::nonexistent_node"));
    }

    #[test]
    fn connection_validation_catches_bad_pins() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let mut pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let registry = make_registry();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);

        pipeline.connections.push(streamkit_api::Connection {
            from_node: "input".to_string(),
            from_pin: "nonexistent_pin".to_string(),
            to_node: "output".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::default(),
        });

        validate_connections(&pipeline, &node_defs, &mut errors);

        assert!(
            errors.iter().any(|e| e.message.contains("not found")),
            "expected pin-not-found error, got: {errors:?}"
        );
    }

    #[test]
    fn valid_oneshot_pipeline_no_errors() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let registry = make_registry();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);
        validate_connections(&pipeline, &node_defs, &mut errors);

        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
        assert_eq!(pipeline.connections.len(), 1);
    }

    #[test]
    fn synthetic_nodes_bypass_permission_checks() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let registry = make_registry();
        let perms = make_restricted_perms();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let node_defs =
            validate_nodes(&pipeline, &registry, Some(&perms), &mut errors, &mut warnings);

        assert!(errors.is_empty(), "synthetic nodes should bypass perms, got: {errors:?}");
        assert_eq!(node_defs.len(), 2);
    }

    #[test]
    fn restricted_perms_deny_non_allowed_node() {
        let yaml = "\
nodes:
  src:
    kind: test::dummy
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut registry = make_registry();
        registry.register_static(
            "test::dummy",
            |_| Err(streamkit_core::StreamKitError::Configuration("test stub".into())),
            serde_json::Value::Object(serde_json::Map::default()),
            streamkit_core::registry::StaticPins { inputs: vec![], outputs: vec![] },
            vec![],
            false,
        );
        let perms = make_restricted_perms();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        validate_nodes(&pipeline, &registry, Some(&perms), &mut errors, &mut warnings);

        assert!(
            errors.iter().any(|e| e.message.contains("Permission denied")),
            "expected permission denied error, got: {errors:?}"
        );
    }

    #[test]
    fn empty_pipeline_rejected() {
        let yaml = "nodes: {}";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        assert!(pipeline.nodes.is_empty());
    }

    /// Register a test node with explicit input/output pin types.
    fn register_typed_node(
        registry: &mut streamkit_core::NodeRegistry,
        kind: &str,
        inputs: Vec<streamkit_core::InputPin>,
        outputs: Vec<streamkit_core::OutputPin>,
    ) {
        registry.register_static(
            kind,
            |_| Err(streamkit_core::StreamKitError::Configuration("test stub".into())),
            serde_json::Value::Object(serde_json::Map::default()),
            streamkit_core::registry::StaticPins { inputs, outputs },
            vec![],
            false,
        );
    }

    #[test]
    fn type_mismatch_reported() {
        use streamkit_core::types::PacketType;
        use streamkit_core::{InputPin, OutputPin, PinCardinality};

        let yaml = "\
nodes:
  src:
    kind: test::text_src
  dst:
    kind: test::audio_dst
    needs: src
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut registry = make_registry();

        register_typed_node(
            &mut registry,
            "test::text_src",
            vec![],
            vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Text,
                cardinality: PinCardinality::Broadcast,
            }],
        );
        register_typed_node(
            &mut registry,
            "test::audio_dst",
            vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Binary],
                cardinality: PinCardinality::One,
            }],
            vec![],
        );

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);
        validate_connections(&pipeline, &node_defs, &mut errors);

        assert!(
            errors.iter().any(|e| e.message.contains("Type mismatch")),
            "expected type mismatch error, got: {errors:?}"
        );
    }

    #[test]
    fn passthrough_source_skips_type_check() {
        use streamkit_core::types::PacketType;
        use streamkit_core::{InputPin, OutputPin, PinCardinality};

        let yaml = "\
nodes:
  src:
    kind: test::passthrough_src
  dst:
    kind: test::audio_dst
    needs: src
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut registry = make_registry();

        register_typed_node(
            &mut registry,
            "test::passthrough_src",
            vec![],
            vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Passthrough,
                cardinality: PinCardinality::Broadcast,
            }],
        );
        register_typed_node(
            &mut registry,
            "test::audio_dst",
            vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Binary],
                cardinality: PinCardinality::One,
            }],
            vec![],
        );

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);
        validate_connections(&pipeline, &node_defs, &mut errors);

        assert!(errors.is_empty(), "passthrough source should skip type check, got: {errors:?}");
    }

    #[test]
    fn passthrough_destination_skips_type_check() {
        use streamkit_core::types::PacketType;
        use streamkit_core::{InputPin, OutputPin, PinCardinality};

        let yaml = "\
nodes:
  src:
    kind: test::text_src
  dst:
    kind: test::passthrough_dst
    needs: src
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut registry = make_registry();

        register_typed_node(
            &mut registry,
            "test::text_src",
            vec![],
            vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::Text,
                cardinality: PinCardinality::Broadcast,
            }],
        );
        register_typed_node(
            &mut registry,
            "test::passthrough_dst",
            vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::Passthrough],
                cardinality: PinCardinality::One,
            }],
            vec![],
        );

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let node_defs = validate_nodes(&pipeline, &registry, None, &mut errors, &mut warnings);
        validate_connections(&pipeline, &node_defs, &mut errors);

        assert!(
            errors.is_empty(),
            "passthrough destination should skip type check, got: {errors:?}"
        );
    }

    #[test]
    fn dynamic_mode_rejects_synthetic_nodes() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut errors: Vec<ValidateDiagnostic> = Vec::new();

        check_mode(&pipeline, Some(PipelineMode::Dynamic), &mut errors);

        assert_eq!(errors.len(), 2, "expected 2 synthetic rejections, got: {errors:?}");
        assert!(errors.iter().all(|e| e.message.contains("only valid in oneshot")));
    }

    #[test]
    fn oneshot_mode_accepts_synthetic_nodes() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut errors: Vec<ValidateDiagnostic> = Vec::new();

        check_mode(&pipeline, Some(PipelineMode::Oneshot), &mut errors);

        assert!(errors.is_empty(), "oneshot mode should accept synthetics, got: {errors:?}");
    }

    #[test]
    fn no_mode_accepts_synthetic_nodes() {
        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let pipeline = minimal_pipeline(yaml).expect("parse should succeed");
        let mut errors: Vec<ValidateDiagnostic> = Vec::new();

        check_mode(&pipeline, None, &mut errors);

        assert!(errors.is_empty(), "no mode should accept synthetics, got: {errors:?}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod helper_tests {
    use super::*;
    use streamkit_api::yaml::parse_yaml;
    use streamkit_core::types::PacketType;
    use streamkit_core::{InputPin, OutputPin, PinCardinality};

    fn pipeline_from(yaml: &str) -> Pipeline {
        compile(parse_yaml(yaml).expect("yaml parse")).expect("compile")
    }

    fn dummy_security_config() -> crate::config::SecurityConfig {
        crate::config::SecurityConfig::default()
    }

    #[test]
    fn is_synthetic_kind_matches_known_synthetics() {
        assert!(is_synthetic_kind("streamkit::http_input"));
        assert!(is_synthetic_kind("streamkit::http_output"));
        assert!(!is_synthetic_kind("audio::opus::encoder"));
        assert!(!is_synthetic_kind(""));
        assert!(!is_synthetic_kind("streamkit::widget"));
    }

    #[test]
    fn synthetic_node_definitions_have_pins() {
        let defs = synthetic_node_definitions();
        assert_eq!(defs.len(), 2);

        let input = defs.iter().find(|d| d.kind == "streamkit::http_input").unwrap();
        assert!(input.inputs.is_empty());
        assert_eq!(input.outputs.len(), 1);

        let output = defs.iter().find(|d| d.kind == "streamkit::http_output").unwrap();
        assert!(output.outputs.is_empty());
        assert_eq!(output.inputs.len(), 1);
    }

    #[test]
    fn validate_pipeline_nodes_requires_http_output() {
        let pipe = pipeline_from("nodes:\n  src:\n    kind: streamkit::http_input\n");
        let err = validate_pipeline_nodes(&pipe).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("streamkit::http_output"), "got: {msg}");
            },
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_pipeline_nodes_generator_with_only_output_is_rejected() {
        let pipe = pipeline_from("nodes:\n  sink:\n    kind: streamkit::http_output\n");
        let err = validate_pipeline_nodes(&pipe).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("Generator-mode"), "got: {msg}");
            },
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_pipeline_nodes_accepts_http_input_plus_output() {
        let pipe = pipeline_from(
            "nodes:\n  input:\n    kind: streamkit::http_input\n  \
             output:\n    kind: streamkit::http_output\n    needs: input\n",
        );
        let (has_http_input, has_file_read, has_http_output) =
            validate_pipeline_nodes(&pipe).unwrap();
        assert!(has_http_input);
        assert!(!has_file_read);
        assert!(has_http_output);
    }

    #[test]
    fn validate_pipeline_nodes_accepts_file_reader_plus_output() {
        let pipe = pipeline_from(
            "nodes:\n  reader:\n    kind: core::file_reader\n    params:\n      path: samples/x.mp4\n  \
             output:\n    kind: streamkit::http_output\n    needs: reader\n",
        );
        let (has_http_input, has_file_read, has_http_output) =
            validate_pipeline_nodes(&pipe).unwrap();
        assert!(!has_http_input);
        assert!(has_file_read);
        assert!(has_http_output);
    }

    #[test]
    fn validate_pipeline_nodes_accepts_generator_with_extra_node() {
        let pipe = pipeline_from(
            "nodes:\n  gen:\n    kind: video::colorbars\n  \
             output:\n    kind: streamkit::http_output\n    needs: gen\n",
        );
        let (has_http_input, has_file_read, has_http_output) =
            validate_pipeline_nodes(&pipe).unwrap();
        assert!(!has_http_input);
        assert!(!has_file_read);
        assert!(has_http_output);
    }

    #[test]
    fn validate_file_reader_paths_ignores_non_file_reader_nodes() {
        let pipe = pipeline_from("nodes:\n  output:\n    kind: streamkit::http_output\n");
        validate_file_reader_paths(&pipe, &dummy_security_config()).unwrap();
    }

    #[test]
    fn validate_file_reader_paths_rejects_invalid_path() {
        let pipe = pipeline_from(
            "nodes:\n  reader:\n    kind: core::file_reader\n    params:\n      path: /etc/passwd\n",
        );
        let err = validate_file_reader_paths(&pipe, &dummy_security_config()).unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("reader"), "got: {msg}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_file_reader_paths_ignores_missing_params() {
        let pipe = pipeline_from("nodes:\n  reader:\n    kind: core::file_reader\n");
        validate_file_reader_paths(&pipe, &dummy_security_config()).unwrap();
    }

    #[test]
    fn validate_file_writer_paths_rejects_missing_params() {
        let pipe = pipeline_from("nodes:\n  writer:\n    kind: core::file_writer\n");
        let err = validate_file_writer_paths(&pipe, &dummy_security_config()).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("file_writer params"), "got: {msg}");
                assert!(msg.contains("writer"), "got: {msg}");
            },
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_file_writer_paths_rejects_non_string_path() {
        let pipe = pipeline_from(
            "nodes:\n  writer:\n    kind: core::file_writer\n    params:\n      path: 42\n",
        );
        let err = validate_file_writer_paths(&pipe, &dummy_security_config()).unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("path to be a string"), "got: {msg}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_file_writer_paths_rejects_disabled_writes() {
        let pipe = pipeline_from(
            "nodes:\n  writer:\n    kind: core::file_writer\n    params:\n      path: /tmp/out.mp4\n",
        );
        let err = validate_file_writer_paths(&pipe, &dummy_security_config()).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("Invalid write path"), "got: {msg}");
                assert!(msg.contains("writer"), "got: {msg}");
            },
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[cfg(feature = "script")]
    #[test]
    fn validate_script_paths_ignores_empty_script_path() {
        let pipe = pipeline_from(
            "nodes:\n  s:\n    kind: core::script\n    params:\n      script_path: \"   \"\n",
        );
        validate_script_paths(&pipe, &dummy_security_config()).unwrap();
    }

    #[cfg(feature = "script")]
    #[test]
    fn validate_script_paths_rejects_invalid_path() {
        let pipe = pipeline_from(
            "nodes:\n  s:\n    kind: core::script\n    params:\n      script_path: /etc/passwd\n",
        );
        let err = validate_script_paths(&pipe, &dummy_security_config()).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("Invalid script_path"), "got: {msg}");
                assert!(msg.contains("'s'"), "got: {msg}");
            },
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[cfg(feature = "script")]
    #[test]
    fn validate_script_paths_ignores_non_script_nodes() {
        let pipe = pipeline_from("nodes:\n  output:\n    kind: streamkit::http_output\n");
        validate_script_paths(&pipe, &dummy_security_config()).unwrap();
    }

    #[test]
    fn check_file_path_security_ok_for_clean_pipeline() {
        let pipe = pipeline_from("nodes:\n  output:\n    kind: streamkit::http_output\n");
        check_file_path_security(&pipe, &dummy_security_config()).unwrap();
    }

    #[test]
    fn check_file_path_security_returns_combined_errors() {
        let pipe = pipeline_from(
            "nodes:\n  reader:\n    kind: core::file_reader\n    params:\n      path: /etc/passwd\n  \
             writer:\n    kind: core::file_writer\n    params:\n      path: /tmp/out.mp4\n",
        );
        let msg = check_file_path_security(&pipe, &dummy_security_config()).unwrap_err();
        // The combined error contains both nodes' diagnostics separated by '; '.
        assert!(msg.contains("reader"), "got: {msg}");
        assert!(msg.contains("writer"), "got: {msg}");
        assert!(msg.contains(';'), "expected combined message, got: {msg}");
    }

    #[test]
    fn validate_pin_types_skips_passthrough_source() {
        let src = OutputPin {
            name: "out".into(),
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::Broadcast,
        };
        let dst = InputPin {
            name: "in".into(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        };
        let conn = streamkit_api::Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: streamkit_api::ConnectionMode::default(),
        };
        let pt_registry = streamkit_core::packet_meta::packet_type_registry();
        let mut errors = Vec::new();
        validate_pin_types(&src, &dst, &conn, "a->b", pt_registry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_pin_types_skips_passthrough_destination() {
        let src = OutputPin {
            name: "out".into(),
            produces_type: PacketType::Text,
            cardinality: PinCardinality::Broadcast,
        };
        let dst = InputPin {
            name: "in".into(),
            accepts_types: vec![PacketType::Passthrough],
            cardinality: PinCardinality::One,
        };
        let conn = streamkit_api::Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: streamkit_api::ConnectionMode::default(),
        };
        let pt_registry = streamkit_core::packet_meta::packet_type_registry();
        let mut errors = Vec::new();
        validate_pin_types(&src, &dst, &conn, "a->b", pt_registry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_pin_types_accepts_compatible_types() {
        let src = OutputPin {
            name: "out".into(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        };
        let dst = InputPin {
            name: "in".into(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        };
        let conn = streamkit_api::Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: streamkit_api::ConnectionMode::default(),
        };
        let pt_registry = streamkit_core::packet_meta::packet_type_registry();
        let mut errors = Vec::new();
        validate_pin_types(&src, &dst, &conn, "a->b", pt_registry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_pin_types_reports_incompatible_types() {
        let src = OutputPin {
            name: "out".into(),
            produces_type: PacketType::Text,
            cardinality: PinCardinality::Broadcast,
        };
        let dst = InputPin {
            name: "in".into(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        };
        let conn = streamkit_api::Connection {
            from_node: "a".into(),
            from_pin: "out".into(),
            to_node: "b".into(),
            to_pin: "in".into(),
            mode: streamkit_api::ConnectionMode::default(),
        };
        let pt_registry = streamkit_core::packet_meta::packet_type_registry();
        let mut errors = Vec::new();
        validate_pin_types(&src, &dst, &conn, "a->b", pt_registry, &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Type mismatch"));
    }

    #[test]
    fn app_error_message_extracts_strings_for_each_variant() {
        assert_eq!(app_error_message(AppError::BadRequest("bad".into())), "bad");
        assert_eq!(
            app_error_message(AppError::PipelineCompilation("compile-fail".into())),
            "compile-fail"
        );
        assert_eq!(app_error_message(AppError::Forbidden("nope".into())), "nope");
    }

    #[test]
    fn pipeline_mode_deserializes_lowercase() {
        let dyn_mode: PipelineMode = serde_json::from_str("\"dynamic\"").unwrap();
        assert!(matches!(dyn_mode, PipelineMode::Dynamic));
        let one_mode: PipelineMode = serde_json::from_str("\"oneshot\"").unwrap();
        assert!(matches!(one_mode, PipelineMode::Oneshot));
        assert!(serde_json::from_str::<PipelineMode>("\"unknown\"").is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod yaml_entrypoint_tests {
    use super::*;

    fn make_state() -> Arc<AppState> {
        crate::server::create_app_state(crate::config::Config::default(), None)
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_reports_parse_error() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let response = validate_pipeline_yaml(&state, &perms, "::: not yaml :::", None).unwrap();
        assert!(!response.valid);
        assert!(!response.errors.is_empty());
        assert!(matches!(response.errors[0].kind, DiagnosticKind::Parse));
        assert!(response.graph.is_none());
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_reports_empty_pipeline() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let response = validate_pipeline_yaml(&state, &perms, "nodes: {}", None).unwrap();
        assert!(!response.valid);
        assert!(response.errors.iter().any(|e| e.message.contains("Pipeline is empty")));
        assert!(response.graph.is_none());
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_returns_graph_for_valid_synthetic_pipeline() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let response = validate_pipeline_yaml(&state, &perms, yaml, None).unwrap();
        assert!(response.valid, "expected valid, errors={:?}", response.errors);
        let graph = response.graph.expect("graph populated for valid pipeline");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.connections.len(), 1);
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_rejects_synthetic_nodes_in_dynamic_mode() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let response =
            validate_pipeline_yaml(&state, &perms, yaml, Some(PipelineMode::Dynamic)).unwrap();
        assert!(!response.valid);
        assert!(response.errors.iter().any(|e| e.message.contains("only valid in oneshot")));
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_accepts_synthetic_nodes_in_oneshot_mode() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let response =
            validate_pipeline_yaml(&state, &perms, yaml, Some(PipelineMode::Oneshot)).unwrap();
        assert!(response.valid, "expected valid: {:?}", response.errors);
    }

    #[tokio::test]
    async fn validate_pipeline_yaml_surfaces_file_security_errors() {
        let state = make_state();
        let perms = crate::permissions::Permissions::admin();

        let yaml = "\
nodes:
  reader:
    kind: core::file_reader
    params:
      path: /etc/passwd
  output:
    kind: streamkit::http_output
    needs: reader
";
        let response = validate_pipeline_yaml(&state, &perms, yaml, None).unwrap();
        assert!(!response.valid);
        assert!(
            response.errors.iter().any(|e| matches!(e.kind, DiagnosticKind::Security)),
            "expected at least one security diagnostic, got: {:?}",
            response.errors
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handler_tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn router(state: Arc<AppState>) -> axum::Router {
        axum::Router::new()
            .route("/api/v1/validate", axum::routing::post(validate_pipeline_handler))
            .with_state(state)
    }

    async fn read_json(body: Body) -> serde_json::Value {
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn validate_handler_returns_diagnostics_for_invalid_yaml() {
        let state = crate::server::create_app_state(crate::config::Config::default(), None);
        let app = router(state);

        let body = serde_json::json!({ "yaml": "::: not yaml :::" }).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert_eq!(json["valid"], false);
        assert!(json["errors"].as_array().unwrap().iter().any(|e| e["kind"] == "parse"));
    }

    #[tokio::test]
    async fn validate_handler_returns_200_for_valid_pipeline() {
        let state = crate::server::create_app_state(crate::config::Config::default(), None);
        let app = router(state);

        let yaml = "\
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let body = serde_json::json!({ "yaml": yaml, "mode": "oneshot" }).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert_eq!(json["valid"], true, "got: {json}");
    }

    #[tokio::test]
    async fn validate_handler_rejects_callers_without_create_sessions() {
        // Use a trusted role header so the test is deterministic regardless of $SK_ROLE.
        let mut config = crate::config::Config::default();
        config.permissions.role_header = Some("x-test-role".to_string());
        let mut limited = crate::permissions::Permissions::viewer();
        limited.create_sessions = false;
        config.permissions.roles.insert("limited".to_string(), limited);

        let state = crate::server::create_app_state(config, None);
        let app = router(state);

        let body = serde_json::json!({ "yaml": "nodes: {}" }).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .header("x-test-role", "limited")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
