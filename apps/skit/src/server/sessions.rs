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
    pipeline.attributes.clone_from(&engine_pipeline.attributes);

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
    check_file_path_security(&engine_pipeline, &app_state.config.security, &app_state.asset_root)
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

    let resolved_attributes = app_state.resolve_metric_attributes(&engine_pipeline);

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

/// Engine nodes lingering in a terminal (`Failed`/`Stopped`) state, mapped
/// to a human-readable label.
///
/// The engine keeps terminal nodes in `node_states` until an explicit
/// `RemoveNode`, and its duplicate guard rejects any add for an id already
/// present there — silently, with no event. On the failing transition the
/// session drops such ids from both `pipeline.nodes` and `creating_nodes`,
/// so they survive only in the engine; a re-add must consult the engine to
/// avoid being silently swallowed.
///
/// Returns `Err` if the engine state can't be queried, so callers can fail
/// closed rather than skip the guard.
async fn engine_terminal_node_ids(
    session: &crate::session::Session,
) -> Result<std::collections::HashMap<String, &'static str>, String> {
    Ok(session
        .get_node_states()
        .await?
        .iter()
        .filter_map(|(id, state)| {
            crate::session::terminal_node_label(state).map(|label| (id.clone(), label))
        })
        .collect())
}

/// A node-id collision found while validating a batch, paired with a
/// caller-ready explanation.
pub(super) struct BatchIdConflict {
    pub node_id: String,
    pub message: String,
}

/// Simulate a batch's Add/Remove sequence and classify every node-id
/// collision against confirmed nodes (`live`), in-flight reservations
/// (`in_flight`, a subset of `live`), the engine's terminal residue, and
/// earlier ops in the same batch.  Pure: callers own the locking that produces
/// `live`/`in_flight` and the engine query that produces `terminal`.  A
/// `RemoveNode` clears an id from the running set so a same-batch
/// remove-then-re-add is not flagged.
fn batch_id_conflicts(
    live: &std::collections::HashSet<String>,
    in_flight: &std::collections::HashSet<String>,
    terminal: &std::collections::HashMap<String, &'static str>,
    operations: &[streamkit_api::BatchOperation],
) -> Vec<BatchIdConflict> {
    let mut present: std::collections::HashSet<String> =
        live.iter().cloned().chain(terminal.keys().cloned()).collect();
    let mut conflicts = Vec::new();
    for op in operations {
        match op {
            streamkit_api::BatchOperation::AddNode { node_id, .. }
                if !present.insert(node_id.clone()) =>
            {
                let message = if in_flight.contains(node_id) {
                    format!("Batch rejected: node '{node_id}' is already being added")
                } else if live.contains(node_id) {
                    format!("Batch rejected: node '{node_id}' already exists in the pipeline")
                } else if let Some(label) = terminal.get(node_id) {
                    format!(
                        "Batch rejected: node '{node_id}' is still present in the engine in a \
                         {label} state; remove it before re-adding"
                    )
                } else {
                    format!(
                        "Batch rejected: node '{node_id}' is added more than once in this batch"
                    )
                };
                conflicts.push(BatchIdConflict { node_id: node_id.clone(), message });
            },
            streamkit_api::BatchOperation::RemoveNode { node_id } => {
                present.remove(node_id.as_str());
            },
            _ => {},
        }
    }
    conflicts
}

/// Whether a batch contains any `AddNode` op.  Used to skip the engine
/// terminal-residue query for pure connect/disconnect/remove batches, where
/// residue can never collide.
fn batch_has_add(operations: &[streamkit_api::BatchOperation]) -> bool {
    operations.iter().any(|op| matches!(op, streamkit_api::BatchOperation::AddNode { .. }))
}

/// Ids the session already treats as occupied, as `(live, in_flight)`: `live`
/// is confirmed nodes in the durable snapshot plus accepted-but-not-yet-
/// confirmed reservations, `in_flight` is just the reservations.  Splitting
/// them lets a collision be worded precisely ("already exists" vs "already
/// being added").  Callers hold the joint pipeline+creating lock so the two
/// views are consistent.
fn occupied_node_ids(
    pipeline: &streamkit_api::Pipeline,
    creating: &std::collections::HashSet<String>,
) -> (std::collections::HashSet<String>, std::collections::HashSet<String>) {
    let in_flight: std::collections::HashSet<String> = creating.iter().cloned().collect();
    let live = pipeline.nodes.keys().cloned().chain(in_flight.iter().cloned()).collect();
    (live, in_flight)
}

/// Check batch operations for duplicate node IDs by simulating the
/// Add/Remove sequence.  Returns the IDs of nodes that would collide.
#[cfg(test)]
pub(super) async fn check_batch_node_id_uniqueness(
    session: &crate::session::Session,
    operations: &[streamkit_api::BatchOperation],
) -> Vec<String> {
    let (live, in_flight) = {
        let pipeline = session.pipeline.lock().await;
        let creating = session.creating_nodes.lock().await;
        occupied_node_ids(&pipeline, &creating)
    };
    let terminal = if batch_has_add(operations) {
        engine_terminal_node_ids(session).await.unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    batch_id_conflicts(&live, &in_flight, &terminal, operations)
        .into_iter()
        .map(|c| c.node_id)
        .collect()
}

pub async fn validate_batch_operations(
    session: &crate::session::Session,
    operations: &[streamkit_api::BatchOperation],
    perms: &crate::permissions::Permissions,
    security_config: &crate::config::SecurityConfig,
    asset_root: &std::path::Path,
) -> Vec<streamkit_api::ValidationError> {
    let mut errors: Vec<streamkit_api::ValidationError> = Vec::new();

    let (live, in_flight) = {
        let pipeline = session.pipeline.lock().await;
        let creating = session.creating_nodes.lock().await;
        occupied_node_ids(&pipeline, &creating)
    };
    let terminal = if batch_has_add(operations) {
        match engine_terminal_node_ids(session).await {
            Ok(terminal) => terminal,
            Err(e) => {
                errors.push(streamkit_api::ValidationError {
                    error_type: streamkit_api::ValidationErrorType::Error,
                    message: format!("Cannot verify node availability against the engine: {e}"),
                    node_id: None,
                    connection_id: None,
                });
                std::collections::HashMap::new()
            },
        }
    } else {
        std::collections::HashMap::new()
    };
    for conflict in batch_id_conflicts(&live, &in_flight, &terminal, operations) {
        errors.push(streamkit_api::ValidationError {
            error_type: streamkit_api::ValidationErrorType::Error,
            message: conflict.message,
            node_id: Some(conflict.node_id),
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
                asset_root,
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
/// `AddNode` uses confirmed-add semantics, matching the single-node WS path
/// (`handle_add_node`): the id is reserved in the session's in-flight set and
/// the engine op is queued, but `pipeline.nodes` is left untouched here.  The
/// durable snapshot is reconciled by the session's `node-added` forwarder once
/// the engine reports the node was constructed and initialized, so a creation
/// that fails after validation leaves no orphan entry behind.  The reservation
/// closes the dispatch→confirmation gap so a concurrent add of the same id is
/// rejected instead of being silently dropped by the engine's duplicate guard.
/// The uniqueness check also consults the engine's terminal-node residue
/// (`Failed`/`Stopped` nodes it keeps in `node_states` until an explicit
/// `RemoveNode`), so re-adding such an id is rejected with guidance to remove
/// it first rather than being silently swallowed by that same guard.
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
    asset_root: &std::path::Path,
) -> Result<(), String> {
    let mut engine_operations = Vec::new();
    {
        // Hold both guards across the conflict check and the reservation
        // below.  Lock order matches `reserve_node_id` (pipeline, then
        // creating_nodes).  Without the joint hold a concurrent add/batch
        // could pass its own check and reach the engine for the same id,
        // where the actor's duplicate guard would silently drop it with no
        // observable signal to the client.
        let mut pipeline = session.pipeline.lock().await;
        let mut creating = session.creating_nodes.lock().await;

        let (live, in_flight) = occupied_node_ids(&pipeline, &creating);
        // Consult the engine's terminal-node residue *inside* the joint lock
        // (not from a pre-lock snapshot): a node going `Creating`→`Failed`
        // could otherwise slip between the snapshot and the reservation and
        // be silently swallowed by the actor's duplicate guard.
        // `broadcast_state_update` writes `node_states` before sending the
        // state update that drains `creating_nodes`, and no other creation
        // for these ids can be in flight while we hold the lock, so the
        // engine view is authoritative here.  Fail closed if it can't be
        // queried.
        let terminal = if batch_has_add(&operations) {
            engine_terminal_node_ids(session).await.map_err(|e| {
                format!("Batch rejected: cannot verify node availability against the engine: {e}")
            })?
        } else {
            std::collections::HashMap::new()
        };
        if let Some(conflict) =
            batch_id_conflicts(&live, &in_flight, &terminal, &operations).into_iter().next()
        {
            return Err(conflict.message);
        }

        // Validate node kinds only after the conflict check so a batch that
        // is both a duplicate and a forbidden kind reports the duplicate
        // first, matching the pre-confirmed-add ordering.
        for op in &operations {
            if let streamkit_api::BatchOperation::AddNode { kind, params, .. } = op {
                if let Some(message) = crate::websocket_handlers::validate_add_node_op(
                    kind,
                    params.as_ref(),
                    perms,
                    security_config,
                    asset_root,
                ) {
                    return Err(message);
                }
            }
        }

        for op in operations {
            match op {
                streamkit_api::BatchOperation::AddNode { node_id, kind, params } => {
                    // Confirmed-add: reserve the id in the in-flight set
                    // (mirroring handle_add_node) but leave pipeline.nodes
                    // untouched until the engine confirms via the node-added
                    // forwarder.
                    creating.insert(node_id.clone());
                    engine_operations.push(
                        streamkit_core::control::EngineControlMessage::AddNode {
                            node_id,
                            kind,
                            params,
                        },
                    );
                },
                streamkit_api::BatchOperation::RemoveNode { node_id } => {
                    // No optimistic `shift_remove` of the node: pruning
                    // `pipeline.nodes` is the engine-driven node-lifecycle
                    // forwarder's job, so a queued confirmed-add can never
                    // re-insert a node the engine has torn down (#607).
                    //
                    // Incident connections ARE pruned synchronously here (and
                    // announced via `ConnectionRemoved`) so they stay in-order
                    // with this batch's own disconnect/reconnect — a node
                    // replacement disconnects before this op, so nothing
                    // survives to be wrongly dropped.
                    session.prune_incident_connections(&mut pipeline, &node_id);
                    // Release any in-flight reservation: the engine's
                    // cancel-while-Creating path emits no terminal state, so
                    // without this an id removed mid-creation would stay
                    // wedged in the in-flight set.  Mirrors handle_remove_node.
                    creating.remove(&node_id);
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
        drop(creating);
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
    asset_root: &std::path::Path,
    event_tx: &tokio::sync::broadcast::Sender<crate::state::BroadcastEvent>,
) -> Result<(), String> {
    tune_session_node_inner(session, node_id, message, security_config, asset_root, event_tx, false)
        .await
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
    asset_root: &std::path::Path,
    event_tx: &tokio::sync::broadcast::Sender<crate::state::BroadcastEvent>,
) -> Result<(), String> {
    tune_session_node_inner(session, node_id, message, security_config, asset_root, event_tx, true)
        .await
}

pub(super) async fn tune_session_node_inner(
    session: &crate::session::Session,
    node_id: String,
    message: streamkit_core::control::NodeControlMessage,
    security_config: &crate::config::SecurityConfig,
    asset_root: &std::path::Path,
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
            asset_root,
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
    use streamkit_core::state::NodeState;
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
        // Generous buffer so a burst of engine state/stats events cannot
        // lag a slow test reader into dropping the one transition it waits
        // for.
        let (tx, rx) = broadcast::channel(1024);
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

    /// Poll `pipeline.nodes` until `node_id` is confirmed by the engine or
    /// the deadline elapses.  `AddNode` is confirmed-add: the durable
    /// snapshot is reconciled by the session's node-added forwarder once the
    /// engine reports success, not synchronously inside
    /// `apply_batch_operations`.  This is the unit-level analogue of the
    /// integration tests' `wait_for_node_added`.
    async fn wait_for_node(session: &Session, node_id: &str) {
        crate::session::test_support::wait_until(
            || async { session.pipeline.lock().await.nodes.contains_key(node_id) },
            &format!("node '{node_id}' was never confirmed into pipeline.nodes"),
        )
        .await;
    }

    /// Poll `pipeline.nodes` until `node_id` is gone or the deadline elapses.
    /// `RemoveNode` is engine-confirmed: the snapshot prune (and incident
    /// connection pruning) is reconciled by the node-lifecycle forwarder once
    /// the engine tears the node down, not synchronously inside
    /// `apply_batch_operations` (#607).
    async fn wait_for_node_removed(session: &Session, node_id: &str) {
        crate::session::test_support::wait_until(
            || async { !session.pipeline.lock().await.nodes.contains_key(node_id) },
            &format!("node '{node_id}' was never pruned from pipeline.nodes"),
        )
        .await;
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
    async fn populate_session_pipeline_preserves_attributes() {
        let (session, _rx) = fresh_session().await;

        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service".to_string(), "tts".to_string());
        let engine_pipeline =
            streamkit_api::Pipeline { attributes: Some(attributes.clone()), ..Default::default() };

        populate_session_pipeline(&session, &engine_pipeline).await;

        let pipeline = session.pipeline.lock().await;
        assert_eq!(
            pipeline.attributes.as_ref(),
            Some(&attributes),
            "session snapshot must round-trip the submitted attributes"
        );
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
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
            std::path::Path::new("."),
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
            std::path::Path::new("."),
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

        let errors = validate_batch_operations(
            &session,
            &ops,
            &perms,
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await;

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
            std::path::Path::new("."),
        )
        .await;

        let dup_errs: Vec<_> =
            errors.iter().filter(|e| e.node_id.as_deref() == Some("dup")).collect();
        assert!(!dup_errs.is_empty(), "expected a duplicate-id error for 'dup', got: {errors:?}");
        assert!(matches!(dup_errs[0].error_type, ValidationErrorType::Error));
        // 'dup' exists nowhere but the batch; the message must say so rather
        // than falsely claim it already exists in the pipeline.
        assert!(
            dup_errs[0].message.contains("added more than once in this batch"),
            "expected a batch-scoped duplicate message, got: {}",
            dup_errs[0].message,
        );
        assert!(
            !dup_errs[0].message.contains("already exists in the pipeline"),
            "must not claim a brand-new id pre-exists, got: {}",
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
            std::path::Path::new("."),
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
            std::path::Path::new("."),
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
            std::path::Path::new("."),
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
    async fn apply_happy_addnode_confirms_into_pipeline_and_records_params() {
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
            std::path::Path::new("."),
        )
        .await
        .expect("happy AddNode must succeed");

        // Confirmed-add: the node lands in the durable snapshot only after
        // the engine reports success via the node-added forwarder.
        wait_for_node(&session, "new").await;

        let pipeline = session.pipeline.lock().await;
        let node =
            pipeline.nodes.get("new").expect("'new' should be confirmed into pipeline.nodes");
        assert_eq!(node.kind, "core::passthrough");
        assert_eq!(node.params, Some(serde_json::json!({ "k": "v" })));
        drop(pipeline);
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_addnode_engine_failure_leaves_no_orphan() {
        let (session, mut rx) = fresh_session().await;

        // `core::definitely_not_a_real_kind` passes validation (the kind
        // registry is not consulted there) but the engine has no entry for
        // it, so creation fails after the batch is accepted.  Under the old
        // optimistic semantics this left an orphan in `pipeline.nodes`;
        // confirmed-add must leave nothing behind.
        let ops = vec![add_op("ghost", "core::definitely_not_a_real_kind")];
        apply_batch_operations(
            &session,
            ops,
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch is accepted; the engine-side creation is what fails");

        // Wait for the engine's `Failed` state transition for this id so the
        // assertion ties to the engine's "creation failed" signal rather
        // than an arbitrary sleep.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut saw_failed = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(BroadcastEvent { event, .. })) => match &event.payload {
                    EventPayload::NodeStateChanged { node_id, state, .. }
                        if node_id == "ghost"
                            && matches!(state, streamkit_api::NodeState::Failed { .. }) =>
                    {
                        saw_failed = true;
                        break;
                    },
                    EventPayload::NodeAdded { node_id, .. } => {
                        assert_ne!(node_id, "ghost", "failed creation must not emit NodeAdded");
                    },
                    _ => {},
                },
                // Lagged would mean the buffer overflowed and an event was
                // dropped; surface it loudly rather than masking it as "no
                // Failed event seen" (the channel is sized to make this
                // impossible for this test).
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    panic!("event receiver lagged by {n}; increase the test channel capacity");
                },
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => {},
            }
        }
        assert!(saw_failed, "expected a Failed state transition for 'ghost'");

        assert!(
            !session.pipeline.lock().await.nodes.contains_key("ghost"),
            "a failed AddNode must not leave an orphan in pipeline.nodes",
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_batch_addnode_reserves_id_blocking_duplicate() {
        let (session, _rx) = fresh_session().await;

        // Confirmed-add leaves pipeline.nodes empty until the engine
        // confirms, so the reservation in creating_nodes is what makes a
        // second add for the same id collide instead of reaching the engine
        // to be silently dropped by its duplicate guard.  Seed the
        // reservation directly so the collision is deterministically against
        // an in-flight id (a real first add races the node-added forwarder,
        // which could confirm it into pipeline.nodes first).
        session.creating_nodes.lock().await.insert("x".to_string());

        let result = apply_batch_operations(
            &session,
            vec![add_op("x", "core::passthrough")],
            &passthrough_only_perms(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await;

        let err = result.expect_err("a duplicate in-flight id must be rejected");
        // An in-flight reservation is not yet in pipeline.nodes, so the
        // message must reflect "being added", matching the WS path's
        // `reserve_node_id`, rather than claiming it already exists.
        assert!(
            err.contains("is already being added"),
            "in-flight collision must say 'is already being added', got: {err}",
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_batch_removenode_releases_inflight_reservation() {
        let (session, _rx) = fresh_session().await;

        // Mimic an id still in flight (reserved, not yet engine-confirmed).
        // The engine's cancel-while-Creating path emits no terminal state,
        // so a batch RemoveNode must drain the reservation itself — else the
        // id stays wedged and can never be re-added.
        session.creating_nodes.lock().await.insert("inflight".to_string());

        apply_batch_operations(
            &session,
            vec![remove_op("inflight")],
            &passthrough_only_perms(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("RemoveNode must succeed");

        assert!(
            !session.creating_nodes.lock().await.contains("inflight"),
            "batch RemoveNode must release the in-flight reservation",
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_batch_failed_addnode_prunes_incident_connection() {
        let (session, _rx) = fresh_session().await;

        // "ghost" passes validation but has no engine factory, so creation
        // fails after the batch is accepted.  The batch records the
        // ghost->sink edge synchronously; once ghost fails, the state
        // forwarder must prune that now-dangling connection.
        let ops = vec![
            add_op("ghost", "core::definitely_not_a_real_kind"),
            add_op("sink", "core::passthrough"),
            connect_op(("ghost", "out"), ("sink", "in")),
        ];
        apply_batch_operations(
            &session,
            ops,
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch is accepted; ghost's creation is what fails");

        // The edge is recorded synchronously, before any async engine work.
        assert!(
            session.pipeline.lock().await.connections.iter().any(|c| c.from_node == "ghost"),
            "the batch should record the ghost->sink connection synchronously",
        );

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let dangling = {
                let pipeline = session.pipeline.lock().await;
                pipeline.connections.iter().any(|c| c.from_node == "ghost" || c.to_node == "ghost")
            };
            if !dangling {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "dangling connection to failed 'ghost' was never pruned",
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let _ = session.shutdown_and_wait().await;
    }

    /// Poll the engine's `node_states` until `node_id` is terminal
    /// (`Failed`/`Stopped`) or the deadline elapses.  The engine retains a
    /// terminal node here until an explicit `RemoveNode`.
    async fn wait_for_terminal(session: &Session, node_id: &str) {
        crate::session::test_support::wait_until(
            || async {
                matches!(
                    session.get_node_states().await.unwrap_or_default().get(node_id),
                    Some(NodeState::Failed { .. } | NodeState::Stopped { .. })
                )
            },
            &format!("node '{node_id}' never reached a terminal state"),
        )
        .await;
    }

    #[tokio::test]
    async fn apply_batch_readd_of_failed_node_is_rejected() {
        let (session, _rx) = fresh_session().await;

        // First add fails engine construction (no factory for this kind),
        // so 'ghost' survives only in the engine's node_states as Failed —
        // dropped from pipeline.nodes and creating_nodes.
        apply_batch_operations(
            &session,
            vec![add_op("ghost", "core::definitely_not_a_real_kind")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch is accepted; ghost's creation is what fails");
        wait_for_terminal(&session, "ghost").await;

        // Re-adding the same id without removing it first must be rejected
        // loudly, not silently swallowed by the engine's duplicate guard.
        let err = apply_batch_operations(
            &session,
            vec![add_op("ghost", "core::passthrough")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("re-adding a failed id must be rejected");
        assert!(err.contains("remove it before re-adding"), "unexpected error: {err}");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_batch_remove_then_readd_of_failed_node_succeeds() {
        let (session, _rx) = fresh_session().await;

        apply_batch_operations(
            &session,
            vec![add_op("ghost", "core::definitely_not_a_real_kind")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch is accepted; ghost's creation is what fails");
        wait_for_terminal(&session, "ghost").await;

        // RemoveNode clears the engine's terminal residue, so a same-batch
        // remove-then-add of the id is accepted.
        apply_batch_operations(
            &session,
            vec![remove_op("ghost"), add_op("ghost", "core::passthrough")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("remove-then-readd of a failed id must be accepted");
        wait_for_node(&session, "ghost").await;
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn reserve_node_id_rejects_readd_of_failed_node() {
        let (session, _rx) = fresh_session().await;

        apply_batch_operations(
            &session,
            vec![add_op("ghost", "core::definitely_not_a_real_kind")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch is accepted; ghost's creation is what fails");
        wait_for_terminal(&session, "ghost").await;

        // The single-node WS reservation path consults the same engine
        // residue as the batch path, for parity.
        let err = session
            .reserve_node_id("ghost")
            .await
            .expect_err("re-adding a failed id must be rejected");
        assert!(err.contains("remove it before re-adding"), "unexpected error: {err}");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn removenode_emits_node_removed_for_failed_residue() {
        let (session, mut rx) = fresh_session().await;

        apply_batch_operations(
            &session,
            vec![add_op("ghost", "core::definitely_not_a_real_kind")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("batch accepted; ghost's creation is what fails");
        wait_for_terminal(&session, "ghost").await;

        // A node that failed during creation never entered pipeline.nodes, but
        // clearing its engine residue must still broadcast NodeRemoved so a
        // client tracking the failed node gets a clear event (#607 follow-up).
        apply_batch_operations(
            &session,
            vec![remove_op("ghost")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("RemoveNode must succeed");
        wait_for_engine_removed(&session, "ghost").await;

        let mut saw_node_removed = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && !saw_node_removed {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(BroadcastEvent { event, .. })) => {
                    if matches!(
                        &event.payload,
                        EventPayload::NodeRemoved { node_id, .. } if node_id == "ghost"
                    ) {
                        saw_node_removed = true;
                    }
                },
                _ => break,
            }
        }
        assert!(saw_node_removed, "expected a NodeRemoved event for failed residue 'ghost'");
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
            std::path::Path::new("."),
        )
        .await
        .expect("connect after AddNode must succeed");

        wait_for_node(&session, "src").await;
        wait_for_node(&session, "dst").await;

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
            std::path::Path::new("."),
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
            std::path::Path::new("."),
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
            std::path::Path::new("."),
        )
        .await
        .expect("setup batch must succeed");
        assert_eq!(session.pipeline.lock().await.connections.len(), 2);

        // Confirmed-add: wait for the engine to confirm all three nodes
        // before removing one.
        wait_for_node(&session, "a").await;
        wait_for_node(&session, "b").await;
        wait_for_node(&session, "c").await;

        let ops = vec![remove_op("b")];
        apply_batch_operations(
            &session,
            ops,
            &passthrough_only_perms(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("RemoveNode must succeed");

        // RemoveNode is engine-confirmed (#607): the node-lifecycle forwarder
        // prunes 'b' and its two incident edges once the engine tears it down.
        wait_for_node_removed(&session, "b").await;

        let pipeline = session.pipeline.lock().await;
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

    /// Poll the engine's `node_states` until `node_id` is absent (the engine
    /// has torn it down and cleared its residue) or the deadline elapses.
    async fn wait_for_engine_removed(session: &Session, node_id: &str) {
        crate::session::test_support::wait_until(
            || async { !session.get_node_states().await.unwrap_or_default().contains_key(node_id) },
            &format!("engine never tore down node '{node_id}'"),
        )
        .await;
    }

    #[tokio::test]
    async fn add_then_remove_cross_op_leaves_no_durable_orphan() {
        let (session, _rx) = fresh_session().await;

        // Add 'b' then immediately remove it WITHOUT waiting for the engine to
        // confirm the add — the exact cross-op timing from #607. The engine
        // emits Added then Removed on one ordered stream, so the forwarder
        // inserts 'b' and then prunes it; a queued confirmed-add can never
        // re-insert a node the engine has already torn down.
        apply_batch_operations(
            &session,
            vec![add_op("b", "core::passthrough")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("add batch must succeed");
        apply_batch_operations(
            &session,
            vec![remove_op("b")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("remove batch must succeed");

        // Wait until the engine has fully processed both ops, then for the
        // ordered forwarder to settle on the terminal (Removed) state.
        wait_for_engine_removed(&session, "b").await;
        wait_for_node_removed(&session, "b").await;

        // Removed is the last lifecycle event for 'b', so once applied the
        // snapshot stays clean — assert it does not get re-inserted.
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            assert!(
                !session.pipeline.lock().await.nodes.contains_key("b"),
                "a queued confirmed-add must not re-insert a torn-down node",
            );
        }
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn removenode_emits_connection_removed_for_incident_edges() {
        let (session, mut rx) = fresh_session().await;
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
            std::path::Path::new("."),
        )
        .await
        .expect("setup batch must succeed");
        wait_for_node(&session, "a").await;
        wait_for_node(&session, "b").await;

        apply_batch_operations(
            &session,
            vec![remove_op("b")],
            &passthrough_only_perms(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("RemoveNode must succeed");
        wait_for_node_removed(&session, "b").await;

        // The incident edge's granular ConnectionRemoved (emitted synchronously
        // by the RemoveNode handler) must arrive before the engine-driven
        // NodeRemoved for 'b' (#607).
        let mut saw_connection_removed = false;
        let mut saw_node_removed = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline
            && !(saw_connection_removed && saw_node_removed)
        {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(BroadcastEvent { event, .. })) => match &event.payload {
                    EventPayload::ConnectionRemoved {
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                        ..
                    } if from_node == "a"
                        && from_pin == "out"
                        && to_node == "b"
                        && to_pin == "in" =>
                    {
                        saw_connection_removed = true;
                    },
                    EventPayload::NodeRemoved { node_id, .. } if node_id == "b" => {
                        assert!(
                            saw_connection_removed,
                            "NodeRemoved for 'b' arrived before its incident ConnectionRemoved",
                        );
                        saw_node_removed = true;
                    },
                    _ => {},
                },
                _ => break,
            }
        }
        assert!(saw_connection_removed, "expected a ConnectionRemoved event for the a->b edge");
        assert!(saw_node_removed, "expected a NodeRemoved event for 'b'");
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_reports_duplicate_before_forbidden_kind() {
        let (session, _rx) = fresh_session().await;
        preinsert_node(&session, "dup", "core::passthrough").await;

        // 'dup' is both a live duplicate and (as core::sink under
        // passthrough-only perms) a forbidden kind; the duplicate must win.
        let err = apply_batch_operations(
            &session,
            vec![add_op("dup", "core::sink")],
            &passthrough_only_perms(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("a duplicate id must be rejected");

        assert!(
            err.contains("already exists in the pipeline"),
            "duplicate must be reported before the forbidden-kind error, got: {err}",
        );
        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn apply_batch_add_fails_closed_when_engine_unavailable() {
        let (session, _rx) = fresh_session().await;
        session.shutdown_and_wait().await.expect("shutdown should succeed");

        // With the engine gone the terminal-residue guard can't be
        // consulted; an add-bearing batch must be rejected rather than
        // silently skip the check and dispatch to a dead engine.
        let err = apply_batch_operations(
            &session,
            vec![add_op("x", "core::passthrough")],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect_err("add batch must fail closed when engine state can't be queried");

        assert!(
            err.contains("verify node availability"),
            "expected a fail-closed error, got: {err}",
        );
    }

    #[tokio::test]
    async fn apply_non_add_batch_succeeds_without_engine_query() {
        let (session, _rx) = fresh_session().await;
        session.shutdown_and_wait().await.expect("shutdown should succeed");

        // A pure disconnect batch references no AddNode, so it must not
        // depend on the engine residue query (which would now fail) and
        // should apply without rejection.
        apply_batch_operations(
            &session,
            vec![disconnect_op(("a", "out"), ("b", "in"))],
            &Permissions::admin(),
            &SecurityConfig::default(),
            std::path::Path::new("."),
        )
        .await
        .expect("non-add batch must not require an engine state query");
    }
}
