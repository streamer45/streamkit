// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::{
    body::Body,
    extract::{
        multipart::MultipartError, ws::WebSocketUpgrade, DefaultBodyLimit, MatchedPath, Multipart,
        Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use bytes::Bytes;
use multer as raw_multer;
use opentelemetry::{global, KeyValue};
use rust_embed::RustEmbed;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
use tower::limit::ConcurrencyLimitLayer;
use tower::ServiceBuilder;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::{debug, error, info, warn};

use crate::file_security;
use crate::plugins::UnifiedPluginManager;
use crate::profiling;
use crate::state::AppState;
use crate::websocket;
use streamkit_api::yaml::{compile, UserPipeline};
use streamkit_api::Pipeline;
use streamkit_api::{ApiPipeline, Event as ApiEvent, EventPayload, MessageType};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_engine::{Engine, OneshotEngineConfig};

use crate::session::SessionManager;

use crate::config::Config;
use tokio_stream::wrappers::ReceiverStream;

use anyhow::Error as AnyhowError;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
struct Assets;

#[cfg(feature = "profiling")]
async fn profile_cpu_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Query<crate::profiling::ProfileParams>,
) -> Result<Response, StatusCode> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.access_all_sessions {
        return Err(StatusCode::FORBIDDEN);
    }
    crate::profiling::profile_cpu(query).await
}

#[cfg(feature = "profiling")]
async fn profile_heap_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.access_all_sessions {
        return Err(StatusCode::FORBIDDEN);
    }
    crate::profiling::profile_heap().await
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Type alias for a boxed byte stream used in media processing
type MediaStream = Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Unpin + Send>;

static ONESHOT_DURATION_HISTOGRAM: OnceLock<opentelemetry::metrics::Histogram<f64>> =
    OnceLock::new();
static HTTP_METRICS: OnceLock<(
    opentelemetry::metrics::Counter<u64>,
    opentelemetry::metrics::Histogram<f64>,
)> = OnceLock::new();

/// Helper function to safely read from a RwLock without panicking.
/// Returns a 503 Service Unavailable error if the lock is poisoned.
fn read_registry(
    app_state: &Arc<AppState>,
) -> Result<std::sync::RwLockReadGuard<'_, streamkit_core::NodeRegistry>, StatusCode> {
    app_state.engine.registry.read().map_err(|e| {
        error!("Engine registry poisoned: {}", e);
        StatusCode::SERVICE_UNAVAILABLE
    })
}

/// Creates a CORS layer from the configuration.
///
/// Supports wildcard patterns in origins:
/// - `*` - Allow all origins
/// - `http://localhost:*` - Match any port on localhost
/// - Exact origins like `https://example.com`
fn origin_matches_pattern(origin: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    // Handle wildcard port matching (e.g., "http://localhost:*")
    if let Some(prefix_without_port) = pattern.strip_suffix(":*") {
        let Some(rest) = origin.strip_prefix(prefix_without_port) else {
            return false;
        };

        let Some(port_str) = rest.strip_prefix(':') else {
            return false;
        };

        return !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit());
    }

    origin == pattern
}

fn escape_html_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn normalized_base_path_for_html(app_state: &AppState) -> Option<String> {
    app_state
        .config
        .server
        .base_path
        .as_deref()
        .map(str::trim)
        .and_then(|p| if p.is_empty() { None } else { Some(p) })
        .map(|p| p.trim_end_matches('/'))
        .and_then(|p| if p == "/" { None } else { Some(p) })
        .map(|p| if p.starts_with('/') { p.to_string() } else { format!("/{p}") })
}

/// Best-effort Origin enforcement for browser security.
///
/// This is NOT authentication. It is a defense-in-depth measure that mitigates
/// cross-site request attacks against local/self-hosted deployments by rejecting
/// requests whose `Origin` header is not on the configured allowlist.
///
/// Behavior:
/// - Only applies to `/api/` paths.
/// - Only applies to non-idempotent methods (POST/PUT/PATCH/DELETE).
/// - If no `Origin` header is present (typical for CLI/tools), the request is allowed.
async fn origin_guard_middleware(
    State(app_state): State<Arc<AppState>>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Response {
    use axum::http::Method;

    let path = req.uri().path();
    let method = req.method().clone();

    let is_api = path.starts_with("/api/");
    let is_mutating = matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE);

    if is_api && is_mutating {
        if let Some(origin) = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            let allowed = app_state
                .config
                .server
                .cors
                .allowed_origins
                .iter()
                .any(|p| origin_matches_pattern(origin, p));

            if !allowed {
                warn!(
                    origin = %origin,
                    method = %method,
                    path = %path,
                    "Rejected request: Origin not allowed"
                );
                return (
                    StatusCode::FORBIDDEN,
                    "Origin not allowed (configure [server.cors].allowed_origins)",
                )
                    .into_response();
            }
        }
    }

    next.run(req).await
}

fn create_cors_layer(config: &crate::config::CorsConfig) -> CorsLayer {
    use axum::http::{HeaderValue, Method};

    // Check for wildcard (allow all)
    if config.allowed_origins.iter().any(|o| o == "*") {
        info!("CORS configured to allow all origins (permissive mode)");
        return CorsLayer::permissive();
    }

    // If no origins specified, use default restrictive behavior
    if config.allowed_origins.is_empty() {
        info!("CORS configured with no allowed origins (most restrictive)");
        return CorsLayer::new();
    }

    // Build list of patterns for matching
    let patterns: Vec<String> = config.allowed_origins.clone();

    info!(
        allowed_origins = ?patterns,
        "CORS configured with origin allowlist"
    );

    // Create a predicate-based allowlist
    let allow_origin = AllowOrigin::predicate(move |origin: &HeaderValue, _request_parts| {
        let Ok(origin_str) = origin.to_str() else {
            return false;
        };

        patterns.iter().any(|pattern| origin_matches_pattern(origin_str, pattern))
    });

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers(Any)
        .expose_headers(Any)
}

#[cfg(test)]
mod cors_tests {
    use super::origin_matches_pattern;

    #[test]
    fn cors_wildcard_port_matches_localhost_port_only() {
        assert!(origin_matches_pattern("http://localhost:8080", "http://localhost:*"));
        assert!(origin_matches_pattern("https://localhost:12345", "https://localhost:*"));

        assert!(!origin_matches_pattern("http://localhost", "http://localhost:*"));
        assert!(!origin_matches_pattern("http://localhost:abc", "http://localhost:*"));
        assert!(!origin_matches_pattern("http://localhost123:8080", "http://localhost:*"));
        assert!(!origin_matches_pattern("http://127.0.0.1:8080", "http://localhost:*"));
    }

    #[test]
    fn cors_exact_match_only() {
        assert!(origin_matches_pattern("https://example.com", "https://example.com"));
        assert!(!origin_matches_pattern("https://example.com:443", "https://example.com"));
        assert!(!origin_matches_pattern("https://example.com", "https://example.com:*"));
    }
}

// File path validation lives in `crate::file_security` so it can be reused by both
// HTTP handlers and the WebSocket control plane.

/// Axum handler to list all available node definitions.
async fn list_node_definitions_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    use streamkit_core::types::PacketType;
    use streamkit_core::{InputPin, NodeDefinition, OutputPin, PinCardinality};

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    let mut definitions = read_registry(&app_state)?.definitions();

    // Add synthetic node definitions for oneshot-only nodes
    // These are virtual markers that get replaced at runtime in oneshot pipelines

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
        "Listed {} available node definitions via HTTP (including synthetic oneshot nodes)",
        definitions.len()
    );
    Ok(Json(definitions))
}

/// Response structure for the permissions endpoint
#[derive(Serialize)]
struct PermissionsResponse {
    role: String,
    permissions: streamkit_api::PermissionsInfo,
}

/// Axum handler to get current user's permissions
async fn get_permissions_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    info!(role = %role_name, "Returning permissions for role via HTTP");

    Json(PermissionsResponse { role: role_name, permissions: perms.to_info() })
}

/// Response structure for the frontend config endpoint
#[derive(Serialize)]
struct FrontendConfig {
    #[cfg(feature = "moq")]
    #[serde(skip_serializing_if = "Option::is_none")]
    moq_gateway_url: Option<String>,
}

/// Axum handler to get frontend configuration
async fn get_config_handler(State(app_state): State<Arc<AppState>>) -> impl IntoResponse {
    #[cfg(not(feature = "moq"))]
    let _ = &app_state;

    let config = FrontendConfig {
        #[cfg(feature = "moq")]
        moq_gateway_url: app_state.config.server.moq_gateway_url.clone(),
    };

    Json(config)
}

async fn list_plugins_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    let mut plugins = app_state.plugin_manager.lock().await.list_plugins();

    // Filter plugins based on allowed_plugins permission
    plugins.retain(|plugin| perms.is_plugin_allowed(&plugin.kind));

    Json(plugins)
}

async fn upload_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, PluginHttpError> {
    // Global hard gate: do not allow runtime plugin uploads unless explicitly enabled.
    if !app_state.config.plugins.allow_http_management {
        return Err(PluginHttpError::Forbidden(
            "Plugin uploads are disabled by configuration. Set [plugins].allow_http_management = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    // Check permission to load plugins
    if !perms.load_plugins {
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot load plugins".to_string(),
        ));
    }

    let mut plugin_file_name: Option<String> = None;
    let mut temp_file_path: Option<std::path::PathBuf> = None;

    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or("").to_string();
        if name != "plugin" {
            continue;
        }

        if let Some(existing) = temp_file_path.as_ref() {
            let _ = tokio::fs::remove_file(existing).await;
            return Err(PluginHttpError::BadRequest(
                "Multiple 'plugin' fields provided".to_string(),
            ));
        }

        let mut field = field;
        let file_name =
            field.file_name().map(std::string::ToString::to_string).ok_or_else(|| {
                PluginHttpError::BadRequest(
                    "Uploaded plugin file must include a filename".to_string(),
                )
            })?;

        // Stream upload to a temp file to avoid buffering large artifacts in memory.
        let tmp_name = format!("streamkit-plugin-upload-{}", uuid::Uuid::new_v4());
        let tmp_path = std::env::temp_dir().join(tmp_name);
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .await
            .map_err(|e| PluginHttpError::BadRequest(format!("Failed to create temp file: {e}")))?;

        let mut total_bytes: usize = 0;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    total_bytes = total_bytes.saturating_add(chunk.len());
                    if total_bytes > app_state.config.server.max_body_size {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(PluginHttpError::BadRequest(format!(
                            "Plugin upload exceeds configured max body size ({} bytes)",
                            app_state.config.server.max_body_size
                        )));
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(PluginHttpError::BadRequest(format!(
                            "Failed to write temp file: {e}"
                        )));
                    }
                },
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Err(PluginHttpError::BadRequest(format!(
                        "Failed to read plugin upload stream: {e}"
                    )));
                },
            }
        }

        // Ensure all data is flushed to disk before we try to load the plugin.
        // This is important because load_from_temp_file uses sync file operations.
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(PluginHttpError::BadRequest(format!("Failed to flush temp file: {e}")));
        }
        if let Err(e) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(PluginHttpError::BadRequest(format!("Failed to sync temp file: {e}")));
        }
        // Explicitly drop the file handle to ensure it's closed before we read it
        drop(file);

        plugin_file_name = Some(file_name);
        temp_file_path = Some(tmp_path);
    }

    let file_name = plugin_file_name
        .ok_or_else(|| PluginHttpError::BadRequest("Missing 'plugin' file field".to_string()))?;
    let tmp_path = temp_file_path
        .ok_or_else(|| PluginHttpError::BadRequest("Missing 'plugin' file field".to_string()))?;

    let mut manager = app_state.plugin_manager.lock().await;
    let summary = manager.load_from_temp_file(&file_name, &tmp_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        PluginHttpError::from(e)
    })?;

    // Check if the loaded plugin is allowed
    if !perms.is_plugin_allowed(&summary.kind) {
        // Unload the plugin since it's not allowed
        let _ = manager.unload_plugin(&summary.kind, true);
        drop(manager);
        return Err(PluginHttpError::Forbidden(format!(
            "Permission denied: plugin '{}' not allowed",
            summary.kind
        )));
    }

    drop(manager);

    Ok((StatusCode::CREATED, Json(summary)))
}

#[derive(Debug, Default, Deserialize)]
struct DeletePluginQuery {
    #[serde(default)]
    keep_file: bool,
}

async fn delete_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Query(query): Query<DeletePluginQuery>,
) -> Result<impl IntoResponse, PluginHttpError> {
    // Global hard gate: do not allow runtime plugin deletion unless explicitly enabled.
    if !app_state.config.plugins.allow_http_management {
        return Err(PluginHttpError::Forbidden(
            "Plugin deletion is disabled by configuration. Set [plugins].allow_http_management = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    // Check permission to delete plugins
    if !perms.delete_plugins {
        warn!(
            plugin_kind = %kind,
            delete_plugins = perms.delete_plugins,
            "Blocked attempt to delete plugin: permission denied"
        );
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot delete plugins".to_string(),
        ));
    }

    info!(plugin_kind = %kind, keep_file = query.keep_file, "Deleting plugin");
    let mut manager = app_state.plugin_manager.lock().await;
    let summary = manager.unload_plugin(&kind, !query.keep_file).map_err(PluginHttpError::from)?;
    drop(manager);

    Ok(Json(summary))
}

async fn list_packet_types_handler() -> impl IntoResponse {
    let registry = streamkit_core::packet_meta::packet_type_registry();
    Json(registry)
}

/// Axum handler to get MoQ WebTransport certificate fingerprints
#[cfg(feature = "moq")]
async fn get_moq_fingerprints_handler(
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(gateway) = &app_state.moq_gateway {
        let fingerprints = gateway.get_fingerprints().await;
        Ok(Json(serde_json::json!({
            "fingerprints": fingerprints
        })))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Handler for /certificate.sha256 - returns the first certificate fingerprint as plain text
/// This is used by the Hang MoQ library for automatic fingerprint fetching
#[cfg(feature = "moq")]
async fn get_certificate_sha256_handler(
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(gateway) = &app_state.moq_gateway {
        let fingerprints = gateway.get_fingerprints().await;
        fingerprints.first().map_or_else(
            || Err(StatusCode::SERVICE_UNAVAILABLE),
            |first_fingerprint| {
                Ok(([(header::CONTENT_TYPE, "text/plain")], first_fingerprint.clone()))
            },
        )
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Request body for creating a session with a pipeline
#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    name: Option<String>,
    yaml: String,
}

/// Response body for creating a session
#[derive(Debug, Serialize)]
struct CreateSessionResponse {
    session_id: String,
    name: Option<String>,
    created_at: String,
}

/// Helper function to populate the session's in-memory pipeline representation
/// from the compiled engine pipeline definition.
async fn populate_session_pipeline(session: &crate::session::Session, engine_pipeline: &Pipeline) {
    let mut pipeline = session.pipeline.lock().await;

    // Add nodes to in-memory pipeline
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

    // Add connections to in-memory pipeline
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

/// Helper function to send all node and connection control messages to the engine actor.
async fn send_pipeline_to_engine(session: &crate::session::Session, engine_pipeline: &Pipeline) {
    // Send control messages to engine actor (asynchronous)
    // The engine will actually instantiate the nodes
    for (node_id, node_spec) in &engine_pipeline.nodes {
        session
            .send_control_message(EngineControlMessage::AddNode {
                node_id: node_id.clone(),
                kind: node_spec.kind.clone(),
                params: node_spec.params.clone(),
            })
            .await;
    }

    // Send connection control messages to engine actor
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
async fn create_session_handler(
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

    // Global session limit
    let (current_count, name_taken) = {
        let session_manager = app_state.session_manager.lock().await;
        let current_count = session_manager.session_count();
        let name_taken = req.name.as_deref().is_some_and(|n| session_manager.is_name_taken(n));
        drop(session_manager);
        (current_count, name_taken)
    };
    if let Some(ref session_name) = req.name {
        if name_taken {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Failed to create session: Session with name '{session_name}' already exists"
                ),
            ));
        }
    }
    if !app_state.config.permissions.can_accept_session(current_count) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Maximum concurrent sessions limit reached".to_string(),
        ));
    }

    // Parse and compile the YAML pipeline
    let user_pipeline: UserPipeline = serde_saphyr::from_str(&req.yaml)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid YAML: {e}")))?;

    let engine_pipeline = compile(user_pipeline)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid pipeline: {e}")))?;

    // Validate the pipeline has at least one node
    if engine_pipeline.nodes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Pipeline is empty. Add some nodes before creating a session.".to_string(),
        ));
    }

    for (node_id, node) in &engine_pipeline.nodes {
        if node.kind == "streamkit::http_input" || node.kind == "streamkit::http_output" {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Node '{node_id}' kind '{}' is oneshot-only and cannot be used in dynamic sessions",
                    node.kind
                ),
            ));
        }

        if !perms.is_node_allowed(&node.kind) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Permission denied: node '{node_id}' kind '{}' not allowed", node.kind),
            ));
        }

        if node.kind.starts_with("plugin::") && !perms.is_plugin_allowed(&node.kind) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Permission denied: node '{node_id}' plugin '{}' not allowed", node.kind),
            ));
        }
    }

    validate_file_reader_paths(&engine_pipeline, &app_state.config.security).map_err(
        |e| match e {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::PipelineCompilation(msg) => {
                (StatusCode::BAD_REQUEST, format!("Invalid pipeline: {msg}"))
            },
            AppError::Serde(err) => {
                (StatusCode::BAD_REQUEST, format!("Invalid YAML config format: {err}"))
            },
            AppError::Multipart(err) => {
                (StatusCode::BAD_REQUEST, format!("Invalid multipart payload: {err}"))
            },
            AppError::Engine(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Pipeline execution error: {err}"))
            },
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
        },
    )?;

    validate_file_writer_paths(&engine_pipeline, &app_state.config.security).map_err(
        |e| match e {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::PipelineCompilation(msg) => {
                (StatusCode::BAD_REQUEST, format!("Invalid pipeline: {msg}"))
            },
            AppError::Serde(err) => {
                (StatusCode::BAD_REQUEST, format!("Invalid YAML config format: {err}"))
            },
            AppError::Multipart(err) => {
                (StatusCode::BAD_REQUEST, format!("Invalid multipart payload: {err}"))
            },
            AppError::Engine(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Pipeline execution error: {err}"))
            },
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
        },
    )?;

    validate_script_paths(&engine_pipeline, &app_state.config.security).map_err(|e| match e {
        AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
        AppError::PipelineCompilation(msg) => {
            (StatusCode::BAD_REQUEST, format!("Invalid pipeline: {msg}"))
        },
        AppError::Serde(err) => {
            (StatusCode::BAD_REQUEST, format!("Invalid YAML config format: {err}"))
        },
        AppError::Multipart(err) => {
            (StatusCode::BAD_REQUEST, format!("Invalid multipart payload: {err}"))
        },
        AppError::Engine(err) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Pipeline execution error: {err}"))
        },
        AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
    })?;

    // Create the session without holding the session manager lock.
    let session = crate::session::Session::create(
        &app_state.engine,
        &app_state.config,
        req.name.clone(),
        app_state.event_tx.clone(),
        Some(role_name.clone()),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create session: {e}")))?;

    // Insert the session with short lock hold and re-check limits to avoid races.
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
        if error_msg == "Maximum concurrent sessions limit reached" {
            return Err((StatusCode::TOO_MANY_REQUESTS, error_msg));
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create session: {error_msg}"),
        ));
    }

    let session_id = session.id.clone();
    let session_name = session.name.clone();
    let created_at_str = crate::session::system_time_to_rfc3339(session.created_at);

    info!(session_id = %session_id, name = ?session_name, "Created new session via HTTP");

    // Update the session pipeline immediately (synchronous)
    // This ensures GET /sessions/{id}/pipeline returns the nodes right away
    populate_session_pipeline(&session, &engine_pipeline).await;

    // Send control messages to engine actor to instantiate nodes and connections
    send_pipeline_to_engine(&session, &engine_pipeline).await;

    info!(
        "Session {} initialized with {} nodes and {} connections",
        session_id,
        engine_pipeline.nodes.len(),
        engine_pipeline.connections.len()
    );

    // Broadcast event to all WebSocket clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionCreated {
            session_id: session_id.clone(),
            name: session_name.clone(),
            created_at: created_at_str.clone(),
        },
    };
    if app_state.event_tx.send(event).is_err() {
        debug!("No WebSocket clients connected to receive SessionCreated event");
    }

    Ok(Json(CreateSessionResponse { session_id, name: session_name, created_at: created_at_str }))
}

/// Axum handler to get the list of active sessions.
async fn list_sessions_handler(
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
async fn destroy_session_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    // Check permission
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

        // Check ownership before destroying
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
    if let Err(e) = session.shutdown_and_wait().await {
        warn!(session_id = %destroyed_id, error = %e, "Error during engine shutdown");
    }

    info!(session_id = %destroyed_id, "Session destroyed successfully via HTTP");

    // Broadcast event to all WebSocket clients
    let event = ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::SessionDestroyed { session_id: destroyed_id.clone() },
    };
    if let Err(e) = app_state.event_tx.send(event) {
        error!("Failed to broadcast SessionDestroyed event: {}", e);
    }

    (StatusCode::OK, Json(serde_json::json!({ "session_id": destroyed_id }))).into_response()
}

/// Axum handler to get the pipeline for a specific session.
async fn get_pipeline_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<ApiPipeline>, StatusCode> {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.list_sessions {
        return Err(StatusCode::FORBIDDEN);
    }

    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    };

    let Some(session) = session else {
        warn!("Attempted to fetch pipeline for non-existent session '{}' via HTTP", session_id);
        return Err(StatusCode::NOT_FOUND);
    };

    if !perms.access_all_sessions && session.created_by.as_ref().is_some_and(|c| c != &role_name) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Fetch current node states without holding the pipeline lock.
    let node_states = session.get_node_states().await.unwrap_or_default();

    // Clone pipeline (short lock hold) and add runtime state to nodes.
    let mut api_pipeline = {
        let pipeline = session.pipeline.lock().await;
        pipeline.clone()
    };
    for (id, node) in &mut api_pipeline.nodes {
        node.state = node_states.get(id).cloned();
    }

    info!("Fetched pipeline with states for session '{}' via HTTP", session_id);
    Ok(Json(api_pipeline))
}

/// Result of parsing multipart request with config and optional media stream
struct MultipartParseResult {
    user_pipeline: UserPipeline,
    media_stream: MediaStream,
    media_content_type: Option<String>,
    has_media: bool,
}

/// Extract content-type header and multipart boundary from request headers.
fn extract_multipart_boundary(headers: &HeaderMap) -> Result<String, AppError> {
    let ct_header = headers
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| AppError::BadRequest("Missing Content-Type header".to_string()))
        .and_then(|hv| {
            hv.to_str().map_err(|_| AppError::BadRequest("Invalid Content-Type header".to_string()))
        })?;
    raw_multer::parse_boundary(ct_header)
        .map_err(|e| AppError::BadRequest(format!("Invalid multipart boundary: {e}")))
}

/// Parse and validate the first multipart field as config.
async fn parse_config_field(
    multipart: &mut raw_multer::Multipart<'_>,
) -> Result<UserPipeline, AppError> {
    tracing::debug!("Parsing first multipart field");
    let first_field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
        .ok_or_else(|| AppError::BadRequest("Empty multipart payload".to_string()))?;
    let first_name = first_field.name().map(std::string::ToString::to_string).unwrap_or_default();
    if first_name != "config" {
        return Err(AppError::BadRequest(
            "Multipart fields must be ordered: 'config' first, then 'media'".to_string(),
        ));
    }

    let config_bytes = first_field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read config field: {e}")))?;
    serde_saphyr::from_slice(&config_bytes).map_err(Into::into)
}

/// Parse the multipart request and extract config and media stream.
/// Returns the parsed pipeline config, media stream (possibly empty), content type, and whether media was provided.
///
/// This needs to stay relatively monolithic because it combines multipart streaming
/// with a spawned task, and the `Multipart<'_>` lifetime makes further extraction awkward.
#[allow(clippy::cognitive_complexity)]
async fn parse_multipart_request(
    req: axum::extract::Request<Body>,
) -> Result<MultipartParseResult, AppError> {
    let headers = req.headers().clone();
    let boundary = extract_multipart_boundary(&headers)?;
    let body_stream = req.into_body().into_data_stream();
    let mut multipart = raw_multer::Multipart::new(body_stream, boundary);

    // Parse the config field
    let user_pipeline = parse_config_field(&mut multipart).await?;

    // Setup channels for streaming media field
    let (media_tx, media_rx) = tokio::sync::mpsc::channel::<Result<Bytes, axum::Error>>(16);
    let (ct_tx, ct_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let (has_media_tx, has_media_rx) = tokio::sync::oneshot::channel::<bool>();

    // Spawn producer task to stream media field
    // Note: Must remain inline due to Multipart<'r> lifetime constraints
    tokio::spawn(async move {
        while let Ok(next) = multipart.next_field().await {
            if let Some(mut field) = next {
                let fname = field.name().map(std::string::ToString::to_string).unwrap_or_default();
                if fname == "media" {
                    let _ = has_media_tx.send(true);
                    let _ = ct_tx.send(field.content_type().map(std::string::ToString::to_string));
                    stream_media_field_chunks(&mut field, &media_tx).await;
                    drop(media_tx);
                    break;
                }
                tracing::warn!("Ignoring unknown multipart field: {}", fname);
            } else {
                let _ = has_media_tx.send(false);
                tracing::debug!("No media field found in multipart");
                break;
            }
        }
    });

    // Wait to see if media field exists (timeout prevents hanging on slow/broken clients).
    // If this times out, fail the request instead of guessing "no media".
    let has_media = tokio::time::timeout(std::time::Duration::from_secs(5), has_media_rx)
        .await
        .map_err(|_| {
            AppError::BadRequest("Timed out waiting for multipart media field".to_string())
        })?
        .unwrap_or(false);

    let media_stream: MediaStream = Box::new(ReceiverStream::new(media_rx).map(|x| x));
    let media_content_type: Option<String> = ct_rx.await.ok().flatten();

    Ok(MultipartParseResult { user_pipeline, media_stream, media_content_type, has_media })
}

/// Stream all chunks from a media field through the provided channel.
async fn stream_media_field_chunks(
    field: &mut raw_multer::Field<'_>,
    media_tx: &tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>,
) {
    let mut chunk_count: usize = 0;
    let mut total_bytes: usize = 0;
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                chunk_count += 1;
                total_bytes += chunk.len();
                if media_tx.send(Ok(chunk)).await.is_err() {
                    tracing::debug!(
                        "Media consumer dropped after {} chunks ({} bytes)",
                        chunk_count,
                        total_bytes
                    );
                    break;
                }
            },
            Ok(None) => {
                tracing::info!(
                    "Finished streaming media after {} chunks ({} bytes)",
                    chunk_count,
                    total_bytes
                );
                break;
            },
            Err(e) => {
                let _ = media_tx.send(Err(axum::Error::new(e))).await;
                break;
            },
        }
    }
}

/// Validate that the pipeline has the required nodes based on whether media was provided.
/// Returns (has_http_input, has_file_read, has_http_output) for logging purposes.
fn validate_pipeline_nodes(
    pipeline_def: &Pipeline,
    has_media: bool,
) -> Result<(bool, bool, bool), AppError> {
    let has_http_input =
        pipeline_def.nodes.values().any(|node| node.kind == "streamkit::http_input");
    let has_http_output =
        pipeline_def.nodes.values().any(|node| node.kind == "streamkit::http_output");
    let has_file_read = pipeline_def.nodes.values().any(|node| node.kind == "core::file_reader");

    // Validate entry point based on whether media was provided
    if has_media {
        // HTTP streaming mode: require http_input
        if !has_http_input {
            return Err(AppError::BadRequest(
                "Pipeline must contain one 'streamkit::http_input' node when media is provided"
                    .to_string(),
            ));
        }
    } else {
        // File-based mode: require file_read, disallow http_input
        if has_http_input {
            return Err(AppError::BadRequest(
                "Pipeline cannot contain 'streamkit::http_input' node when no media is provided"
                    .to_string(),
            ));
        }
        if !has_file_read {
            return Err(AppError::BadRequest(
                "Pipeline must contain at least one 'core::file_reader' node when no media is provided"
                    .to_string(),
            ));
        }
    }

    // Always require http_output for response streaming
    if !has_http_output {
        return Err(AppError::BadRequest(
            "Pipeline must contain one 'streamkit::http_output' node for oneshot processing"
                .to_string(),
        ));
    }

    Ok((has_http_input, has_file_read, has_http_output))
}

/// Validate file paths in all file_reader nodes to prevent path traversal attacks.
fn validate_file_reader_paths(
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
fn validate_file_writer_paths(
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
fn validate_script_paths(
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
fn load_script_secrets(
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

/// Build HTTP response from pipeline execution result.
fn build_streaming_response(
    pipeline_result: streamkit_engine::OneshotPipelineResult,
    start_time: Instant,
    duration_histogram: opentelemetry::metrics::Histogram<f64>,
) -> Response {
    tracing::debug!(
        "Creating streaming response with content type: {}",
        pipeline_result.content_type
    );

    let stream = ReceiverStream::new(pipeline_result.data_stream).map(Ok::<_, Infallible>);
    let stream = InstrumentedOneshotStream::new(stream, start_time, duration_histogram);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    match pipeline_result.content_type.parse() {
        Ok(ct) => headers.insert("Content-Type", ct),
        Err(e) => {
            tracing::error!(
                content_type = %pipeline_result.content_type,
                error = %e,
                "Failed to parse content type from pipeline output, using fallback"
            );
            // Fallback MIME type is a constant string that will always parse successfully
            #[allow(clippy::expect_used)]
            headers.insert(
                "Content-Type",
                "application/octet-stream".parse().expect("fallback MIME type should always parse"),
            )
        },
    };

    tracing::info!("Returning streaming response to client");
    (headers, body).into_response()
}

struct InstrumentedOneshotStream<S> {
    inner: S,
    start_time: Instant,
    recorded: bool,
    duration_histogram: opentelemetry::metrics::Histogram<f64>,
}

impl<S> InstrumentedOneshotStream<S> {
    const fn new(
        inner: S,
        start_time: Instant,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> Self {
        Self { inner, start_time, recorded: false, duration_histogram }
    }

    fn record(&mut self, status: &'static str) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        let labels = [KeyValue::new("status", status)];
        self.duration_histogram.record(self.start_time.elapsed().as_secs_f64(), &labels);
    }
}

impl<S> Drop for InstrumentedOneshotStream<S> {
    fn drop(&mut self) {
        if !self.recorded {
            // If the client disconnects early, the response body stream is dropped without EOF.
            // Record as error so we still get visibility into partial/aborted oneshot executions.
            self.record("error");
        }
    }
}

impl<S> Stream for InstrumentedOneshotStream<S>
where
    S: Stream<Item = Result<Bytes, Infallible>> + Unpin,
{
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(None) => {
                self.record("ok");
                Poll::Ready(None)
            },
            other => other,
        }
    }
}

/// The Axum handler for a oneshot multipart processing request.
#[allow(clippy::cognitive_complexity)]
async fn process_oneshot_pipeline_handler(
    State(app_state): State<Arc<AppState>>,
    req: axum::extract::Request<Body>,
) -> Result<Response, AppError> {
    tracing::info!("Processing multipart request");

    // Enforce role-based access control for oneshot execution.
    //
    // StreamKit does not implement authentication, but it does implement RBAC.
    // Even for local demos, enforce the configured role/permissions so deployments
    // can run safely behind a reverse proxy or other auth layer.
    let headers = req.headers().clone();
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
    if !perms.create_sessions {
        return Err(AppError::Forbidden(
            "Permission denied: cannot execute oneshot pipelines".to_string(),
        ));
    }

    // Parse multipart request to get config and media stream
    let parse_result = parse_multipart_request(req).await?;

    // Compile pipeline definition
    tracing::debug!("Compiling user pipeline definition");
    let pipeline_def: Pipeline = compile(parse_result.user_pipeline)?;
    tracing::debug!("Pipeline compilation completed");

    // Validate pipeline structure
    let (has_http_input, has_file_read, has_http_output) =
        validate_pipeline_nodes(&pipeline_def, parse_result.has_media)?;

    // Enforce allowed node/plugin kinds for oneshot execution.
    //
    // Note: `streamkit::http_input` and `streamkit::http_output` are oneshot-only marker nodes,
    // but they are not part of the general `allowed_nodes` allowlist. Treat them as implicitly
    // allowed when oneshot execution itself is permitted.
    for (node_id, node_def) in &pipeline_def.nodes {
        let kind = node_def.kind.as_str();
        if kind == "streamkit::http_input" || kind == "streamkit::http_output" {
            continue;
        }

        if !perms.is_node_allowed(kind) {
            return Err(AppError::Forbidden(format!(
                "Permission denied: node type '{kind}' not allowed (node '{node_id}')"
            )));
        }

        if kind.starts_with("plugin::") && !perms.is_plugin_allowed(kind) {
            return Err(AppError::Forbidden(format!(
                "Permission denied: plugin '{kind}' not allowed (node '{node_id}')"
            )));
        }
    }

    // Validate file paths in file-based mode
    if !parse_result.has_media {
        validate_file_reader_paths(&pipeline_def, &app_state.config.security)?;
    }

    validate_file_writer_paths(&pipeline_def, &app_state.config.security)?;
    validate_script_paths(&pipeline_def, &app_state.config.security)?;

    tracing::info!(
        "Pipeline validation passed: mode={}, has_http_input={}, has_file_read={}, has_http_output={}",
        if parse_result.has_media { "http-streaming" } else { "file-based" },
        has_http_input,
        has_file_read,
        has_http_output
    );
    tracing::info!(role = %role_name, "Executing oneshot pipeline for role");

    // Execute oneshot pipeline
    tracing::info!("Starting oneshot pipeline execution");
    let oneshot_start_time = Instant::now();
    let oneshot_duration_histogram = ONESHOT_DURATION_HISTOGRAM
        .get_or_init(|| {
            global::meter("skit_engine")
                .f64_histogram("oneshot_pipeline.duration")
                .with_description(
                    "Oneshot pipeline runtime from request start until response stream ends",
                )
                .build()
        })
        .clone();

    // Build oneshot config from server configuration
    let oneshot_config = {
        let cfg = &app_state.config.engine.oneshot;
        OneshotEngineConfig {
            packet_batch_size: cfg.packet_batch_size,
            media_channel_capacity: cfg
                .media_channel_capacity
                .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_MEDIA_CAPACITY),
            io_channel_capacity: cfg
                .io_channel_capacity
                .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_IO_CAPACITY),
        }
    };

    let pipeline_result = match app_state
        .engine
        .run_oneshot_pipeline(
            pipeline_def,
            parse_result.media_stream,
            parse_result.media_content_type,
            parse_result.has_media,
            Some(oneshot_config),
        )
        .await
    {
        Ok(result) => {
            tracing::info!("Oneshot pipeline execution completed");
            result
        },
        Err(e) => {
            let labels = [KeyValue::new("status", "error")];
            oneshot_duration_histogram.record(oneshot_start_time.elapsed().as_secs_f64(), &labels);
            return Err(e.into());
        },
    };

    // Build and return streaming response
    Ok(build_streaming_response(pipeline_result, oneshot_start_time, oneshot_duration_histogram))
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(app_state): State<Arc<AppState>>,
) -> Response {
    // Security: mitigate Cross-Site WebSocket Hijacking (CSWSH).
    //
    // Browsers always send an Origin header for WebSocket connections. If we accept
    // any Origin, any website can connect to a user's local StreamKit instance and
    // drive the control plane. Reuse the configured CORS origin allowlist.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let allowed = {
            let patterns = &app_state.config.server.cors.allowed_origins;
            patterns.iter().any(|p| origin_matches_pattern(origin, p))
        };

        if !allowed {
            warn!(origin = %origin, "Rejected WebSocket connection: Origin not allowed");
            return (StatusCode::FORBIDDEN, "WebSocket Origin not allowed").into_response();
        }
    }

    // Extract role name and permissions from headers
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
    ws.on_upgrade(move |socket| websocket::handle_websocket(socket, app_state, perms, role_name))
}

async fn static_handler(
    uri: axum::http::Uri,
    State(app_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let raw_path = uri.path();
    if raw_path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path = raw_path.trim_start_matches('/');

    // If path is empty, serve index.html
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let mut headers = HeaderMap::new();
        // MIME types from mime_guess should always be valid for HTTP headers.
        // This expect is justified: mime_guess returns standard MIME types that always parse.
        #[allow(clippy::expect_used)]
        headers.insert(
            header::CONTENT_TYPE,
            mime.as_ref().parse().expect("MIME type should always be valid header value"),
        );

        let cache_control =
            if path == "index.html" { "no-cache" } else { "public, max-age=31536000, immutable" };
        headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static(cache_control));

        // Inject <base> tag into index.html if base_path is configured
        if path == "index.html" {
            if let Some(base_path) = normalized_base_path_for_html(app_state.as_ref()) {
                let base_path = escape_html_attr(&base_path);
                let html = String::from_utf8_lossy(&content.data);
                let injected =
                    html.replace("<head>", &format!("<head>\n    <base href=\"{base_path}/\">"));
                return (headers, injected.into_bytes()).into_response();
            }
        }

        (headers, content.data).into_response()
    } else {
        if std::path::Path::new(path).extension().is_some() {
            return StatusCode::NOT_FOUND.into_response();
        }

        // For SPA routing, if the file is not found, serve index.html
        debug!(
            path = %path,
            "Static asset not found, serving index.html for client-side routing"
        );
        if let Some(content) = Assets::get("index.html") {
            let mime = mime_guess::from_path("index.html").first_or_octet_stream();
            let mut headers = HeaderMap::new();
            // MIME types from mime_guess should always be valid for HTTP headers.
            // This expect is justified: mime_guess returns standard MIME types that always parse.
            #[allow(clippy::expect_used)]
            headers.insert(
                header::CONTENT_TYPE,
                mime.as_ref().parse().expect("MIME type should always be valid header value"),
            );
            headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-cache"));

            // Inject <base> tag if base_path is configured
            if let Some(base_path) = normalized_base_path_for_html(app_state.as_ref()) {
                let base_path = escape_html_attr(&base_path);
                let html = String::from_utf8_lossy(&content.data);
                let injected =
                    html.replace("<head>", &format!("<head>\n    <base href=\"{base_path}/\">"));
                return (headers, injected.into_bytes()).into_response();
            }

            (headers, content.data).into_response()
        } else {
            error!("FATAL: index.html not found in embedded assets!");
            (StatusCode::INTERNAL_SERVER_ERROR, "index.html not found").into_response()
        }
    }
}

async fn metrics_middleware(req: axum::http::Request<Body>, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    // Extract matched path for metrics, falling back to the full URI path if no match
    let path = req.extensions().get::<MatchedPath>().map_or_else(
        || req.uri().path().to_owned(),
        |matched_path| matched_path.as_str().to_owned(),
    );

    let response = next.run(req).await;

    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    let (counter, histogram) = HTTP_METRICS
        .get_or_init(|| {
            let meter = global::meter("skit_server");
            (
                meter.u64_counter("http.server.requests").build(),
                meter.f64_histogram("http.server.duration").build(),
            )
        })
        .clone();

    let labels = [
        KeyValue::new("http.method", method.to_string()),
        KeyValue::new("http.route", path),
        KeyValue::new("http.status_code", status),
    ];

    counter.add(1, &labels);
    histogram.record(latency, &labels);

    response
}

/// Creates the Axum application with all routes and middleware.
///
/// # Panics
///
/// Panics if the plugin manager fails to initialize. This can happen if:
/// - Plugin directories cannot be created due to filesystem permissions
/// - Plugin directories exist but are not accessible
///
/// Since this occurs during application initialization, a panic here is acceptable
/// as the server cannot function without plugin support.
pub fn create_app(config: Config) -> (Router, Arc<AppState>) {
    // --- Create the shared application state ---
    let (event_tx, _) = tokio::sync::broadcast::channel(128);

    // Create ResourceManager for shared resources (ML models, etc.)
    let resource_policy = streamkit_core::ResourcePolicy {
        keep_loaded: config.resources.keep_models_loaded,
        max_memory_mb: config.resources.max_memory_mb,
    };
    let resource_manager = Arc::new(streamkit_core::ResourceManager::new(resource_policy));

    // Set node buffer configuration for codec/container nodes
    // This must be done before any nodes are created
    let node_buffer_config = streamkit_core::NodeBufferConfig {
        codec_channel_capacity: config
            .engine
            .advanced
            .codec_channel_capacity
            .unwrap_or(streamkit_engine::constants::DEFAULT_CODEC_CHANNEL_CAPACITY),
        stream_channel_capacity: config
            .engine
            .advanced
            .stream_channel_capacity
            .unwrap_or(streamkit_engine::constants::DEFAULT_STREAM_CHANNEL_CAPACITY),
        demuxer_buffer_size: config
            .engine
            .advanced
            .demuxer_buffer_size
            .unwrap_or(streamkit_engine::constants::DEFAULT_DEMUXER_BUFFER_SIZE),
        moq_peer_channel_capacity: config
            .engine
            .advanced
            .moq_peer_channel_capacity
            .unwrap_or(streamkit_engine::constants::DEFAULT_MOQ_PEER_CHANNEL_CAPACITY),
    };
    streamkit_core::set_node_buffer_config(node_buffer_config);

    // Create engine with resource management support
    let plugin_base_dir = std::path::PathBuf::from(&config.plugins.directory);
    let wasm_plugin_dir = plugin_base_dir.join("wasm");
    let native_plugin_dir = plugin_base_dir.join("native");

    // Create engine with script configuration if feature is enabled
    #[cfg(feature = "script")]
    let engine = {
        // Convert server config AllowlistRule to nodes AllowlistRule
        let global_script_allowlist = if config.script.global_fetch_allowlist.is_empty() {
            None
        } else {
            Some(
                config
                    .script
                    .global_fetch_allowlist
                    .iter()
                    .map(|rule| streamkit_nodes::core::script::AllowlistRule {
                        url: rule.url.clone(),
                        methods: rule.methods.clone(),
                    })
                    .collect(),
            )
        };

        // Load secrets from environment variables
        let secrets = load_script_secrets(&config.script.secrets);

        Arc::new(Engine::with_resource_manager_and_script_config(
            resource_manager.clone(),
            global_script_allowlist,
            secrets,
        ))
    };

    #[cfg(not(feature = "script"))]
    let engine = Arc::new(Engine::with_resource_manager(resource_manager.clone()));

    // Initialize plugin manager - panic on failure since we can't proceed without it
    // This expect is justified and documented in the function's # Panics section
    #[allow(clippy::expect_used)]
    let plugin_manager = UnifiedPluginManager::new(
        Arc::clone(&engine),
        resource_manager,
        wasm_plugin_dir,
        native_plugin_dir,
    )
    .expect("Failed to initialize unified plugin manager");
    let plugin_manager = Arc::new(tokio::sync::Mutex::new(plugin_manager));

    // Spawn background task to load plugins asynchronously to avoid blocking startup
    UnifiedPluginManager::spawn_load_existing(
        Arc::clone(&plugin_manager),
        config.resources.prewarm.clone(),
    );

    #[cfg(feature = "moq")]
    let moq_gateway = {
        let gateway = Arc::new(crate::moq_gateway::MoqGateway::new());
        // Initialize global gateway registry so nodes can access it
        let trait_obj: Arc<dyn streamkit_core::moq_gateway::MoqGatewayTrait> = gateway.clone();
        streamkit_core::moq_gateway::init_moq_gateway(trait_obj);
        Some(gateway)
    };

    let app_state = Arc::new(AppState {
        engine,
        session_manager: Arc::new(tokio::sync::Mutex::new(SessionManager::default())),
        config: Arc::new(config),
        event_tx,
        plugin_manager,
        #[cfg(feature = "moq")]
        moq_gateway,
    });

    let mut oneshot_route = post(process_oneshot_pipeline_handler)
        // Use configurable body limit for oneshot processing
        .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size));
    if let Some(max) = app_state.config.permissions.max_concurrent_oneshots {
        oneshot_route = oneshot_route.layer(ConcurrencyLimitLayer::new(max));
    }

    #[cfg_attr(not(feature = "moq"), allow(unused_mut))]
    let mut router = Router::new()
        .route("/healthz", get(health_handler))
        .route("/health", get(health_handler))
        .route("/api/v1/process", oneshot_route)
        .route(
            "/api/v1/plugins",
            get(list_plugins_handler)
                .post(upload_plugin_handler)
                // Plugin uploads are multipart; raise default body limit for realistic artifacts.
                .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size)),
        )
        .route("/api/v1/plugins/{kind}", delete(delete_plugin_handler))
        .route("/api/v1/control", get(websocket_handler))
        .route("/api/v1/permissions", get(get_permissions_handler))
        .route("/api/v1/config", get(get_config_handler))
        .route("/api/v1/schema/nodes", get(list_node_definitions_handler))
        .route("/api/v1/schema/packets", get(list_packet_types_handler))
        .route("/api/v1/sessions", get(list_sessions_handler).post(create_session_handler))
        .route("/api/v1/sessions/{id}", delete(destroy_session_handler))
        .route("/api/v1/sessions/{id}/pipeline", get(get_pipeline_handler))
        .route(
            "/api/v1/profile/cpu",
            get({
                #[cfg(feature = "profiling")]
                {
                    profile_cpu_handler
                }
                #[cfg(not(feature = "profiling"))]
                {
                    profiling::profile_cpu
                }
            }),
        )
        .route(
            "/api/v1/profile/heap",
            get({
                #[cfg(feature = "profiling")]
                {
                    profile_heap_handler
                }
                #[cfg(not(feature = "profiling"))]
                {
                    profiling::profile_heap
                }
            }),
        )
        .merge(crate::samples::samples_router())
        .merge(crate::assets::assets_router());

    // Add MoQ routes if feature is enabled
    #[cfg(feature = "moq")]
    {
        router = router.route("/api/v1/moq/fingerprints", get(get_moq_fingerprints_handler));
        router = router.route("/certificate.sha256", get(get_certificate_sha256_handler));
    }

    let cors_layer = create_cors_layer(&app_state.config.server.cors);

    let router = router.fallback(static_handler);

    // If server.base_path is set (e.g. "/s/session_xxx"), serve the entire app under that
    // prefix too. This makes subpath deployments work even without a reverse-proxy rewrite.
    let base_path = app_state
        .config
        .server
        .base_path
        .as_deref()
        .map(str::trim)
        .and_then(|p| if p.is_empty() { None } else { Some(p) })
        .map(|p| p.trim_end_matches('/'))
        .and_then(|p| if p == "/" { None } else { Some(p) })
        .map(|p| if p.starts_with('/') { p.to_string() } else { format!("/{p}") });

    let router = if let Some(base_path) = base_path {
        let cloned = router.clone();
        router.nest(&base_path, cloned)
    } else {
        router
    };

    let router = router
        .with_state(Arc::clone(&app_state))
        .layer(middleware::from_fn_with_state(Arc::clone(&app_state), origin_guard_middleware))
        .layer(ServiceBuilder::new().layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    let route = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map_or_else(|| request.uri().path(), |matched| matched.as_str());
                    tracing::info_span!("http_request", http_method = %request.method(), http_route = %route)
                })
                // Keep per-request logs out of INFO hot paths; surface failures at WARN.
                .on_response(DefaultOnResponse::new().level(tracing::Level::DEBUG))
                .on_failure(DefaultOnFailure::new().level(tracing::Level::WARN)),
        ))
    .layer(middleware::from_fn(metrics_middleware))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::HeaderName::from_static("referrer-policy"),
        header::HeaderValue::from_static("no-referrer"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("SAMEORIGIN"),
    ))
    .layer(cors_layer);

    (router, app_state)
}

#[cfg(feature = "moq")]
#[allow(clippy::unused_async)]
fn start_moq_webtransport_acceptor(
    app_state: &Arc<AppState>,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    use moq_native::{ServerConfig as MoqServerConfig, ServerTlsConfig};

    let gateway = if let Some(gw) = &app_state.moq_gateway {
        Arc::clone(gw)
    } else {
        warn!("MoQ gateway not initialized, skipping WebTransport acceptor");
        return Ok(());
    };

    // Parse address for WebTransport (UDP will use the same port as HTTP/HTTPS)
    let addr: SocketAddr = config.server.address.parse()?;

    // Configure TLS - use provided certificates if available, otherwise auto-generate
    let tls = if config.server.tls
        && !config.server.cert_path.is_empty()
        && !config.server.key_path.is_empty()
    {
        info!(
            cert_path = %config.server.cert_path,
            key_path = %config.server.key_path,
            "Using provided TLS certificates for MoQ WebTransport"
        );
        ServerTlsConfig {
            cert: vec![std::path::PathBuf::from(&config.server.cert_path)],
            key: vec![std::path::PathBuf::from(&config.server.key_path)],
            generate: vec![],
        }
    } else {
        info!("Auto-generating self-signed certificate for MoQ WebTransport (14-day validity for local development)");
        ServerTlsConfig { cert: vec![], key: vec![], generate: vec!["localhost".to_string()] }
    };

    let moq_config = MoqServerConfig { bind: Some(addr), tls };

    info!(
        address = %addr,
        "Starting MoQ WebTransport acceptor on UDP (same port as HTTP server)"
    );

    tokio::spawn(async move {
        match moq_config.init() {
            Ok(mut server) => {
                // Store fingerprints in gateway for HTTP endpoint
                let fingerprints = server.fingerprints().to_vec();
                gateway.set_fingerprints(fingerprints.clone()).await;

                for (i, fp) in fingerprints.iter().enumerate() {
                    info!("🔐 MoQ WebTransport certificate fingerprint #{}: {}", i + 1, fp);
                }
                info!("💡 Access fingerprints at: http://{}/api/v1/moq/fingerprints", addr);

                info!("MoQ WebTransport server listening for connections");

                // Accept connections in a loop
                while let Some(request) = server.accept().await {
                    let gateway = Arc::clone(&gateway);

                    tokio::spawn(async move {
                        match request {
                            moq_native::Request::WebTransport(wt_request) => {
                                let path = wt_request.url().path().to_string();
                                debug!(path = %path, "Received WebTransport connection request");

                                match wt_request.ok().await {
                                    Ok(session) => {
                                        if let Err(e) =
                                            gateway.accept_connection(session, path.clone()).await
                                        {
                                            warn!(path = %path, error = %e, "Failed to route WebTransport connection");
                                        }
                                    },
                                    Err(e) => {
                                        warn!(path = %path, error = %e, "Failed to accept WebTransport session");
                                    },
                                }
                            },
                            moq_native::Request::Quic(_quic_request) => {
                                debug!("Received raw QUIC connection (not WebTransport), ignoring");
                            },
                        }
                    });
                }

                info!("MoQ WebTransport server stopped accepting connections");
            },
            Err(e) => {
                error!(error = %e, "Failed to initialize MoQ WebTransport server");
            },
        }
    });

    Ok(())
}

/// Starts the HTTP/HTTPS server and optional MoQ WebTransport acceptor.
///
/// # Errors
///
/// Returns an error if:
/// - The server address cannot be parsed
/// - TLS is enabled but certificates cannot be loaded
/// - The server fails to bind to the specified address
/// - The server encounters a runtime error
///
/// # Panics
///
/// Panics if:
/// - The Ctrl+C signal handler cannot be installed (critical OS failure)
/// - The SIGTERM signal handler cannot be installed on Unix systems (critical OS failure)
/// - The plugin manager fails to initialize (via `create_app`)
pub async fn start_server(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let (app, app_state) = create_app(config.clone());
    #[cfg(not(feature = "moq"))]
    let _ = &app_state;

    let addr: SocketAddr = config.server.address.parse()?;
    if !addr.ip().is_loopback() && config.permissions.role_header.is_none() {
        if !config.permissions.allow_insecure_no_auth {
            return Err(format!(
                "Refusing to start: server.address is '{addr}' (non-loopback) but permissions.role_header is not set. \
                 StreamKit does not implement authentication; without a trusted auth layer, all requests fall back to SK_ROLE/default_role ('{}'). \
                 Fix: put StreamKit behind an authenticating reverse proxy and set permissions.role_header, or (unsafe) set permissions.allow_insecure_no_auth = true to override.",
                config.permissions.default_role
            )
            .into());
        }
        warn!(
            address = %addr,
            default_role = %config.permissions.default_role,
            allow_http_management = config.plugins.allow_http_management,
            "Starting without a trusted role header on a non-loopback address; all requests fall back to SK_ROLE/default_role. \
             This is unsafe unless the server is only reachable by trusted clients."
        );
    }

    // Start MoQ WebTransport acceptor if feature is enabled
    #[cfg(feature = "moq")]
    start_moq_webtransport_acceptor(&app_state, config)?;

    // Set up graceful shutdown signal handler
    // These expect() calls are justified and documented in the function's # Panics section
    #[allow(clippy::expect_used)]
    let shutdown_signal = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install signal handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => {
                info!("Received CTRL-C signal, initiating graceful shutdown");
            },
            () = terminate => {
                info!("Received SIGTERM signal, initiating graceful shutdown");
            },
        }
    };

    if config.server.tls {
        if config.server.cert_path.is_empty() || config.server.key_path.is_empty() {
            return Err("TLS is enabled but cert_path or key_path is not configured".into());
        }

        info!(
            address = %addr,
            cert_path = %config.server.cert_path,
            key_path = %config.server.key_path,
            "Starting HTTPS API server"
        );

        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            &config.server.cert_path,
            &config.server.key_path,
        )
        .await
        .map_err(|e| {
            error!(
                error = %e,
                cert_path = %config.server.cert_path,
                key_path = %config.server.key_path,
                "Failed to load TLS certificates"
            );
            e
        })?;

        let handle = axum_server::Handle::new();

        // Spawn a task to listen for shutdown signal
        tokio::spawn({
            let handle = handle.clone();
            async move {
                shutdown_signal.await;
                handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
            }
        });

        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .map_err(|e| {
                error!(error = %e, "API server error");
                e.into()
            })
    } else {
        info!(address = %addr, "Starting HTTP API server");

        let handle = axum_server::Handle::new();

        // Spawn a task to listen for shutdown signal
        tokio::spawn({
            let handle = handle.clone();
            async move {
                shutdown_signal.await;
                handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
            }
        });

        axum_server::bind(addr).handle(handle).serve(app.into_make_service()).await.map_err(|e| {
            error!(error = %e, "API server error");
            e.into()
        })
    }
}

// --- A simple error type for the Axum handler ---
#[derive(Debug)]
enum AppError {
    Engine(StreamKitError),
    Multipart(MultipartError),
    Serde(serde_saphyr::Error),
    PipelineCompilation(String),
    BadRequest(String),
    Forbidden(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::Engine(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Pipeline execution error: {e}"))
            },
            Self::Multipart(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Error receiving request data: {e}"))
            },
            Self::Serde(e) => (StatusCode::BAD_REQUEST, format!("Invalid YAML config format: {e}")),
            Self::PipelineCompilation(e) => {
                (StatusCode::BAD_REQUEST, format!("Invalid pipeline: {e}"))
            },
            Self::BadRequest(e) => (StatusCode::BAD_REQUEST, e),
            Self::Forbidden(e) => (StatusCode::FORBIDDEN, e),
        };
        (status, msg).into_response()
    }
}

// Boilerplate to convert errors from other libraries into our AppError
impl From<StreamKitError> for AppError {
    fn from(e: StreamKitError) -> Self {
        Self::Engine(e)
    }
}
impl From<MultipartError> for AppError {
    fn from(e: MultipartError) -> Self {
        Self::Multipart(e)
    }
}
impl From<serde_saphyr::Error> for AppError {
    fn from(e: serde_saphyr::Error) -> Self {
        Self::Serde(e)
    }
}
impl From<String> for AppError {
    fn from(e: String) -> Self {
        Self::PipelineCompilation(e)
    }
}

#[derive(Debug)]
enum PluginHttpError {
    BadRequest(String),
    Forbidden(String),
    Multipart(MultipartError),
    Manager(AnyhowError),
}

impl IntoResponse for PluginHttpError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg).into_response(),
            Self::Multipart(err) => {
                error!(error = %err, "Multipart error processing plugin request");
                (StatusCode::BAD_REQUEST, format!("Invalid multipart payload: {err}"))
                    .into_response()
            },
            Self::Manager(err) => {
                error!(error = %err, "Plugin manager error");
                (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()).into_response()
            },
        }
    }
}

impl From<MultipartError> for PluginHttpError {
    fn from(err: MultipartError) -> Self {
        Self::Multipart(err)
    }
}

impl From<AnyhowError> for PluginHttpError {
    fn from(err: AnyhowError) -> Self {
        Self::Manager(err)
    }
}
