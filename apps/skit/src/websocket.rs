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

    if let Some(response) = handle_api_request(request, app_state, perms, role_name).await {
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
                let broadcast_event = match event_result {
                    Ok(ev) => ev,
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

                let event = broadcast_event.event;

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
                        | EventPayload::NodeTelemetry { session_id, .. }
                        | EventPayload::NodeViewDataUpdated { session_id, .. }
                        | EventPayload::RuntimeSchemasUpdated { session_id, .. } => {
                            visible_session_ids.contains(session_id)
                        }
                    }
                };

                if should_send {
                    metrics.messages_counter.add(1, &[KeyValue::new("direction", "outbound")]);
                    if broadcast_event.json.is_empty() {
                        // Pre-serialization failed — skip this event.
                        warn!("Skipping broadcast event with empty pre-serialized JSON");
                        metrics.errors_counter.add(1, &[KeyValue::new("error_type", "serialize_error")]);
                    } else if socket
                        .send(axum::extract::ws::Message::Text(
                            broadcast_event.json.clone(),
                        ))
                        .await
                        .is_err()
                    {
                        warn!("Failed to send WebSocket event");
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

#[cfg(test)]
mod tests {
    // Tests intentionally use unwrap/expect so any failure points directly at the
    // failed precondition (router build, JSON encode, etc.) rather than a
    // propagated `?` from deep inside the test body.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::config::{AuthMode, Config};
    use crate::state::BroadcastEvent;
    use axum::extract::WebSocketUpgrade;
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use std::net::SocketAddr;
    use streamkit_api::{EventPayload, Message, RequestPayload};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    fn make_test_app_state() -> Arc<AppState> {
        let mut config = Config::default();
        // Disable auth so the WS handler can attach without credentials.
        config.auth.mode = AuthMode::Disabled;
        crate::server::create_app_state(config, None)
    }

    async fn spawn_ws_server(perms: Permissions, role: &'static str) -> SocketAddr {
        let state = make_test_app_state();
        let app = Router::new().route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let state = state.clone();
                let perms = perms.clone();
                async move {
                    let response: Response = ws.on_upgrade(move |socket| async move {
                        handle_websocket(socket, state, perms, role.to_string()).await;
                    });
                    response
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service()).await.unwrap();
        });
        addr
    }

    async fn connect(
        addr: SocketAddr,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let (ws, _resp) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
        ws
    }

    fn parse_response(json_str: &str) -> ApiResponse {
        serde_json::from_str(json_str).expect("response is a valid ApiResponse")
    }

    #[test]
    fn max_ws_message_bytes_is_positive_default() {
        // Without SK_WEBSOCKET_MAX_MESSAGE_BYTES set, the OnceLock must
        // resolve to the documented default and never zero.
        let n = max_ws_message_bytes();
        assert!(n >= DEFAULT_MAX_WS_MESSAGE_BYTES, "got {n}");
    }

    #[test]
    fn websocket_metrics_shared_is_callable_repeatedly() {
        // OpenTelemetry's Counter/Gauge types intentionally don't expose
        // pointer identity, so we can't directly assert that two calls return
        // instruments backed by the same OnceLock cell. The singleton
        // invariant is enforced at the source by `OnceLock::get_or_init`; this
        // test simply locks in that repeated access does not panic and yields
        // usable instruments.
        let a = WebSocketMetrics::shared();
        let b = WebSocketMetrics::shared();
        a.messages_counter.add(0, &[]);
        b.connections_gauge.record(0, &[]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_json_yields_error_response_and_keeps_connection_open() {
        let addr = spawn_ws_server(Permissions::admin(), "admin").await;
        let mut ws = connect(addr).await;

        ws.send(WsMessage::Text("this is not json {".into())).await.unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = match msg {
            WsMessage::Text(t) => t,
            other => panic!("expected text response, got {other:?}"),
        };

        let response = parse_response(&text);
        assert!(response.correlation_id.is_none());
        match response.payload {
            ResponsePayload::Error { message } => {
                assert!(message.contains("Invalid JSON"), "got: {message}");
            },
            other => panic!("expected ResponsePayload::Error, got {other:?}"),
        }

        // Connection must remain open after a parse failure — send a follow-up
        // valid request and confirm we still get a response.
        let req = ApiRequest {
            message_type: MessageType::Request,
            correlation_id: Some("c1".into()),
            payload: RequestPayload::GetPermissions,
        };
        ws.send(WsMessage::Text(serde_json::to_string(&req).unwrap().into())).await.unwrap();
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match msg {
            WsMessage::Text(t) => {
                let resp = parse_response(&t);
                assert_eq!(resp.correlation_id.as_deref(), Some("c1"));
                assert!(matches!(resp.payload, ResponsePayload::Permissions { .. }));
            },
            other => panic!("expected text response, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_text_message_is_rejected_and_connection_is_closed() {
        let addr = spawn_ws_server(Permissions::admin(), "admin").await;
        let mut ws = connect(addr).await;

        // 2 MiB > 1 MiB default cap.
        let oversized = "x".repeat(2 * 1024 * 1024);
        ws.send(WsMessage::Text(oversized.into())).await.unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = match msg {
            WsMessage::Text(t) => t,
            other => panic!("expected text rejection response, got {other:?}"),
        };
        let response = parse_response(&text);
        // Assert on response shape only; the production error message is free
        // to evolve (i18n, rewording) without breaking this contract.
        assert!(
            matches!(response.payload, ResponsePayload::Error { .. }),
            "expected ResponsePayload::Error, got {:?}",
            response.payload
        );

        // Server must follow up with an explicit Close frame. A bare
        // stream-end (None) would be a regression — the production handler
        // sends `Message::Close(None)` after the error response (see
        // handle_websocket() oversized-text branch).
        let close_or_end =
            tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.unwrap();
        assert!(
            matches!(close_or_end, Some(Ok(WsMessage::Close(_)))),
            "expected explicit Close frame after oversized text, got {close_or_end:?}",
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_binary_message_closes_connection_without_response() {
        let addr = spawn_ws_server(Permissions::admin(), "admin").await;
        let mut ws = connect(addr).await;

        // 2 MiB > 1 MiB default cap.
        let oversized = vec![0u8; 2 * 1024 * 1024];
        ws.send(WsMessage::Binary(oversized.into())).await.unwrap();

        // The binary branch in handle_websocket() does not emit an error
        // payload — it logs, increments the counter, and sends an explicit
        // Close frame. Pin that contract; a bare stream-end (None) would
        // indicate the close-on-overflow path regressed.
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.unwrap();
        assert!(
            matches!(msg, Some(Ok(WsMessage::Close(_)))),
            "expected explicit Close frame after oversized binary, got {msg:?}",
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn valid_request_round_trips_response_with_matching_correlation_id() {
        let addr = spawn_ws_server(Permissions::admin(), "admin").await;
        let mut ws = connect(addr).await;

        let req = ApiRequest {
            message_type: MessageType::Request,
            correlation_id: Some("xyz".into()),
            payload: RequestPayload::ListSessions,
        };
        ws.send(WsMessage::Text(serde_json::to_string(&req).unwrap().into())).await.unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = match msg {
            WsMessage::Text(t) => t,
            other => panic!("expected text response, got {other:?}"),
        };
        let response = parse_response(&text);
        assert_eq!(response.correlation_id.as_deref(), Some("xyz"));
        assert!(matches!(response.payload, ResponsePayload::SessionsListed { .. }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn client_close_terminates_handler_cleanly() {
        let addr = spawn_ws_server(Permissions::admin(), "admin").await;
        let mut ws = connect(addr).await;

        ws.close(None).await.unwrap();
        // Drain any echo / close acknowledgement, then explicitly verify the
        // stream terminates. Without this assertion a hung-handler regression
        // (server keeps the recv loop alive after client-initiated close)
        // would still let this test pass.
        let mut saw_terminal = false;
        while let Some(msg) =
            tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.unwrap()
        {
            match msg {
                Ok(WsMessage::Close(_)) | Err(_) => {
                    saw_terminal = true;
                    break;
                },
                Ok(_) => {},
            }
        }
        // After the terminal frame (or stream end), the next read must be
        // None — the handler has dropped the socket and no further messages
        // can arrive.
        let tail =
            tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.unwrap();
        assert!(
            tail.is_none(),
            "handler kept stream alive after client close: saw_terminal={saw_terminal}, tail={tail:?}",
        );
    }

    /// Drives a request/response round trip through the WebSocket and returns
    /// after the response is observed. Because `handle_websocket` calls
    /// `event_tx.subscribe()` (line 141 of this file) *before* entering the
    /// recv loop that emits this response, observing the response on the
    /// client side proves the broadcast subscription is live — letting
    /// subsequent `event_tx.send()` calls reach the handler without relying on
    /// fixed sleeps.
    async fn wait_for_handler_subscribed(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        let req = ApiRequest {
            message_type: MessageType::Request,
            correlation_id: Some("__ready__".into()),
            payload: RequestPayload::GetPermissions,
        };
        ws.send(WsMessage::Text(serde_json::to_string(&req).unwrap().into())).await.unwrap();
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = match msg {
            WsMessage::Text(t) => t,
            other => panic!("expected text readiness response, got {other:?}"),
        };
        let resp = parse_response(&text);
        assert_eq!(resp.correlation_id.as_deref(), Some("__ready__"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn broadcast_events_are_filtered_for_clients_without_access_all_sessions() {
        // Build a state shared between the broadcaster and the WS handler so we
        // can push events through `event_tx` and observe the gate.
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        let state = crate::server::create_app_state(config, None);
        // viewer() grants list_sessions but NOT access_all_sessions — exactly
        // the role that should be filtered out of cross-session events.
        let perms = Permissions::viewer();
        assert!(!perms.access_all_sessions, "viewer must not see all sessions");

        let state_clone = state.clone();
        let perms_clone = perms.clone();
        let app = Router::new().route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let state = state_clone.clone();
                let perms = perms_clone.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        handle_websocket(socket, state, perms, "viewer".to_string()).await;
                    })
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service()).await.unwrap();
        });

        let mut ws = connect(addr).await;
        wait_for_handler_subscribed(&mut ws).await;

        // Publish a node-state event for a session the viewer cannot see.
        // Because `visible_session_ids` is empty and access_all_sessions is
        // false, this must be filtered out.
        let event = Message {
            message_type: MessageType::Event,
            correlation_id: None,
            payload: EventPayload::NodeStateChanged {
                session_id: "invisible-session".into(),
                node_id: "n1".into(),
                state: streamkit_core::NodeState::Running,
                timestamp: "1970-01-01T00:00:00Z".into(),
            },
        };
        let _ = state.event_tx.send(BroadcastEvent::to_all(event));

        // Positive-proof companion: round-trip a request *after* publishing
        // the filtered event. Reading the response back proves the handler is
        // alive and has processed both branches of its select loop. If the
        // filter were broken, the invisible-session event would be sitting in
        // the stream alongside (or ahead of) the response — drain until we
        // see the response and fail on any unrelated message in the way.
        let req = ApiRequest {
            message_type: MessageType::Request,
            correlation_id: Some("__filter_check__".into()),
            payload: RequestPayload::GetPermissions,
        };
        ws.send(WsMessage::Text(serde_json::to_string(&req).unwrap().into())).await.unwrap();

        // If the handler picked the event branch first and forwarded it, the
        // *first* message off the wire will be the event (not the response).
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("response must arrive within deadline")
            .unwrap()
            .unwrap();
        let WsMessage::Text(text) = msg else {
            panic!("expected text response, got {msg:?}");
        };
        let resp = parse_response(&text);
        assert_eq!(
            resp.correlation_id.as_deref(),
            Some("__filter_check__"),
            "filter leaked: received non-response message before round-trip: {text}",
        );

        // Otherwise the handler picked the WS branch first and the leaked event
        // would still arrive *after* the response. A short additional read
        // catches that ordering.
        let extra = tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await;
        assert!(extra.is_err(), "filter leaked extra messages after response: {extra:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn broadcast_events_reach_clients_with_access_all_sessions() {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        let state = crate::server::create_app_state(config, None);
        let perms = Permissions::admin();

        let state_clone = state.clone();
        let perms_clone = perms.clone();
        let app = Router::new().route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let state = state_clone.clone();
                let perms = perms_clone.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        handle_websocket(socket, state, perms, "admin".to_string()).await;
                    })
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service()).await.unwrap();
        });

        let mut ws = connect(addr).await;
        wait_for_handler_subscribed(&mut ws).await;

        let event = Message {
            message_type: MessageType::Event,
            correlation_id: None,
            payload: EventPayload::NodeStateChanged {
                session_id: "any-session".into(),
                node_id: "n1".into(),
                state: streamkit_core::NodeState::Running,
                timestamp: "1970-01-01T00:00:00Z".into(),
            },
        };
        let _ = state.event_tx.send(BroadcastEvent::to_all(event));

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = match msg {
            WsMessage::Text(t) => t,
            other => panic!("expected event text, got {other:?}"),
        };
        // Parse and assert on the serde tag rather than a case-tolerant
        // substring — EventPayload carries `#[serde(tag = "event")]` with
        // `rename_all = "lowercase"`, so the variant tag is exactly
        // "nodestatechanged". A mis-cased substring check would silently
        // accept a renamed variant.
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("event text must be valid JSON");
        assert_eq!(value["payload"]["event"], "nodestatechanged", "got: {text}");
        assert_eq!(value["payload"]["session_id"], "any-session", "got: {text}");
    }
}
