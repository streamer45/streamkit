// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::extract::ws::WebSocket;
use opentelemetry::{global, KeyValue};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::broadcast::error::RecvError;
use tracing::{error, info, warn};

use streamkit_api::{
    EventPayload, MessageType, Request as ApiRequest, Response as ApiResponse, ResponsePayload,
};

use crate::permissions::Permissions;
use crate::state::AppState;

static ACTIVE_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
const DEFAULT_MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024; // 1 MiB

fn max_ws_message_bytes() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("SK_WEBSOCKET_MAX_MESSAGE_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_WS_MESSAGE_BYTES)
    })
}

/// Helper function to send a JSON message over WebSocket with consistent error handling.
/// Returns `Ok(())` if the message was sent successfully, `Err(())` if serialization
/// or sending failed (indicating the connection should be closed).
///
/// The `Sync` bound on `T` is required because the message reference crosses an `.await` point,
/// and the future must be `Send` to work with Tokio's multi-threaded runtime.
async fn send_json_message<T: Serialize + Sync>(
    socket: &mut WebSocket,
    message: &T,
    message_type: &str,
) -> Result<(), ()> {
    match serde_json::to_string(message) {
        Ok(json) => {
            if socket.send(axum::extract::ws::Message::Text(json.into())).await.is_err() {
                warn!("Failed to send WebSocket {}", message_type);
                Err(())
            } else {
                Ok(())
            }
        },
        Err(e) => {
            error!(error = %e, "Failed to serialize {}", message_type);
            Err(())
        },
    }
}

/// Metrics for WebSocket connection handling
#[derive(Clone)]
struct WebSocketMetrics {
    connections_gauge: opentelemetry::metrics::Gauge<u64>,
    messages_counter: opentelemetry::metrics::Counter<u64>,
    errors_counter: opentelemetry::metrics::Counter<u64>,
}

impl WebSocketMetrics {
    fn shared() -> Self {
        static METRICS: OnceLock<WebSocketMetrics> = OnceLock::new();
        METRICS
            .get_or_init(|| {
                let meter = global::meter("skit_websocket");
                Self {
                    connections_gauge: meter
                        .u64_gauge("websocket.connections.active")
                        .with_description("Number of active WebSocket connections")
                        .build(),
                    messages_counter: meter
                        .u64_counter("websocket.messages")
                        .with_description("Total WebSocket messages")
                        .build(),
                    errors_counter: meter
                        .u64_counter("websocket.errors")
                        .with_description("WebSocket errors")
                        .build(),
                }
            })
            .clone()
    }
}

/// Handle a text message received from the WebSocket client.
/// Returns true if the connection should continue, false if it should break.
async fn handle_client_message(
    socket: &mut WebSocket,
    text: String,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
    metrics: &WebSocketMetrics,
) -> bool {
    metrics.messages_counter.add(1, &[KeyValue::new("direction", "inbound")]);

    // Parse the incoming request
    let request: ApiRequest = match serde_json::from_str(&text) {
        Ok(req) => req,
        Err(e) => {
            warn!(error = %e, message_len = text.len(), "Failed to parse WebSocket message");
            metrics.errors_counter.add(1, &[KeyValue::new("error_type", "parse_error")]);
            let error_response = ApiResponse {
                message_type: MessageType::Response,
                correlation_id: None,
                payload: ResponsePayload::Error { message: format!("Invalid JSON: {e}") },
            };
            let _ = send_json_message(socket, &error_response, "error response").await;
            return true; // Continue processing
        },
    };

    // Handle the request and generate a response
    if let Some(response) = handle_api_request(request, app_state, perms, role_name).await {
        // Send the response back
        metrics.messages_counter.add(1, &[KeyValue::new("direction", "outbound")]);
        if send_json_message(socket, &response, "response").await.is_err() {
            metrics.errors_counter.add(1, &[KeyValue::new("error_type", "send_error")]);
            return false; // Break loop
        }
    }

    true // Continue processing
}

/// Main WebSocket connection handler.
#[allow(clippy::cognitive_complexity)]
pub async fn handle_websocket(
    mut socket: WebSocket,
    app_state: Arc<AppState>,
    perms: Permissions,
    role_name: String,
) {
    info!("WebSocket connection established");

    let metrics = WebSocketMetrics::shared();
    let active = ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed) + 1;
    metrics.connections_gauge.record(active, &[]);

    let mut event_rx = app_state.event_tx.subscribe();

    let mut visible_session_ids: HashSet<String> = if perms.access_all_sessions {
        HashSet::new()
    } else {
        let session_manager = app_state.session_manager.lock().await;
        session_manager
            .list_sessions()
            .into_iter()
            .filter(|session| {
                session.created_by.as_ref().is_none_or(|creator| creator == &role_name)
            })
            .map(|session| session.id)
            .collect()
    };

    loop {
        tokio::select! {
            // A message was received from the client
            Some(msg) = socket.recv() => {
                match msg {
                    Ok(axum::extract::ws::Message::Text(text)) => {
                        let max_len = max_ws_message_bytes();
                        if text.len() > max_len {
                            warn!(
                                message_len = text.len(),
                                max_len,
                                "Rejected WebSocket message: too large"
                            );
                            metrics
                                .errors_counter
                                .add(1, &[KeyValue::new("error_type", "message_too_large")]);

                            let error_response = ApiResponse {
                                message_type: MessageType::Response,
                                correlation_id: None,
                                payload: ResponsePayload::Error {
                                    message: format!(
                                        "WebSocket message too large (max {max_len} bytes)"
                                    ),
                                },
                            };
                            let _ = send_json_message(&mut socket, &error_response, "error response")
                                .await;
                            let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
                            break;
                        }

                        if !handle_client_message(&mut socket, text.to_string(), &app_state, &perms, &role_name, &metrics).await {
                            break;
                        }
                    }
                    Ok(axum::extract::ws::Message::Binary(data)) => {
                        let max_len = max_ws_message_bytes();
                        if data.len() > max_len {
                            warn!(
                                message_len = data.len(),
                                max_len,
                                "Rejected WebSocket message: too large"
                            );
                            metrics
                                .errors_counter
                                .add(1, &[KeyValue::new("error_type", "message_too_large")]);
                            let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
                            break;
                        }
                    }
                    Ok(axum::extract::ws::Message::Close(_)) => {
                        info!("WebSocket connection closed");
                        break;
                    }
                    Err(e) => {
                        error!(error = %e, "WebSocket error");
                        metrics.errors_counter.add(1, &[KeyValue::new("error_type", "connection_error")]);
                        break;
                    }
                    _ => {}
                }
            },

            // A broadcast event was received
            event_result = event_rx.recv() => {
                let event = match event_result {
                    Ok(event) => event,
                    Err(RecvError::Lagged(skipped)) => {
                        warn!(skipped, "WebSocket event receiver lagged; dropping events to catch up");
                        metrics.errors_counter.add(1, &[KeyValue::new("error_type", "recv_lagged")]);
                        continue;
                    }
                    Err(RecvError::Closed) => {
                        warn!("WebSocket event channel closed; terminating connection");
                        metrics.errors_counter.add(1, &[KeyValue::new("error_type", "recv_closed")]);
                        break;
                    }
                };

                let should_send = if perms.access_all_sessions {
                    true
                } else {
                    match &event.payload {
                        EventPayload::SessionCreated { session_id, .. } => {
                            let session = {
                                let session_manager = app_state.session_manager.lock().await;
                                session_manager.get_session_by_name_or_id(session_id)
                            };
                            session.is_some_and(|session| {
                                let visible = session
                                    .created_by
                                    .as_ref()
                                    .is_none_or(|creator| creator == &role_name);
                                if visible {
                                    visible_session_ids.insert(session.id);
                                }
                                visible
                            })
                        }
                        EventPayload::SessionDestroyed { session_id } => {
                            visible_session_ids.remove(session_id)
                        }
                        EventPayload::NodeStateChanged { session_id, .. }
                        | EventPayload::NodeStatsUpdated { session_id, .. }
                        | EventPayload::NodeParamsChanged { session_id, .. }
                        | EventPayload::NodeAdded { session_id, .. }
                        | EventPayload::NodeRemoved { session_id, .. }
                        | EventPayload::ConnectionAdded { session_id, .. }
                        | EventPayload::ConnectionRemoved { session_id, .. }
                        | EventPayload::NodeTelemetry { session_id, .. } => {
                            visible_session_ids.contains(session_id)
                        }
                    }
                };

                if should_send {
                    metrics.messages_counter.add(1, &[KeyValue::new("direction", "outbound")]);
                    if send_json_message(&mut socket, &event, "event").await.is_err() {
                        metrics.errors_counter.add(1, &[KeyValue::new("error_type", "send_error")]);
                        break;
                    }
                }
            }
            else => break,
        }
    }

    let prev = ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
    let active = prev.saturating_sub(1);
    metrics.connections_gauge.record(active, &[]);
    info!("WebSocket connection terminated");
}

/// Main API request handler that delegates to specific handlers in websocket_handlers module.
async fn handle_api_request(
    request: ApiRequest,
    app_state: &AppState,
    perms: &Permissions,
    role_name: &str,
) -> Option<ApiResponse> {
    let correlation_id = request.correlation_id.clone();

    let payload = crate::websocket_handlers::handle_request_payload(
        request.payload,
        app_state,
        perms,
        role_name,
        correlation_id.clone(),
    )
    .await?;

    Some(ApiResponse { message_type: MessageType::Response, correlation_id, payload })
}
