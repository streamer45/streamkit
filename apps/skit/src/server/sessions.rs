// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use opentelemetry::global;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::state::AppState;
use streamkit_api::yaml::{compile, UserPipeline};
use streamkit_api::{Event as ApiEvent, EventPayload, MessageType, Pipeline};
use streamkit_core::control::EngineControlMessage;

use super::preview;
use super::validation::{check_file_path_security, is_synthetic_kind};

/// Request body for creating a session with a pipeline
#[derive(Debug, Deserialize)]
pub(super) struct CreateSessionRequest {
    name: Option<String>,
    yaml: String,
}

/// Response body for creating a session
#[derive(Debug, Serialize)]
pub(super) struct CreateSessionResponse {
    session_id: String,
    name: Option<String>,
    created_at: String,
}

/// Populate the session's in-memory pipeline from the compiled engine definition.
pub async fn populate_session_pipeline(
    session: &crate::session::Session,
    engine_pipeline: &Pipeline,
) {
    let mut pipeline = session.pipeline.lock().await;

    // Forward top-level metadata so the UI can read it from the session snapshot.
    pipeline.name.clone_from(&engine_pipeline.name);
    pipeline.description.clone_from(&engine_pipeline.description);
    pipeline.mode = engine_pipeline.mode;
    pipeline.client.clone_from(&engine_pipeline.client);

    for (node_id, node_spec) in &engine_pipeline.nodes {
        pipeline.nodes.insert(
            node_id.clone(),
            streamkit_api::Node {
                kind: node_spec.kind.clone(),
                params: node_spec.params.clone(),
                state: None,
            },
        );
    }

    pipeline.connections.extend(engine_pipeline.connections.iter().map(|c| {
        streamkit_api::Connection {
            from_node: c.from_node.clone(),
            from_pin: c.from_pin.clone(),
            to_node: c.to_node.clone(),
            to_pin: c.to_pin.clone(),
            mode: c.mode,
        }
    }));
}

/// Send all node and connection control messages to the engine actor.
pub async fn send_pipeline_to_engine(
    session: &crate::session::Session,
    engine_pipeline: &Pipeline,
) {
    for (node_id, node_spec) in &engine_pipeline.nodes {
        session
            .send_control_message(EngineControlMessage::AddNode {
                node_id: node_id.clone(),
                kind: node_spec.kind.clone(),
                params: node_spec.params.clone(),
            })
            .await;
    }

    for conn in &engine_pipeline.connections {
        let core_mode = match conn.mode {
            streamkit_api::ConnectionMode::Reliable => {
                streamkit_core::control::ConnectionMode::Reliable
            },
            streamkit_api::ConnectionMode::BestEffort => {
                streamkit_core::control::ConnectionMode::BestEffort
            },
        };
        session
            .send_control_message(EngineControlMessage::Connect {
                from_node: conn.from_node.clone(),
                from_pin: conn.from_pin.clone(),
                to_node: conn.to_node.clone(),
                to_pin: conn.to_pin.clone(),
                mode: core_mode,
            })
            .await;
    }
}

/// Axum handler to create a new session with a pipeline from YAML.
pub(super) async fn create_session_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, (StatusCode, String)> {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.create_sessions {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot create sessions".to_string(),
        ));
    }

    let result = create_dynamic_session(&app_state, &req.yaml, req.name, role_name, &perms).await;

    match result {
        Ok(r) => Ok(Json(CreateSessionResponse {
            session_id: r.session_id,
            name: r.name,
            created_at: r.created_at,
        })),
        Err(e) => Err(match e {
            CreateSessionError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg),
            CreateSessionError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            CreateSessionError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            CreateSessionError::LimitReached(msg) => (StatusCode::TOO_MANY_REQUESTS, msg),
            CreateSessionError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        }),
    }
}

/// Axum handler to get the list of active sessions.
pub(super) async fn list_sessions_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.list_sessions {
        return (StatusCode::FORBIDDEN, "Permission denied: cannot list sessions".to_string())
            .into_response();
    }

    let sessions = app_state.session_manager.lock().await.list_sessions();
    let session_infos: Vec<streamkit_api::SessionInfo> = sessions
        .into_iter()
        .filter(|session| {
            if perms.access_all_sessions {
                return true;
            }
            session.created_by.as_ref().is_none_or(|creator| creator == &role_name)
        })
        .map(|session| streamkit_api::SessionInfo {
            id: session.id,
            name: session.name,
            created_at: crate::session::system_time_to_rfc3339(session.created_at),
        })
        .collect();
    info!("Listed {} active sessions via HTTP", session_infos.len());
    Json(session_infos).into_response()
}

/// Axum handler to destroy a session.
pub(super) async fn destroy_session_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.destroy_sessions {
        warn!(
            session_id = %session_id,
            destroy_sessions = perms.destroy_sessions,
            "Blocked attempt to destroy session via HTTP: permission denied"
        );
        return (StatusCode::FORBIDDEN, "Permission denied: cannot destroy sessions".to_string())
            .into_response();
    }

    let removed_session = {
        let mut session_manager = app_state.session_manager.lock().await;

        let Some(session) = session_manager.get_session_by_name_or_id(&session_id) else {
            return (StatusCode::NOT_FOUND, format!("Session '{session_id}' not found"))
                .into_response();
        };

        if !perms.access_all_sessions
            && session.created_by.as_ref().is_some_and(|creator| creator != &role_name)
        {
            warn!(
                session_id = %session_id,
                role = %role_name,
                "Blocked attempt to destroy session via HTTP: not owner"
            );
            return (
                StatusCode::FORBIDDEN,
                "Permission denied: you do not own this session".to_string(),
            )
                .into_response();
        }

        session_manager.remove_session_by_id(&session.id)
    };

    let Some(session) = removed_session else {
        return (StatusCode::NOT_FOUND, format!("Session '{session_id}' not found"))
            .into_response();
    };

    let destroyed_id = session.id.clone();

    // Broadcast event to all WebSocket clients BEFORE starting shutdown
    // so clients are notified immediately.  The session has already been
    // removed from the manager so ListSessions will no longer include it.
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionDestroyed { session_id: destroyed_id.clone() },
    };
    if let Err(e) = app_state.event_tx.send(crate::state::BroadcastEvent::to_all(event)) {
        error!("Failed to broadcast SessionDestroyed event: {}", e);
    }

    // Run engine shutdown in a background task so the HTTP response
    // returns immediately (shutdown_and_wait has a 10-second timeout).
    let shutdown_id = destroyed_id.clone();
    let tracker = app_state.shutdown_tracker.clone();
    let handle = tokio::spawn(async move {
        // Tear down any active previews before shutting down the engine.
        #[cfg(feature = "moq")]
        preview::teardown_all_previews(&session).await;

        if let Err(e) = session.shutdown_and_wait().await {
            warn!(session_id = %shutdown_id, error = %e, "Error during engine shutdown");
            global::meter("skit_server").u64_counter("session.shutdown.errors").build().add(1, &[]);
        } else {
            info!(session_id = %shutdown_id, "Session destroyed successfully via HTTP");
        }
    });
    tracker.track(handle).await;

    (StatusCode::OK, Json(serde_json::json!({ "session_id": destroyed_id }))).into_response()
}

/// Error type returned by [`create_dynamic_session`].
///
/// Each variant carries enough semantic meaning for both HTTP and MCP callers
/// to map to the appropriate protocol-level error (e.g. status codes for HTTP,
/// `McpError` variants for MCP).
pub enum CreateSessionError {
    /// Invalid input (YAML parse, compile, empty pipeline, synthetic nodes,
    /// bad file paths).
    InvalidInput(String),
    /// Permission denied (node or plugin not allowed).
    Forbidden(String),
    /// Session name already taken.
    Conflict(String),
    /// Maximum concurrent-session limit reached.
    LimitReached(String),
    /// Internal failure (engine allocation, session insert, etc.).
    Internal(String),
}

/// Result returned by [`create_dynamic_session`] on success.
pub struct CreateSessionResult {
    pub session_id: String,
    pub name: Option<String>,
    pub created_at: String,
}

/// Shared implementation for creating a dynamic pipeline session.
///
/// Handles YAML parsing, compilation, permission checks, file-path security,
/// session-limit pre-flight, engine allocation, session insertion, pipeline
/// population, engine dispatch, and event broadcast.
///
/// Callers are responsible for extracting auth and checking
/// `perms.create_sessions` before calling this function.
///
/// # Errors
///
/// Returns a [`CreateSessionError`] variant matching the failure category
/// (invalid input, permission denied, name conflict, session limit, or
/// internal error).
pub async fn create_dynamic_session(
    app_state: &Arc<AppState>,
    yaml: &str,
    name: Option<String>,
    role_name: String,
    perms: &crate::permissions::Permissions,
) -> Result<CreateSessionResult, CreateSessionError> {
    let user_pipeline: UserPipeline = streamkit_api::yaml::parse_yaml(yaml)
        .map_err(|e| CreateSessionError::InvalidInput(format!("YAML parse error: {e}")))?;

    let engine_pipeline = compile(user_pipeline).map_err(|e| {
        CreateSessionError::InvalidInput(format!("Pipeline compilation error: {e}"))
    })?;

    if engine_pipeline.nodes.is_empty() {
        return Err(CreateSessionError::InvalidInput(
            "Pipeline is empty. Add some nodes before creating a session.".to_string(),
        ));
    }

    for (node_id, node) in &engine_pipeline.nodes {
        if is_synthetic_kind(&node.kind) {
            return Err(CreateSessionError::InvalidInput(format!(
                "Node '{node_id}' kind '{}' is oneshot-only and cannot be used in dynamic sessions",
                node.kind
            )));
        }
        if !perms.is_node_allowed(&node.kind) {
            return Err(CreateSessionError::Forbidden(format!(
                "Permission denied: node '{node_id}' kind '{}' not allowed",
                node.kind
            )));
        }
        if node.kind.starts_with("plugin::") && !perms.is_plugin_allowed(&node.kind) {
            return Err(CreateSessionError::Forbidden(format!(
                "Permission denied: node '{node_id}' plugin '{}' not allowed",
                node.kind
            )));
        }
    }

    // File-path security — policy violations are permission denials, not
    // malformed input (preserves the 403 FORBIDDEN status the old HTTP
    // handler returned for AppError::Forbidden from validate_file_*_paths).
    check_file_path_security(&engine_pipeline, &app_state.config.security)
        .map_err(CreateSessionError::Forbidden)?;

    // Pre-flight: reject early if over the session limit or name is taken,
    // avoiding wasted engine allocation.  The checks are re-verified under
    // the lock inside add_session for correctness.
    let (current_count, name_taken) = {
        let sm = app_state.session_manager.lock().await;
        (sm.session_count(), name.as_deref().is_some_and(|n| sm.is_name_taken(n)))
    };
    if let Some(ref session_name) = name {
        if name_taken {
            return Err(CreateSessionError::Conflict(format!(
                "Session with name '{session_name}' already exists"
            )));
        }
    }
    if !app_state.config.permissions.can_accept_session(current_count) {
        return Err(CreateSessionError::LimitReached(
            "Maximum concurrent sessions limit reached".to_string(),
        ));
    }

    let resolved_attributes = crate::metrics_labels::resolve_attributes(
        engine_pipeline.attributes.as_ref(),
        &app_state.config.server.metrics.attributes,
    );

    let session = crate::session::Session::create(
        &app_state.engine,
        &app_state.config,
        name,
        app_state.event_tx.clone(),
        Some(role_name),
        app_state.asset_root.clone(),
        resolved_attributes,
    )
    .await
    .map_err(|e| CreateSessionError::Internal(format!("Failed to create session: {e}")))?;

    // Insert under the lock (re-checks limit and name uniqueness).
    let insert_result = {
        let mut sm = app_state.session_manager.lock().await;
        let count = sm.session_count();
        if app_state.config.permissions.can_accept_session(count) {
            sm.add_session(session.clone())
        } else {
            Err("Maximum concurrent sessions limit reached".to_string())
        }
    };
    if let Err(msg) = insert_result {
        warn!(error = %msg, "create_dynamic_session failed during insert");
        let _ = session.shutdown_and_wait().await;
        if msg.contains("limit reached") {
            return Err(CreateSessionError::LimitReached(msg));
        }
        return Err(CreateSessionError::Internal(format!("Failed to create session: {msg}")));
    }

    let session_id = session.id.clone();
    let session_name = session.name.clone();
    let created_at = crate::session::system_time_to_rfc3339(session.created_at);

    populate_session_pipeline(&session, &engine_pipeline).await;
    send_pipeline_to_engine(&session, &engine_pipeline).await;

    info!(
        session_id = %session_id,
        name = ?session_name,
        nodes = engine_pipeline.nodes.len(),
        connections = engine_pipeline.connections.len(),
        "Created new session"
    );

    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionCreated {
            session_id: session_id.clone(),
            name: session_name.clone(),
            created_at: created_at.clone(),
        },
    };
    if app_state.event_tx.send(crate::state::BroadcastEvent::to_all(event)).is_err() {
        debug!("No WebSocket clients connected to receive SessionCreated event");
    }

    Ok(CreateSessionResult { session_id, name: session_name, created_at })
}

/// Validate a batch of operations against a session's pipeline without applying.
///
/// Returns a list of validation errors.  An empty list means all operations
/// are valid.  Callers must perform session-level permission and ownership
/// checks before calling this function.
/// Check batch operations for duplicate node IDs by simulating the
/// Add/Remove sequence.  Returns the IDs of nodes that would collide.
pub(super) async fn check_batch_node_id_uniqueness(
    session: &crate::session::Session,
    operations: &[streamkit_api::BatchOperation],
) -> Vec<String> {
    let mut live_ids: std::collections::HashSet<String> =
        session.pipeline.lock().await.nodes.keys().cloned().collect();
    let mut duplicates = Vec::new();
    for op in operations {
        match op {
            streamkit_api::BatchOperation::AddNode { node_id, .. }
                if !live_ids.insert(node_id.clone()) =>
            {
                duplicates.push(node_id.clone());
            },
            streamkit_api::BatchOperation::RemoveNode { node_id } => {
                live_ids.remove(node_id.as_str());
            },
            _ => {},
        }
    }
    duplicates
}

pub async fn validate_batch_operations(
    session: &crate::session::Session,
    operations: &[streamkit_api::BatchOperation],
    perms: &crate::permissions::Permissions,
    security_config: &crate::config::SecurityConfig,
) -> Vec<streamkit_api::ValidationError> {
    let mut errors: Vec<streamkit_api::ValidationError> = Vec::new();

    for node_id in check_batch_node_id_uniqueness(session, operations).await {
        errors.push(streamkit_api::ValidationError {
            error_type: streamkit_api::ValidationErrorType::Error,
            message: format!("Batch rejected: node '{node_id}' already exists in the pipeline"),
            node_id: Some(node_id),
            connection_id: None,
        });
    }

    for op in operations {
        if let streamkit_api::BatchOperation::AddNode { node_id, kind, params, .. } = op {
            if let Some(message) = crate::websocket_handlers::validate_add_node_op(
                kind,
                params.as_ref(),
                perms,
                security_config,
            ) {
                errors.push(streamkit_api::ValidationError {
                    error_type: streamkit_api::ValidationErrorType::Error,
                    message,
                    node_id: Some(node_id.clone()),
                    connection_id: None,
                });
            }
        }
    }

    errors
}

/// Apply a batch of graph mutations atomically to a running session.
///
/// Returns `Ok(())` on success, or `Err(message)` if pre-validation fails
/// (e.g. duplicate node IDs or forbidden node kinds).  Callers must perform
/// session-level permission and ownership checks before calling this function.
///
/// # Errors
///
/// Returns an error string when a batch operation fails pre-validation
/// (duplicate node IDs or forbidden node kinds).
pub async fn apply_batch_operations(
    session: &crate::session::Session,
    operations: Vec<streamkit_api::BatchOperation>,
    perms: &crate::permissions::Permissions,
    security_config: &crate::config::SecurityConfig,
) -> Result<(), String> {
    let duplicates = check_batch_node_id_uniqueness(session, &operations).await;
    if let Some(node_id) = duplicates.first() {
        return Err(format!("Batch rejected: node '{node_id}' already exists in the pipeline"));
    }

    for op in &operations {
        if let streamkit_api::BatchOperation::AddNode { kind, params, .. } = op {
            if let Some(message) = crate::websocket_handlers::validate_add_node_op(
                kind,
                params.as_ref(),
                perms,
                security_config,
            ) {
                return Err(message);
            }
        }
    }

    let mut engine_operations = Vec::new();
    {
        let mut pipeline = session.pipeline.lock().await;
        for op in operations {
            match op {
                streamkit_api::BatchOperation::AddNode { node_id, kind, params } => {
                    pipeline.nodes.insert(
                        node_id.clone(),
                        streamkit_api::Node {
                            kind: kind.clone(),
                            params: params.clone(),
                            state: None,
                        },
                    );
                    engine_operations.push(
                        streamkit_core::control::EngineControlMessage::AddNode {
                            node_id,
                            kind,
                            params,
                        },
                    );
                },
                streamkit_api::BatchOperation::RemoveNode { node_id } => {
                    pipeline.nodes.shift_remove(&node_id);
                    pipeline
                        .connections
                        .retain(|conn| conn.from_node != node_id && conn.to_node != node_id);
                    engine_operations.push(
                        streamkit_core::control::EngineControlMessage::RemoveNode { node_id },
                    );
                },
                streamkit_api::BatchOperation::Connect {
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                    mode,
                } => {
                    pipeline.connections.push(streamkit_api::Connection {
                        from_node: from_node.clone(),
                        from_pin: from_pin.clone(),
                        to_node: to_node.clone(),
                        to_pin: to_pin.clone(),
                        mode,
                    });
                    let core_mode = match mode {
                        streamkit_api::ConnectionMode::Reliable => {
                            streamkit_core::control::ConnectionMode::Reliable
                        },
                        streamkit_api::ConnectionMode::BestEffort => {
                            streamkit_core::control::ConnectionMode::BestEffort
                        },
                    };
                    engine_operations.push(
                        streamkit_core::control::EngineControlMessage::Connect {
                            from_node,
                            from_pin,
                            to_node,
                            to_pin,
                            mode: core_mode,
                        },
                    );
                },
                streamkit_api::BatchOperation::Disconnect {
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                } => {
                    pipeline.connections.retain(|conn| {
                        !(conn.from_node == from_node
                            && conn.from_pin == from_pin
                            && conn.to_node == to_node
                            && conn.to_pin == to_pin)
                    });
                    engine_operations.push(
                        streamkit_core::control::EngineControlMessage::Disconnect {
                            from_node,
                            from_pin,
                            to_node,
                            to_pin,
                        },
                    );
                },
            }
        }
        drop(pipeline);
    }

    for msg in engine_operations {
        session.send_control_message(msg).await;
    }

    Ok(())
}

/// Send a control message to a specific node in a running session.
///
/// For `UpdateParams` messages, this function also validates file-path
/// security, updates the durable pipeline model, and broadcasts a
/// `NodeParamsChanged` event.  Callers must perform session-level
/// permission and ownership checks before calling this function.
///
/// # Errors
///
/// Returns an error string when the security policy rejects the
/// `UpdateParams` payload.
pub async fn tune_session_node(
    session: &crate::session::Session,
    node_id: String,
    message: streamkit_core::control::NodeControlMessage,
    security_config: &crate::config::SecurityConfig,
    event_tx: &tokio::sync::broadcast::Sender<crate::state::BroadcastEvent>,
) -> Result<(), String> {
    tune_session_node_inner(session, node_id, message, security_config, event_tx, false).await
}

/// Like [`tune_session_node`] but the durable params are fully replaced
/// instead of deep-merged. Used by `update_pipeline` which needs declarative
/// "desired state" semantics.
///
/// # Errors
///
/// Returns an error string when the security policy rejects the
/// `UpdateParams` payload.
#[cfg(feature = "mcp")]
pub async fn tune_session_node_replace(
    session: &crate::session::Session,
    node_id: String,
    message: streamkit_core::control::NodeControlMessage,
    security_config: &crate::config::SecurityConfig,
    event_tx: &tokio::sync::broadcast::Sender<crate::state::BroadcastEvent>,
) -> Result<(), String> {
    tune_session_node_inner(session, node_id, message, security_config, event_tx, true).await
}

pub(super) async fn tune_session_node_inner(
    session: &crate::session::Session,
    node_id: String,
    message: streamkit_core::control::NodeControlMessage,
    security_config: &crate::config::SecurityConfig,
    event_tx: &tokio::sync::broadcast::Sender<crate::state::BroadcastEvent>,
    replace: bool,
) -> Result<(), String> {
    use streamkit_core::control::NodeControlMessage;

    if let NodeControlMessage::UpdateParams(ref params) = message {
        let kind = {
            let pipeline = session.pipeline.lock().await;
            pipeline.nodes.get(&node_id).map(|n| n.kind.clone())
        };

        if !crate::websocket_handlers::validate_update_params_security(
            kind.as_deref(),
            params,
            security_config,
        ) {
            return Err("Security policy rejected the UpdateParams payload".to_string());
        }

        {
            let mut durable_params = params.clone();
            if let serde_json::Value::Object(ref mut map) = durable_params {
                map.retain(|k, _| !k.starts_with('_'));
            }
            let mut pipeline = session.pipeline.lock().await;
            if let Some(node) = pipeline.nodes.get_mut(&node_id) {
                node.params = Some(if replace {
                    durable_params
                } else {
                    match node.params.take() {
                        Some(existing) => {
                            crate::websocket_handlers::deep_merge_json(existing, durable_params)
                        },
                        None => durable_params,
                    }
                });
            }
        }

        let event = streamkit_api::Event {
            message_type: streamkit_api::MessageType::Event,
            correlation_id: None,
            payload: streamkit_api::EventPayload::NodeParamsChanged {
                session_id: session.id.clone(),
                node_id: node_id.clone(),
                params: params.clone(),
            },
        };
        if let Err(e) = event_tx.send(crate::state::BroadcastEvent::to_all(event)) {
            tracing::error!("Failed to broadcast NodeParamsChanged event: {}", e);
        }
    }

    let control_msg = streamkit_core::control::EngineControlMessage::TuneNode { node_id, message };
    session.send_control_message(control_msg).await;

    Ok(())
}

#[cfg(test)]
// test assertions intentionally use unwrap/expect so a failed setup panics with a clear message
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod sessions_batch_tests {
    use super::*;
    use crate::config::SecurityConfig;
    use crate::permissions::Permissions;
    use crate::session::Session;
    use crate::state::BroadcastEvent;
    use streamkit_api::{BatchOperation, ConnectionMode, ValidationErrorType};
    use streamkit_engine::Engine;
    use tempfile::NamedTempFile;
    use tokio::sync::broadcast;

    /// Spin up a real `Engine` + `Session` for unit-testing the batch
    /// helpers.  Mirrors `create_dynamic_session` minus the YAML compile
    /// and `SessionManager` insertion: tests only inspect the durable
    /// pipeline model owned by the returned session.  The receiver is
    /// kept alive so the engine actor's broadcast sender does not error
    /// on send (which would still be benign, but noisy).
    async fn fresh_session() -> (Session, broadcast::Receiver<BroadcastEvent>) {
        let engine = Engine::without_plugins();
        let config = crate::config::Config::default();
        let (tx, rx) = broadcast::channel(16);
        let session = Session::create(
            &engine,
            &config,
            Some("batch-helpers-test".to_string()),
            tx,
            Some("test-role".to_string()),
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            streamkit_engine::ResolvedAttributes::default(),
        )
        .await
        .expect("Session::create on a fresh engine should succeed");
        (session, rx)
    }

    /// Permissions that allow `core::passthrough` only.  Anything else
    /// (notably `core::sink`) is forbidden, which gives us a clean
    /// permission-denied path without depending on `Permissions::user`'s
    /// exact allow-list.
    fn passthrough_only_perms() -> Permissions {
        let mut perms = Permissions::user();
        perms.allowed_nodes = vec!["core::passthrough".to_string()];
        perms
    }

    fn add_op(node_id: &str, kind: &str) -> BatchOperation {
        BatchOperation::AddNode {
            node_id: node_id.to_string(),
            kind: kind.to_string(),
            params: None,
        }
    }

    fn remove_op(node_id: &str) -> BatchOperation {
        BatchOperation::RemoveNode { node_id: node_id.to_string() }
    }

    fn connect_op(from: (&str, &str), to: (&str, &str)) -> BatchOperation {
        BatchOperation::Connect {
            from_node: from.0.to_string(),
            from_pin: from.1.to_string(),
            to_node: to.0.to_string(),
            to_pin: to.1.to_string(),
            mode: ConnectionMode::Reliable,
        }
    }

    fn disconnect_op(from: (&str, &str), to: (&str, &str)) -> BatchOperation {
        BatchOperation::Disconnect {
            from_node: from.0.to_string(),
            from_pin: from.1.to_string(),
            to_node: to.0.to_string(),
            to_pin: to.1.to_string(),
        }
    }

    /// Seed the session's durable pipeline model directly.  We bypass
    /// the engine because these unit tests only exercise the helpers'
    /// view of `pipeline.nodes`, not the actor's state.
    async fn preinsert_node(session: &Session, node_id: &str, kind: &str) {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.nodes.insert(
            node_id.to_string(),
            streamkit_api::Node { kind: kind.to_string(), params: None, state: None },
        );
    }

    #[tokio::test]
    async fn check_uniqueness_empty_ops_returns_empty() {
        let (session, _rx) = fresh_session().await;
        let conflicts = check_batch_node_id_uniqueness(&session, &[]).await;
        assert!(conflicts.is_empty(), "expected no conflicts, got: {conflicts:?}");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn check_uniqueness_addnode_collides_with_existing_pipeline_node() {
        let (session, _rx) = fresh_session().await;
        preinsert_node(&session, "alpha", "core::passthrough").await;

        let ops = vec![add_op("alpha", "core::passthrough")];
        let conflicts = check_batch_node_id_uniqueness(&session, &ops).await;

        assert_eq!(conflicts, vec!["alpha".to_string()]);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn check_uniqueness_duplicate_within_batch_reports_id_once() {
        let (session, _rx) = fresh_session().await;
        let ops = vec![add_op("beta", "core::passthrough"), add_op("beta", "core::passthrough")];

        let conflicts = check_batch_node_id_uniqueness(&session, &ops).await;

        // First Add seeds the live set; the second Add for the same id
        // is the one reported.  The contract is "per collision", which
        // for two identical Adds means one entry.
        assert_eq!(conflicts, vec!["beta".to_string()]);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn check_uniqueness_remove_then_add_for_existing_node_does_not_conflict() {
        let (session, _rx) = fresh_session().await;
        preinsert_node(&session, "gamma", "core::passthrough").await;

        let ops = vec![remove_op("gamma"), add_op("gamma", "core::passthrough")];
        let conflicts = check_batch_node_id_uniqueness(&session, &ops).await;

        assert!(
            conflicts.is_empty(),
            "RemoveNode before AddNode for the same id must not report a conflict, got: {conflicts:?}",
        );
        let _ = session.shutdown_and_wait().await;
    }

    // Documents the current order-sensitive behavior: an AddNode that
    // collides with the live pipeline is reported as a conflict even
    // when a later RemoveNode in the same batch would clear it.  See
    // the follow-up note in the PR description.
    #[tokio::test]
    async fn check_uniqueness_add_then_remove_for_existing_node_still_conflicts() {
        let (session, _rx) = fresh_session().await;
        preinsert_node(&session, "delta", "core::passthrough").await;

        let ops = vec![add_op("delta", "core::passthrough"), remove_op("delta")];
        let conflicts = check_batch_node_id_uniqueness(&session, &ops).await;

        assert_eq!(conflicts, vec!["delta".to_string()]);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn validate_empty_ops_returns_no_errors() {
        let (session, _rx) = fresh_session().await;
        let errors = validate_batch_operations(
            &session,
            &[],
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn validate_forbidden_node_kind_reports_permission_denied() {
        let (session, _rx) = fresh_session().await;
        let ops = vec![add_op("forbidden", "core::sink")];

        let errors = validate_batch_operations(
            &session,
            &ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;

        assert_eq!(errors.len(), 1, "expected exactly one error, got {errors:?}");
        let err = &errors[0];
        assert!(matches!(err.error_type, ValidationErrorType::Error));
        assert_eq!(err.node_id.as_deref(), Some("forbidden"));
        assert!(err.connection_id.is_none());
        assert!(
            err.message.starts_with("Permission denied:"),
            "expected 'Permission denied:' prefix, got: {}",
            err.message,
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn validate_file_reader_outside_allowed_paths_reports_file_security_error() {
        let (session, _rx) = fresh_session().await;
        // Admin perms so the kind itself is allowed: only the file-path
        // check should fire.
        let perms = Permissions::admin();
        // Default allowed_file_paths is `["samples/**"]` (relative to
        // cwd).  A real existing file outside that glob must fail the
        // security check.
        let tmp = NamedTempFile::new().expect("failed to create tempfile");
        let outside_path = tmp.path().to_string_lossy().to_string();

        let ops = vec![BatchOperation::AddNode {
            node_id: "reader".to_string(),
            kind: "core::file_reader".to_string(),
            params: Some(serde_json::json!({ "path": outside_path })),
        }];

        let errors =
            validate_batch_operations(&session, &ops, &perms, &SecurityConfig::default()).await;

        assert_eq!(errors.len(), 1, "expected one file-security error, got {errors:?}");
        let err = &errors[0];
        assert!(matches!(err.error_type, ValidationErrorType::Error));
        assert_eq!(err.node_id.as_deref(), Some("reader"));
        assert!(
            err.message.starts_with("Invalid file path:"),
            "expected 'Invalid file path:' prefix, got: {}",
            err.message,
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn validate_duplicate_id_in_batch_is_reported() {
        let (session, _rx) = fresh_session().await;
        let ops = vec![add_op("dup", "core::passthrough"), add_op("dup", "core::passthrough")];

        let errors = validate_batch_operations(
            &session,
            &ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;

        let dup_errs: Vec<_> =
            errors.iter().filter(|e| e.node_id.as_deref() == Some("dup")).collect();
        assert!(!dup_errs.is_empty(), "expected a duplicate-id error for 'dup', got: {errors:?}");
        assert!(matches!(dup_errs[0].error_type, ValidationErrorType::Error));
        assert!(
            dup_errs[0].message.contains("already exists"),
            "expected 'already exists' in message, got: {}",
            dup_errs[0].message,
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn validate_happy_path_returns_no_errors() {
        let (session, _rx) = fresh_session().await;
        let ops = vec![add_op("a", "core::passthrough"), add_op("b", "core::passthrough")];

        let errors = validate_batch_operations(
            &session,
            &ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;

        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_duplicate_id_is_rejected_and_pipeline_unchanged() {
        let (session, _rx) = fresh_session().await;
        preinsert_node(&session, "existing", "core::passthrough").await;
        let before_len = session.pipeline.lock().await.nodes.len();

        let ops = vec![add_op("existing", "core::passthrough")];
        let result = apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;

        let err = result.expect_err("expected Err for duplicate id");
        assert!(err.contains("already exists"), "unexpected error message: {err}");

        let after = session.pipeline.lock().await;
        assert_eq!(after.nodes.len(), before_len, "pipeline must not be mutated on rejection");
        assert!(after.nodes.contains_key("existing"));
        drop(after);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_forbidden_kind_is_rejected_and_pipeline_unchanged() {
        let (session, _rx) = fresh_session().await;
        let before_len = session.pipeline.lock().await.nodes.len();

        let ops = vec![add_op("nope", "core::sink")];
        let result = apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await;

        let err = result.expect_err("expected Err for forbidden kind");
        assert!(err.starts_with("Permission denied:"), "unexpected error message: {err}");

        let after = session.pipeline.lock().await;
        assert_eq!(after.nodes.len(), before_len);
        assert!(!after.nodes.contains_key("nope"));
        drop(after);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_happy_addnode_mutates_pipeline_and_records_params() {
        let (session, _rx) = fresh_session().await;

        let ops = vec![BatchOperation::AddNode {
            node_id: "new".to_string(),
            kind: "core::passthrough".to_string(),
            params: Some(serde_json::json!({ "k": "v" })),
        }];
        apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("happy AddNode must succeed");

        let pipeline = session.pipeline.lock().await;
        let node = pipeline.nodes.get("new").expect("'new' should be in pipeline.nodes");
        assert_eq!(node.kind, "core::passthrough");
        assert_eq!(node.params, Some(serde_json::json!({ "k": "v" })));
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_connect_after_addnode_records_connection() {
        let (session, _rx) = fresh_session().await;
        let ops = vec![
            add_op("src", "core::passthrough"),
            add_op("dst", "core::passthrough"),
            connect_op(("src", "out"), ("dst", "in")),
        ];

        apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("connect after AddNode must succeed");

        let pipeline = session.pipeline.lock().await;
        assert!(pipeline.nodes.contains_key("src"));
        assert!(pipeline.nodes.contains_key("dst"));
        let conn = pipeline
            .connections
            .iter()
            .find(|c| c.from_node == "src" && c.to_node == "dst")
            .expect("expected a 'src -> dst' connection");
        assert_eq!(conn.from_pin, "out");
        assert_eq!(conn.to_pin, "in");
        assert_eq!(conn.mode, ConnectionMode::Reliable);
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_disconnect_removes_existing_connection() {
        let (session, _rx) = fresh_session().await;
        let setup = vec![
            add_op("a", "core::passthrough"),
            add_op("b", "core::passthrough"),
            connect_op(("a", "out"), ("b", "in")),
        ];
        apply_batch_operations(
            &session,
            setup,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("setup batch must succeed");
        assert_eq!(session.pipeline.lock().await.connections.len(), 1);

        let ops = vec![disconnect_op(("a", "out"), ("b", "in"))];
        apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("disconnect must succeed");

        let pipeline = session.pipeline.lock().await;
        assert!(
            pipeline.connections.is_empty(),
            "connection should be removed, got: {:?}",
            pipeline.connections,
        );
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_removenode_drops_node_and_incident_connections() {
        let (session, _rx) = fresh_session().await;
        let setup = vec![
            add_op("a", "core::passthrough"),
            add_op("b", "core::passthrough"),
            add_op("c", "core::passthrough"),
            connect_op(("a", "out"), ("b", "in")),
            connect_op(("b", "out"), ("c", "in")),
        ];
        apply_batch_operations(
            &session,
            setup,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("setup batch must succeed");
        assert_eq!(session.pipeline.lock().await.connections.len(), 2);

        let ops = vec![remove_op("b")];
        apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
        )
        .await
        .expect("RemoveNode must succeed");

        let pipeline = session.pipeline.lock().await;
        assert!(!pipeline.nodes.contains_key("b"), "'b' should be removed");
        assert!(pipeline.nodes.contains_key("a"));
        assert!(pipeline.nodes.contains_key("c"));
        assert!(
            pipeline.connections.is_empty(),
            "all connections incident on 'b' should be removed, got: {:?}",
            pipeline.connections,
        );
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
    }
}
