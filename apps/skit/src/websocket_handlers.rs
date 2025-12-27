// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! WebSocket API request handlers.
//!
//! This module contains handler functions for each API request type, extracted from
//! the main `handle_api_request` function.

use crate::file_security;
use crate::permissions::Permissions;
use crate::session::Session;
use crate::state::AppState;
use streamkit_api::{
    Event as ApiEvent, EventPayload, MessageType, RequestPayload, ResponsePayload,
};
use streamkit_core::control::{EngineControlMessage, NodeControlMessage};
use streamkit_core::registry::NodeDefinition;
use streamkit_core::types::PacketType;
use streamkit_core::{InputPin, OutputPin, PinCardinality};
use tracing::{debug, error, info, warn};

/// Check if the user has access to modify/destroy a session.
///
/// Returns true if:
/// - The user has `access_all_sessions` permission, OR
/// - The session was created by the same user/role
fn can_access_session(session: &Session, role_name: &str, perms: &Permissions) -> bool {
    if perms.access_all_sessions {
        return true;
    }
    // Allow access if session was created by this role, or has no creator (legacy sessions)
    session.created_by.as_ref().is_none_or(|creator| creator == role_name)
}

pub async fn handle_request_payload(
    payload: RequestPayload,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
    correlation_id: Option<String>,
) -> Option<ResponsePayload> {
    match payload {
        RequestPayload::CreateSession { name } => {
            handle_create_session(name, app_state, perms, role_name, correlation_id).await
        },
        RequestPayload::DestroySession { session_id } => {
            handle_destroy_session(session_id, app_state, perms, role_name, correlation_id).await
        },
        RequestPayload::ListSessions => handle_list_sessions(app_state, perms, role_name).await,
        RequestPayload::ListNodes => Some(handle_list_nodes(app_state, perms)),
        RequestPayload::AddNode { session_id, node_id, kind, params } => {
            handle_add_node(session_id, node_id, kind, params, app_state, perms, role_name).await
        },
        RequestPayload::RemoveNode { session_id, node_id } => {
            handle_remove_node(session_id, node_id, app_state, perms, role_name).await
        },
        RequestPayload::Connect { session_id, from_node, from_pin, to_node, to_pin, mode } => {
            handle_connect(
                session_id, from_node, from_pin, to_node, to_pin, mode, app_state, perms, role_name,
            )
            .await
        },
        RequestPayload::Disconnect { session_id, from_node, from_pin, to_node, to_pin } => {
            handle_disconnect(
                session_id, from_node, from_pin, to_node, to_pin, app_state, perms, role_name,
            )
            .await
        },
        RequestPayload::TuneNode { session_id, node_id, message } => {
            handle_tune_node(session_id, node_id, message, app_state, perms, role_name).await
        },
        RequestPayload::TuneNodeAsync { session_id, node_id, message } => {
            handle_tune_node_async(session_id, node_id, message, app_state, perms, role_name).await
        },
        RequestPayload::GetPipeline { session_id } => {
            handle_get_pipeline(session_id, app_state, perms, role_name).await
        },
        RequestPayload::ValidateBatch { session_id: _, operations } => {
            Some(handle_validate_batch(&operations, app_state, perms))
        },
        RequestPayload::ApplyBatch { session_id, operations } => {
            handle_apply_batch(session_id, operations, app_state, perms, role_name).await
        },
        RequestPayload::GetPermissions => Some(handle_get_permissions(perms, role_name)),
    }
}

async fn handle_create_session(
    name: Option<String>,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
    _correlation_id: Option<String>,
) -> Option<ResponsePayload> {
    // Check permission
    if !perms.create_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot create sessions".to_string(),
        });
    }

    // Check global session limits + duplicate names with short lock hold.
    let (current_count, name_taken) = {
        let session_manager = app_state.session_manager.lock().await;
        let current_count = session_manager.session_count();
        let name_taken = name.as_deref().is_some_and(|n| session_manager.is_name_taken(n));
        drop(session_manager);
        (current_count, name_taken)
    };
    if let Some(ref session_name) = name {
        if name_taken {
            return Some(ResponsePayload::Error {
                message: format!("Session with name '{session_name}' already exists"),
            });
        }
    }
    if !app_state.config.permissions.can_accept_session(current_count) {
        return Some(ResponsePayload::Error {
            message: "Maximum concurrent sessions limit reached".to_string(),
        });
    }

    let session = match crate::session::Session::create(
        &app_state.engine,
        &app_state.config,
        name.clone(),
        app_state.event_tx.clone(),
        Some(role_name.to_string()),
    )
    .await
    {
        Ok(session) => session,
        Err(error_msg) => return Some(ResponsePayload::Error { message: error_msg }),
    };

    // Insert session with short lock hold, re-checking limits to avoid races.
    let insert_result = {
        let mut session_manager = app_state.session_manager.lock().await;
        let current_count = session_manager.session_count();
        if app_state.config.permissions.can_accept_session(current_count) {
            session_manager.add_session(session.clone())
        } else {
            Err("Maximum concurrent sessions limit reached".to_string())
        }
    };
    if let Err(error_msg) = insert_result {
        let _ = session.shutdown_and_wait().await;
        return Some(ResponsePayload::Error { message: error_msg });
    }

    info!(session_id = %session.id, name = ?session.name, "Created new session");

    // Broadcast event to all clients
    let created_at_str = crate::session::system_time_to_rfc3339(session.created_at);
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionCreated {
            session_id: session.id.clone(),
            name: session.name.clone(),
            created_at: created_at_str.clone(),
        },
    };
    if app_state.event_tx.send(event).is_err() {
        debug!("No WebSocket clients connected to receive SessionCreated event");
    }

    Some(ResponsePayload::SessionCreated {
        session_id: session.id,
        name: session.name,
        created_at: created_at_str,
    })
}

async fn handle_destroy_session(
    session_id: String,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
    _correlation_id: Option<String>,
) -> Option<ResponsePayload> {
    // Check permission
    if !perms.destroy_sessions {
        warn!(
            session_id = %session_id,
            destroy_sessions = perms.destroy_sessions,
            "Blocked attempt to destroy session: permission denied"
        );
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot destroy sessions".to_string(),
        });
    }

    let removed_session = {
        let mut session_manager = app_state.session_manager.lock().await;

        let Some(session) = session_manager.get_session_by_name_or_id(&session_id) else {
            return Some(ResponsePayload::Error {
                message: format!("Session '{session_id}' not found"),
            });
        };

        // Check ownership before destroying
        if !can_access_session(&session, role_name, perms) {
            warn!(
                session_id = %session_id,
                role = %role_name,
                "Blocked attempt to destroy session: not owner"
            );
            return Some(ResponsePayload::Error {
                message: "Permission denied: you do not own this session".to_string(),
            });
        }

        session_manager.remove_session_by_id(&session.id)
    };

    let Some(session) = removed_session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };
    let destroyed_id = session.id.clone();

    if let Err(e) = session.shutdown_and_wait().await {
        warn!(session_id = %destroyed_id, error = %e, "Error during engine shutdown");
    }

    info!(session_id = %destroyed_id, "Session destroyed successfully");

    // Broadcast event to all clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionDestroyed { session_id: destroyed_id.clone() },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast SessionDestroyed event: {}", e);
    }

    Some(ResponsePayload::SessionDestroyed { session_id: destroyed_id })
}

async fn handle_list_sessions(
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission
    if !perms.list_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot list sessions".to_string(),
        });
    }

    let sessions = app_state.session_manager.lock().await.list_sessions();

    // Filter sessions based on ownership and permissions
    let session_infos: Vec<streamkit_api::SessionInfo> = sessions
        .into_iter()
        .filter(|session| {
            // Admin with access_all_sessions can see all sessions
            if perms.access_all_sessions {
                return true;
            }
            // Otherwise, only see sessions you created
            session.created_by.as_ref().is_none_or(|creator| creator == role_name)
        })
        .map(|session| streamkit_api::SessionInfo {
            id: session.id,
            name: session.name,
            created_at: crate::session::system_time_to_rfc3339(session.created_at),
        })
        .collect();

    info!(
        role = %role_name,
        access_all = perms.access_all_sessions,
        filtered_sessions = session_infos.len(),
        "Listed sessions with filtering"
    );
    Some(ResponsePayload::SessionsListed { sessions: session_infos })
}

fn handle_list_nodes(app_state: &AppState, perms: &Permissions) -> ResponsePayload {
    // Check permission
    if !perms.list_nodes {
        return ResponsePayload::Error {
            message: "Permission denied: cannot list nodes".to_string(),
        };
    }

    let mut definitions = {
        let registry = match app_state.engine.registry.read() {
            Ok(reg) => reg,
            Err(e) => {
                error!("Engine registry poisoned: {}", e);
                return ResponsePayload::Error {
                    message: "Service temporarily unavailable".to_string(),
                };
            },
        };
        registry.definitions()
    };

    // Add synthetic node definitions for oneshot-only nodes
    // These are virtual markers that get replaced at runtime in stateless pipelines

    definitions.push(NodeDefinition {
        kind: "streamkit::http_input".to_string(),
        description: Some(
            "Synthetic input node for oneshot HTTP pipelines. \
             Receives binary data from the HTTP request body."
                .to_string(),
        ),
        param_schema: serde_json::json!({}),
        inputs: vec![],
        outputs: vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }],
        categories: vec!["transport".to_string(), "oneshot".to_string()],
        bidirectional: false,
    });

    definitions.push(NodeDefinition {
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
    });

    // Filter nodes based on allowed_nodes permission.
    // For plugin nodes, also enforce allowed_plugins so the UI doesn't advertise unusable kinds.
    definitions.retain(|def| {
        if !perms.is_node_allowed(&def.kind) {
            return false;
        }
        if def.kind.starts_with("plugin::") {
            return perms.is_plugin_allowed(&def.kind);
        }
        true
    });

    info!(
        "Listed {} available node definitions (including synthetic oneshot nodes)",
        definitions.len()
    );
    ResponsePayload::NodesListed { nodes: definitions }
}

#[allow(clippy::too_many_arguments)]
async fn handle_add_node(
    session_id: String,
    node_id: String,
    kind: String,
    params: Option<serde_json::Value>,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to modify sessions
    if !perms.modify_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        });
    }

    // Reject oneshot-only marker nodes on the dynamic control plane.
    if kind == "streamkit::http_input" || kind == "streamkit::http_output" {
        return Some(ResponsePayload::Error {
            message: format!(
                "Node type '{kind}' is oneshot-only and cannot be used in dynamic sessions"
            ),
        });
    }

    // Check if the node type is allowed
    if !perms.is_node_allowed(&kind) {
        return Some(ResponsePayload::Error {
            message: format!("Permission denied: node type '{kind}' not allowed"),
        });
    }

    // If this is a plugin node, enforce the plugin allowlist too.
    if kind.starts_with("plugin::") && !perms.is_plugin_allowed(&kind) {
        return Some(ResponsePayload::Error {
            message: format!("Permission denied: plugin '{kind}' not allowed"),
        });
    }

    // Security: validate file_reader paths on the control plane too (not just oneshot/HTTP).
    if kind == "core::file_reader" {
        let Some(path) =
            params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str)
        else {
            return Some(ResponsePayload::Error {
                message: "Invalid file_reader params: expected params.path to be a string"
                    .to_string(),
            });
        };
        if let Err(e) = file_security::validate_file_path(path, &app_state.config.security) {
            return Some(ResponsePayload::Error { message: format!("Invalid file path: {e}") });
        }
    }

    // Security: validate file_writer paths on the control plane too (avoid arbitrary file writes).
    if kind == "core::file_writer" {
        let Some(path) =
            params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str)
        else {
            return Some(ResponsePayload::Error {
                message: "Invalid file_writer params: expected params.path to be a string"
                    .to_string(),
            });
        };
        if let Err(e) = file_security::validate_write_path(path, &app_state.config.security) {
            return Some(ResponsePayload::Error { message: format!("Invalid write path: {e}") });
        }
    }

    // Security: validate script_path (if present) for core::script nodes.
    if kind == "core::script" {
        if let Some(path) =
            params.as_ref().and_then(|p| p.get("script_path")).and_then(serde_json::Value::as_str)
        {
            if !path.trim().is_empty() {
                if let Err(e) = file_security::validate_file_path(path, &app_state.config.security)
                {
                    return Some(ResponsePayload::Error {
                        message: format!("Invalid script_path: {e}"),
                    });
                }
            }
        }
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.nodes.insert(
            node_id.clone(),
            streamkit_api::Node { kind: kind.clone(), params: params.clone(), state: None },
        );
    } // Lock released here

    // Broadcast event to all clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::NodeAdded {
            session_id: session.id.clone(),
            node_id: node_id.clone(),
            kind: kind.clone(),
            params: params.clone(),
        },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast NodeAdded event: {}", e);
    }

    // Now safe to do async operations without holding session_manager lock
    let control_msg = EngineControlMessage::AddNode { node_id, kind, params };
    session.send_control_message(control_msg).await;
    Some(ResponsePayload::Success)
}

async fn handle_remove_node(
    session_id: String,
    node_id: String,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to modify sessions
    if !perms.modify_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.nodes.shift_remove(&node_id);
        pipeline.connections.retain(|conn| conn.from_node != node_id && conn.to_node != node_id);
    }

    // Broadcast event to all clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::NodeRemoved {
            session_id: session.id.clone(),
            node_id: node_id.clone(),
        },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast NodeRemoved event: {}", e);
    }

    // Now safe to do async operations without holding session_manager lock
    let control_msg = EngineControlMessage::RemoveNode { node_id };
    session.send_control_message(control_msg).await;
    Some(ResponsePayload::Success)
}

#[allow(clippy::too_many_arguments)]
async fn handle_connect(
    session_id: String,
    from_node: String,
    from_pin: String,
    to_node: String,
    to_pin: String,
    mode: streamkit_api::ConnectionMode,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to modify sessions
    if !perms.modify_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.connections.push(streamkit_api::Connection {
            from_node: from_node.clone(),
            from_pin: from_pin.clone(),
            to_node: to_node.clone(),
            to_pin: to_pin.clone(),
            mode,
        });
    }

    // Broadcast event to all clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::ConnectionAdded {
            session_id: session.id.clone(),
            from_node: from_node.clone(),
            from_pin: from_pin.clone(),
            to_node: to_node.clone(),
            to_pin: to_pin.clone(),
        },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast ConnectionAdded event: {}", e);
    }

    // Now safe to do async operations without holding session_manager lock
    // Convert API ConnectionMode to core ConnectionMode
    let core_mode = match mode {
        streamkit_api::ConnectionMode::Reliable => {
            streamkit_core::control::ConnectionMode::Reliable
        },
        streamkit_api::ConnectionMode::BestEffort => {
            streamkit_core::control::ConnectionMode::BestEffort
        },
    };
    let control_msg =
        EngineControlMessage::Connect { from_node, from_pin, to_node, to_pin, mode: core_mode };
    session.send_control_message(control_msg).await;
    Some(ResponsePayload::Success)
}

#[allow(clippy::too_many_arguments)]
async fn handle_disconnect(
    session_id: String,
    from_node: String,
    from_pin: String,
    to_node: String,
    to_pin: String,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to modify sessions
    if !perms.modify_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.connections.retain(|conn| {
            !(conn.from_node == from_node
                && conn.from_pin == from_pin
                && conn.to_node == to_node
                && conn.to_pin == to_pin)
        });
    }

    // Broadcast event to all clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::ConnectionRemoved {
            session_id: session.id.clone(),
            from_node: from_node.clone(),
            from_pin: from_pin.clone(),
            to_node: to_node.clone(),
            to_pin: to_pin.clone(),
        },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast ConnectionRemoved event: {}", e);
    }

    // Now safe to do async operations without holding session_manager lock
    let control_msg = EngineControlMessage::Disconnect { from_node, from_pin, to_node, to_pin };
    session.send_control_message(control_msg).await;
    Some(ResponsePayload::Success)
}

async fn handle_tune_node(
    session_id: String,
    node_id: String,
    message: NodeControlMessage,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to tune nodes
    if !perms.tune_nodes {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot tune nodes".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    // Handle UpdateParams specially for event broadcasting (and validate file paths)
    if let NodeControlMessage::UpdateParams(ref params) = message {
        let (kind, file_path, script_path) = {
            let pipeline = session.pipeline.lock().await;
            let kind = pipeline.nodes.get(&node_id).map(|n| n.kind.clone());
            let file_path =
                params.get("path").and_then(serde_json::Value::as_str).map(str::to_string);
            let script_path =
                params.get("script_path").and_then(serde_json::Value::as_str).map(str::to_string);
            drop(pipeline);
            (kind, file_path, script_path)
        };

        let file_path = file_path.as_deref();
        let script_path = script_path.as_deref();

        if kind.as_deref() == Some("core::file_reader") {
            let Some(path) = file_path else {
                return Some(ResponsePayload::Error {
                    message: "Invalid file_reader params: expected params.path to be a string"
                        .to_string(),
                });
            };
            if let Err(e) = file_security::validate_file_path(path, &app_state.config.security) {
                return Some(ResponsePayload::Error { message: format!("Invalid file path: {e}") });
            }
        }

        if kind.as_deref() == Some("core::file_writer") {
            if let Some(path) = file_path {
                if let Err(e) = file_security::validate_write_path(path, &app_state.config.security)
                {
                    return Some(ResponsePayload::Error {
                        message: format!("Invalid write path: {e}"),
                    });
                }
            }
        }

        if kind.as_deref() == Some("core::script") {
            if let Some(path) = script_path {
                if !path.trim().is_empty() {
                    if let Err(e) =
                        file_security::validate_file_path(path, &app_state.config.security)
                    {
                        return Some(ResponsePayload::Error {
                            message: format!("Invalid script_path: {e}"),
                        });
                    }
                }
            }
        }

        {
            let mut pipeline = session.pipeline.lock().await;
            if let Some(node) = pipeline.nodes.get_mut(&node_id) {
                node.params = Some(params.clone());
            } else {
                warn!(
                    node_id = %node_id,
                    "Attempted to tune params for non-existent node in pipeline model"
                );
            }
        } // Lock released here

        // Broadcast event to all clients
        let event = ApiEvent {
            message_type: MessageType::Event,
            correlation_id: None,
            payload: EventPayload::NodeParamsChanged {
                session_id: session.id.clone(),
                node_id: node_id.clone(),
                params: params.clone(),
            },
        };
        if let Err(e) = app_state.event_tx.send(event) {
            error!("Failed to broadcast NodeParamsChanged event: {}", e);
        }
    }

    // Now safe to do async operations without holding session_manager lock
    let control_msg = EngineControlMessage::TuneNode { node_id, message };
    session.send_control_message(control_msg).await;
    Some(ResponsePayload::Success)
}

/// Handle async node tuning (fire-and-forget).
///
/// Complexity (37/30) is due to: permission check, session lookup, conditional UpdateParams
/// handling with pipeline update + event broadcast, engine message sending.
#[allow(clippy::cognitive_complexity)]
async fn handle_tune_node_async(
    session_id: String,
    node_id: String,
    message: NodeControlMessage,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to tune nodes
    if !perms.tune_nodes {
        // For async operations, we don't send a response but we should still log
        warn!("Permission denied: attempted to tune node without permission via TuneNodeAsync");
        return None;
    }

    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    if let Some(session) = session {
        // Check ownership
        if !can_access_session(&session, role_name, perms) {
            warn!(
                session_id = %session_id,
                role = %role_name,
                "Permission denied: attempted to tune node in session not owned via TuneNodeAsync"
            );
            return None;
        }

        // Handle UpdateParams specially for pipeline model updates and event broadcasting
        if let NodeControlMessage::UpdateParams(ref params) = message {
            let (kind, file_path, script_path) = {
                let pipeline = session.pipeline.lock().await;
                let kind = pipeline.nodes.get(&node_id).map(|n| n.kind.clone());
                let file_path =
                    params.get("path").and_then(serde_json::Value::as_str).map(str::to_string);
                let script_path = params
                    .get("script_path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                drop(pipeline);
                (kind, file_path, script_path)
            };

            let file_path = file_path.as_deref();
            let script_path = script_path.as_deref();

            if kind.as_deref() == Some("core::file_reader") {
                let Some(path) = file_path else {
                    warn!("Invalid file_reader params: expected params.path to be a string");
                    return None;
                };
                if let Err(e) = file_security::validate_file_path(path, &app_state.config.security)
                {
                    warn!("Invalid file path: {e}");
                    return None;
                }
            }

            if kind.as_deref() == Some("core::file_writer") {
                if let Some(path) = file_path {
                    if let Err(e) =
                        file_security::validate_write_path(path, &app_state.config.security)
                    {
                        warn!("Invalid write path: {e}");
                        return None;
                    }
                }
            }

            if kind.as_deref() == Some("core::script") {
                if let Some(path) = script_path {
                    if !path.trim().is_empty() {
                        if let Err(e) =
                            file_security::validate_file_path(path, &app_state.config.security)
                        {
                            warn!("Invalid script_path: {e}");
                            return None;
                        }
                    }
                }
            }

            {
                let mut pipeline = session.pipeline.lock().await;
                if let Some(node) = pipeline.nodes.get_mut(&node_id) {
                    node.params = Some(params.clone());
                } else {
                    warn!(
                        node_id = %node_id,
                        "Attempted to tune params for non-existent node in pipeline model via TuneNodeAsync"
                    );
                }
            } // Lock released here

            // Broadcast event to all clients
            let event = ApiEvent {
                message_type: MessageType::Event,
                correlation_id: None,
                payload: EventPayload::NodeParamsChanged {
                    session_id: session.id.clone(),
                    node_id: node_id.clone(),
                    params: params.clone(),
                },
            };
            if let Err(e) = app_state.event_tx.send(event) {
                error!("Failed to broadcast NodeParamsChanged event: {}", e);
            }
        }

        let control_msg = EngineControlMessage::TuneNode { node_id, message };
        session.send_control_message(control_msg).await;
    } else {
        warn!("Could not tune non-existent session '{}' via TuneNodeAsync", session_id);
    }
    None // Do not send a response
}

async fn handle_get_pipeline(
    session_id: String,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission
    if !perms.list_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot view pipelines".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    let node_states = session.get_node_states().await.unwrap_or_default();

    // Clone pipeline (short lock hold) and add runtime state to nodes.
    let mut api_pipeline = {
        let pipeline = session.pipeline.lock().await;
        pipeline.clone()
    };
    for (id, node) in &mut api_pipeline.nodes {
        node.state = node_states.get(id).cloned();
    }

    info!(
        session_id = %session_id,
        node_count = api_pipeline.nodes.len(),
        connection_count = api_pipeline.connections.len(),
        "Retrieved pipeline"
    );

    Some(ResponsePayload::Pipeline { pipeline: api_pipeline })
}

fn handle_validate_batch(
    operations: &[streamkit_api::BatchOperation],
    app_state: &AppState,
    perms: &Permissions,
) -> ResponsePayload {
    // Validate that user has permission for modify_sessions
    if !perms.modify_sessions {
        return ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        };
    }

    // Basic validation: check that all referenced node types are allowed
    for op in operations {
        if let streamkit_api::BatchOperation::AddNode { kind, params, .. } = op {
            if !perms.is_node_allowed(kind) {
                return ResponsePayload::Error {
                    message: format!("Permission denied: node type '{kind}' not allowed"),
                };
            }

            if kind == "core::file_reader" {
                let path =
                    params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str);
                let Some(path) = path else {
                    return ResponsePayload::Error {
                        message: "Invalid file_reader params: expected params.path to be a string"
                            .to_string(),
                    };
                };
                if let Err(e) = file_security::validate_file_path(path, &app_state.config.security)
                {
                    return ResponsePayload::Error { message: format!("Invalid file path: {e}") };
                }
            }

            if kind == "core::file_writer" {
                let path =
                    params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str);
                let Some(path) = path else {
                    return ResponsePayload::Error {
                        message: "Invalid file_writer params: expected params.path to be a string"
                            .to_string(),
                    };
                };
                if let Err(e) = file_security::validate_write_path(path, &app_state.config.security)
                {
                    return ResponsePayload::Error { message: format!("Invalid write path: {e}") };
                }
            }

            if kind == "core::script" {
                if let Some(path) = params
                    .as_ref()
                    .and_then(|p| p.get("script_path"))
                    .and_then(serde_json::Value::as_str)
                {
                    if !path.trim().is_empty() {
                        if let Err(e) =
                            file_security::validate_file_path(path, &app_state.config.security)
                        {
                            return ResponsePayload::Error {
                                message: format!("Invalid script_path: {e}"),
                            };
                        }
                    }
                }
            }
        }
    }

    info!(operation_count = operations.len(), "Validated batch operations");
    ResponsePayload::ValidationResult { errors: Vec::new() }
}

#[allow(clippy::significant_drop_tightening)]
async fn handle_apply_batch(
    session_id: String,
    operations: Vec<streamkit_api::BatchOperation>,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ResponsePayload> {
    // Check permission to modify sessions
    if !perms.modify_sessions {
        return Some(ResponsePayload::Error {
            message: "Permission denied: cannot modify sessions".to_string(),
        });
    }

    // Get session with SHORT lock hold to avoid blocking other operations
    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    }; // Session manager lock released here

    let Some(session) = session else {
        return Some(ResponsePayload::Error {
            message: format!("Session '{session_id}' not found"),
        });
    };

    // Check ownership (session is cloned, doesn't need lock)
    if !can_access_session(&session, role_name, perms) {
        return Some(ResponsePayload::Error {
            message: "Permission denied: you do not own this session".to_string(),
        });
    }

    // Validate permissions for all operations
    for op in &operations {
        if let streamkit_api::BatchOperation::AddNode { kind, params, .. } = op {
            if !perms.is_node_allowed(kind) {
                return Some(ResponsePayload::Error {
                    message: format!("Permission denied: node type '{kind}' not allowed"),
                });
            }

            if kind == "core::file_reader" {
                let path =
                    params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str);
                let Some(path) = path else {
                    return Some(ResponsePayload::Error {
                        message: "Invalid file_reader params: expected params.path to be a string"
                            .to_string(),
                    });
                };
                if let Err(e) = file_security::validate_file_path(path, &app_state.config.security)
                {
                    return Some(ResponsePayload::Error {
                        message: format!("Invalid file path: {e}"),
                    });
                }
            }

            if kind == "core::file_writer" {
                let path =
                    params.as_ref().and_then(|p| p.get("path")).and_then(serde_json::Value::as_str);
                let Some(path) = path else {
                    return Some(ResponsePayload::Error {
                        message: "Invalid file_writer params: expected params.path to be a string"
                            .to_string(),
                    });
                };
                if let Err(e) = file_security::validate_write_path(path, &app_state.config.security)
                {
                    return Some(ResponsePayload::Error {
                        message: format!("Invalid write path: {e}"),
                    });
                }
            }

            if kind == "core::script" {
                if let Some(path) = params
                    .as_ref()
                    .and_then(|p| p.get("script_path"))
                    .and_then(serde_json::Value::as_str)
                {
                    if !path.trim().is_empty() {
                        if let Err(e) =
                            file_security::validate_file_path(path, &app_state.config.security)
                        {
                            return Some(ResponsePayload::Error {
                                message: format!("Invalid script_path: {e}"),
                            });
                        }
                    }
                }
            }
        }
    }

    // Apply all operations in order
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
                    engine_operations.push(EngineControlMessage::AddNode { node_id, kind, params });
                },
                streamkit_api::BatchOperation::RemoveNode { node_id } => {
                    pipeline.nodes.shift_remove(&node_id);
                    pipeline
                        .connections
                        .retain(|conn| conn.from_node != node_id && conn.to_node != node_id);
                    engine_operations.push(EngineControlMessage::RemoveNode { node_id });
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
                    engine_operations.push(EngineControlMessage::Connect {
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                        mode: core_mode,
                    });
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
                    engine_operations.push(EngineControlMessage::Disconnect {
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                    });
                },
            }
        }
        drop(pipeline);
    } // Release pipeline lock

    // Now safe to do async operations without holding session_manager lock
    for msg in engine_operations {
        session.send_control_message(msg).await;
    }

    info!(
        session_id = %session_id,
        "Applied batch operations successfully"
    );

    Some(ResponsePayload::BatchApplied { success: true, errors: Vec::new() })
}

fn handle_get_permissions(perms: &Permissions, role_name: &str) -> ResponsePayload {
    info!(role = %role_name, "Returning permissions for role");
    ResponsePayload::Permissions { role: role_name.to_string(), permissions: perms.to_info() }
}
