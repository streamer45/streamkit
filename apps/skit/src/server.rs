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
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};
use tower::limit::ConcurrencyLimitLayer;
use tower::ServiceBuilder;
use tower_http::{
    cors::{AllowHeaders, AllowOrigin, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::{debug, error, info, warn};

use crate::file_security;
use crate::marketplace_installer::InstallPluginRequest;
use crate::marketplace_security::{origin_key, MarketplaceUrlPolicy, OriginKey};
use crate::plugin_paths;
use crate::plugin_records::{
    active_dir as plugin_active_dir, namespaced_kind as active_namespaced_kind, ActivePluginRecord,
};
use crate::plugins::UnifiedPluginManager;
use crate::profiling;
use crate::state::AppState;
use crate::websocket;
use streamkit_api::yaml::{compile, UserPipeline};
use streamkit_api::Pipeline;
use streamkit_api::{ApiPipeline, Event as ApiEvent, EventPayload, MessageType};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_engine::{Engine, OneshotEngineConfig, OneshotInput};

use crate::session::SessionManager;

use crate::config::Config;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use anyhow::{Context, Error as AnyhowError};
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

fn build_hash() -> &'static str {
    option_env!("SKIT_BUILD_HASH").unwrap_or("unknown")
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "build_hash": build_hash(),
    }))
}

/// Serve the public JWKS (JSON Web Key Set) for verifying StreamKit-issued JWTs.
///
/// Exposed at `/.well-known/jwks.json` when built-in auth is enabled.
async fn jwks_handler(State(app_state): State<Arc<AppState>>) -> Response {
    if !app_state.auth.is_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(key_provider) = app_state.auth.key_provider() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Auth key provider not available".to_string())
            .into_response();
    };

    Json(key_provider.jwks()).into_response()
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

const BUILTIN_AUTH_ROLE_HEADER: &str = "x-streamkit-role";

fn normalize_base_path(base_path: Option<&str>) -> Option<String> {
    base_path
        .map(str::trim)
        .and_then(|p| if p.is_empty() { None } else { Some(p) })
        .map(|p| p.trim_end_matches('/'))
        .and_then(|p| if p == "/" { None } else { Some(p) })
        .map(|p| if p.starts_with('/') { p.to_string() } else { format!("/{p}") })
}

fn normalized_base_path_for_html(app_state: &AppState) -> String {
    normalize_base_path(app_state.config.server.base_path.as_deref()).unwrap_or_default()
}

fn strip_base_path_prefix<'a>(path: &'a str, base_path: Option<&str>) -> &'a str {
    let Some(base_path) = base_path else {
        return path;
    };

    let base_path = base_path.trim().trim_end_matches('/');
    if base_path.is_empty() || base_path == "/" {
        return path;
    }

    // Normalize matching: config may specify base_path with or without a leading '/'.
    if base_path.starts_with('/') {
        let Some(rest) = path.strip_prefix(base_path) else {
            return path;
        };

        if rest.is_empty() {
            return "/";
        }

        // Only treat this as a base_path prefix if it ends on a boundary ("/" or exact match).
        if rest.starts_with('/') {
            return rest;
        }

        return path;
    }

    // base_path without leading slash: match against path after the initial '/'
    if !path.starts_with('/') {
        return path;
    }

    let Some(rest) = path[1..].strip_prefix(base_path) else {
        return path;
    };

    if rest.is_empty() {
        return "/";
    }

    // Only treat this as a base_path prefix if it ends on a boundary ("/" or exact match).
    if rest.starts_with('/') {
        rest
    } else {
        path
    }
}

async fn auth_guard_middleware(
    State(app_state): State<Arc<AppState>>,
    mut req: axum::http::Request<Body>,
    next: Next,
) -> Response {
    if !app_state.auth.is_enabled() {
        return next.run(req).await;
    }

    let raw_path = req.uri().path();
    let path = strip_base_path_prefix(raw_path, app_state.config.server.base_path.as_deref());

    // Only guard API routes; static assets (UI) stay public and handle auth via /login.
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }

    // Auth endpoints handle their own auth semantics (login/me/logout).
    if path.starts_with("/api/v1/auth/") {
        return next.run(req).await;
    }

    let auth_ctx = match crate::auth::validate_token_from_headers(
        req.headers(),
        &app_state.auth,
        &app_state.config,
        &app_state.config.permissions,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Inject the role into a trusted header so existing handlers can use RBAC without refactors.
    //
    // SECURITY: Always overwrite any incoming header of the same name.
    let Ok(role_value) = header::HeaderValue::from_str(&auth_ctx.role) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid role in token".to_string())
            .into_response();
    };
    // Header name is static and guaranteed to be valid.
    #[allow(clippy::expect_used)]
    let header_name = header::HeaderName::from_static(BUILTIN_AUTH_ROLE_HEADER);
    req.headers_mut().insert(header_name, role_value);

    next.run(req).await
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

    let raw_path = req.uri().path();
    let path = strip_base_path_prefix(raw_path, app_state.config.server.base_path.as_deref());
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

fn create_cors_layer(
    config: &crate::config::CorsConfig,
    auth_enabled: bool,
) -> Result<CorsLayer, String> {
    use axum::http::{HeaderValue, Method};

    let has_wildcard = config.allowed_origins.iter().any(|o| o == "*");

    // CRITICAL: Wildcard origins not allowed with credentials (browsers reject this)
    if auth_enabled && has_wildcard {
        return Err(
            "CORS allowed_origins='*' is incompatible with auth (cookies require explicit origins). \
             Set allowed_origins to specific origins or disable auth.".to_string()
        );
    }

    if has_wildcard {
        info!("CORS configured to allow all origins (reflect Origin header)");
        // When credentials are enabled, `Access-Control-Allow-Origin: *` is invalid.
        // Mirror the request origin instead.
        return Ok(CorsLayer::new()
            .allow_origin(AllowOrigin::mirror_request())
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
                Method::PATCH,
            ])
            .allow_headers(AllowHeaders::mirror_request())
            .allow_credentials(true));
    }

    // If no origins specified, use default restrictive behavior
    if config.allowed_origins.is_empty() {
        info!("CORS configured with no allowed origins (most restrictive)");
        return Ok(CorsLayer::new());
    }

    // Build list of patterns for matching
    let patterns: Vec<String> = config.allowed_origins.clone();

    info!(
        allowed_origins = ?patterns,
        auth_enabled,
        "CORS configured with origin allowlist"
    );

    // Create a predicate-based allowlist
    let allow_origin = AllowOrigin::predicate(move |origin: &HeaderValue, _request_parts| {
        let Ok(origin_str) = origin.to_str() else {
            return false;
        };

        patterns.iter().any(|pattern| origin_matches_pattern(origin_str, pattern))
    });

    let mut layer = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        // When credentials are enabled, wildcard headers (`*`) are invalid. Mirror the
        // preflight request headers instead.
        .allow_headers(AllowHeaders::mirror_request());

    // Enable credentials for browser clients.
    //
    // NOTE: The UI always uses `credentials: 'include'` so cookie auth works without query params.
    // In dev (Vite), the UI is served cross-origin and talks directly to the backend, so CORS must
    // allow credentials even when auth is disabled (otherwise browsers will block responses).
    layer = layer.allow_credentials(true);

    Ok(layer)
}

fn cors_allowed_origins_are_loopback_only(origins: &[String]) -> bool {
    if origins.is_empty() {
        return false;
    }

    origins.iter().all(|pattern| {
        origin_matches_pattern("http://localhost:80", pattern)
            || origin_matches_pattern("http://127.0.0.1:80", pattern)
            || origin_matches_pattern("https://localhost:443", pattern)
            || origin_matches_pattern("https://127.0.0.1:443", pattern)
    })
}

#[cfg(test)]
mod cors_tests {
    use super::{create_cors_layer, origin_matches_pattern};

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

    #[test]
    #[allow(clippy::unwrap_used)]
    fn cors_layer_does_not_panic_when_credentials_enabled() {
        let cors_config = crate::config::CorsConfig::default();
        let layer = create_cors_layer(&cors_config, false).unwrap();

        // `CorsLayer` validates its configuration when layered; this should not panic.
        let _app = axum::Router::<()>::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(layer);
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
///
/// Viewer role is denied - they cannot access server configuration.
async fn get_config_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Check auth and deny viewers
    if app_state.auth.is_enabled() {
        let auth_ctx = crate::auth::validate_token_from_headers(
            &headers,
            &app_state.auth,
            &app_state.config,
            &app_state.config.permissions,
        )
        .await?;

        if auth_ctx.role == "viewer" {
            return Err((StatusCode::FORBIDDEN, "Viewers cannot access config".to_string()));
        }
    } else {
        // Auth disabled - still check role for viewer restriction
        let (role, _) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
        if role == "viewer" {
            return Err((StatusCode::FORBIDDEN, "Viewers cannot access config".to_string()));
        }
    }

    let config = FrontendConfig {
        #[cfg(feature = "moq")]
        moq_gateway_url: app_state.config.server.moq_gateway_url.clone(),
    };

    Ok(Json(config))
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
    if !app_state.config.plugins.http_management.allow_http_management {
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
    let mut declared_kind: Option<String> = None;

    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or("").to_string();
        if name == "kind" {
            if declared_kind.is_some() {
                return Err(PluginHttpError::BadRequest(
                    "Multiple 'kind' fields provided".to_string(),
                ));
            }
            let value = field.text().await.map_err(|e| {
                PluginHttpError::BadRequest(format!("Failed to read 'kind' field: {e}"))
            })?;
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(PluginHttpError::BadRequest(
                    "Plugin kind must not be empty".to_string(),
                ));
            }
            declared_kind = Some(value);
            continue;
        }

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

    let extension = std::path::Path::new(&file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let placeholder_kind = match extension {
        "wasm" => Some("plugin::wasm::placeholder"),
        "so" | "dylib" | "dll" => Some("plugin::native::placeholder"),
        _ => None,
    };

    if let Some(kind) = declared_kind.as_ref() {
        if let Some(placeholder) = placeholder_kind {
            let expected_prefix = if placeholder.starts_with("plugin::wasm::") {
                "plugin::wasm::"
            } else {
                "plugin::native::"
            };
            if !kind.starts_with(expected_prefix) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(PluginHttpError::BadRequest(format!(
                    "Declared plugin kind '{kind}' does not match uploaded file type"
                )));
            }
        }
        if !perms.is_plugin_allowed(kind) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::Forbidden(format!(
                "Permission denied: plugin '{kind}' not allowed"
            )));
        }
    } else if let Some(placeholder) = placeholder_kind {
        if !perms.is_plugin_allowed(placeholder) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::BadRequest(
                "Plugin kind must be provided when allowed_plugins is restricted".to_string(),
            ));
        }
    }

    let summary = match tokio::task::spawn_blocking({
        let manager = Arc::clone(&app_state.plugin_manager);
        let file_name = file_name.clone();
        let tmp_path = tmp_path.clone();
        move || {
            let mut mgr = manager.blocking_lock();
            mgr.load_from_temp_file(&file_name, &tmp_path)
        }
    })
    .await
    {
        Ok(Ok(summary)) => summary,
        Ok(Err(err)) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::from(err));
        },
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::BadRequest(format!("Plugin load task failed: {err}")));
        },
    };

    // Check if the loaded plugin is allowed
    if !perms.is_plugin_allowed(&summary.kind) {
        let _ = tokio::task::spawn_blocking({
            let manager = Arc::clone(&app_state.plugin_manager);
            let kind = summary.kind.clone();
            move || {
                let mut mgr = manager.blocking_lock();
                let _ = mgr.unload_plugin(&kind, true);
            }
        })
        .await;

        return Err(PluginHttpError::Forbidden(format!(
            "Permission denied: plugin '{}' not allowed",
            summary.kind
        )));
    }

    if let Some(kind) = declared_kind.as_ref() {
        if summary.kind != *kind {
            let _ = tokio::task::spawn_blocking({
                let manager = Arc::clone(&app_state.plugin_manager);
                let summary_kind = summary.kind.clone();
                move || {
                    let mut mgr = manager.blocking_lock();
                    let _ = mgr.unload_plugin(&summary_kind, true);
                }
            })
            .await;

            return Err(PluginHttpError::BadRequest(format!(
                "Uploaded plugin kind '{}' does not match declared kind '{}'",
                summary.kind, kind
            )));
        }
    }

    Ok((StatusCode::CREATED, Json(summary)))
}

#[derive(Debug, Serialize)]
struct InstallPluginResponse {
    job_id: String,
}

async fn install_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<InstallPluginRequest>,
) -> Result<impl IntoResponse, PluginHttpError> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err(PluginHttpError::Forbidden(
            "Marketplace installs are disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot install plugins".to_string(),
        ));
    }

    if !app_state.marketplace_jobs.is_registry_configured(&request.registry) {
        return Err(PluginHttpError::BadRequest(format!(
            "Registry '{registry}' is not configured",
            registry = request.registry
        )));
    }

    let job_id = app_state.marketplace_jobs.enqueue(request, perms).await;
    Ok((StatusCode::ACCEPTED, Json(InstallPluginResponse { job_id })))
}

#[derive(Debug, Serialize)]
struct MarketplaceRegistry {
    id: String,
    url: String,
}

async fn list_marketplace_registries_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registries = app_state.marketplace_jobs.registries();
    let payload: Vec<MarketplaceRegistry> =
        registries.into_iter().map(|url| MarketplaceRegistry { id: url.clone(), url }).collect();
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
struct MarketplacePluginsQuery {
    registry: String,
    q: Option<String>,
}

async fn validate_marketplace_registry_url(
    config: &crate::config::PluginConfig,
    registry: &str,
) -> anyhow::Result<(MarketplaceUrlPolicy, reqwest::Url, OriginKey)> {
    let policy = MarketplaceUrlPolicy::from_config(config);
    let registry_url = policy.validate_url("registry index", registry, None).await?;
    let registry_origin = origin_key(&registry_url)?;
    Ok((policy, registry_url, registry_origin))
}

async fn validate_marketplace_plugin_urls(
    policy: &MarketplaceUrlPolicy,
    registry_origin: &OriginKey,
    version: &crate::marketplace::RegistryPluginVersion,
) -> anyhow::Result<(reqwest::Url, reqwest::Url)> {
    let manifest_url =
        policy.validate_url("manifest url", &version.manifest_url, Some(registry_origin)).await?;
    let signature_url_value = version
        .signature_url
        .as_deref()
        .map_or_else(|| format!("{}.minisig", manifest_url.as_str()), str::to_string);
    let signature_url =
        policy.validate_url("signature url", &signature_url_value, Some(registry_origin)).await?;
    Ok((manifest_url, signature_url))
}

#[cfg(test)]
mod marketplace_url_tests {
    use super::{validate_marketplace_plugin_urls, validate_marketplace_registry_url};
    use anyhow::{bail, Result};

    #[tokio::test]
    async fn browsing_rejects_cross_origin_manifest() -> Result<()> {
        let mut config = crate::config::PluginConfig::default();
        config.marketplace.security.marketplace_require_registry_origin = true;
        let (policy, _registry_url, registry_origin) =
            validate_marketplace_registry_url(&config, "https://registry.example.com/index.json")
                .await?;

        let version = crate::marketplace::RegistryPluginVersion {
            version: "1.0.0".to_string(),
            manifest_url: "https://evil.example.com/manifest.json".to_string(),
            signature_url: Some("https://registry.example.com/manifest.minisig".to_string()),
            published_at: None,
        };

        match validate_marketplace_plugin_urls(&policy, &registry_origin, &version).await {
            Ok(_) => bail!("expected origin rejection"),
            Err(err) => assert!(err.to_string().contains("origin")),
        }

        Ok(())
    }
}

async fn list_marketplace_plugins_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MarketplacePluginsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registry = query.registry.trim();
    if registry.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Registry is required".to_string()));
    }
    if !app_state.marketplace_jobs.is_registry_configured(registry) {
        return Err((StatusCode::BAD_REQUEST, format!("Registry '{registry}' is not configured")));
    }

    let (policy, registry_url, registry_origin) =
        validate_marketplace_registry_url(&app_state.config.plugins, registry)
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, format!("Registry URL rejected: {err}")))?;

    let registry_client = app_state.marketplace_jobs.registry_client();
    let mut index = registry_client
        .fetch_index_with_policy(&registry_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch registry index: {err}"))
        })?;

    let filter = query.q.as_deref().map(str::trim).filter(|val| !val.is_empty());
    if let Some(filter) = filter {
        let filter = filter.to_lowercase();
        index.plugins.retain(|plugin| marketplace_plugin_matches(plugin, &filter));
    }

    Ok(Json(index))
}

fn marketplace_plugin_matches(plugin: &crate::marketplace::RegistryPlugin, filter: &str) -> bool {
    if plugin.id.to_lowercase().contains(filter) {
        return true;
    }
    if let Some(name) = plugin.name.as_ref() {
        if name.to_lowercase().contains(filter) {
            return true;
        }
    }
    if let Some(description) = plugin.description.as_ref() {
        if description.to_lowercase().contains(filter) {
            return true;
        }
    }
    false
}

#[derive(Debug, Deserialize)]
struct MarketplacePluginQuery {
    registry: String,
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarketplaceSignatureStatus {
    verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarketplacePluginDetails {
    registry: String,
    plugin: crate::marketplace::RegistryPlugin,
    version: crate::marketplace::RegistryPluginVersion,
    manifest: crate::marketplace::PluginManifest,
    signature: MarketplaceSignatureStatus,
    allow_native_marketplace: bool,
}

async fn get_marketplace_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(plugin_id): Path<String>,
    Query(query): Query<MarketplacePluginQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registry = query.registry.trim();
    if registry.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Registry is required".to_string()));
    }
    if !app_state.marketplace_jobs.is_registry_configured(registry) {
        return Err((StatusCode::BAD_REQUEST, format!("Registry '{registry}' is not configured")));
    }

    let plugin_id = plugin_id.trim();
    if plugin_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Plugin id is required".to_string()));
    }
    if let Err(err) = plugin_paths::validate_path_component("plugin id", plugin_id) {
        return Err((StatusCode::BAD_REQUEST, err.to_string()));
    }

    let (policy, registry_url, registry_origin) =
        validate_marketplace_registry_url(&app_state.config.plugins, registry)
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, format!("Registry URL rejected: {err}")))?;

    let registry_client = app_state.marketplace_jobs.registry_client();
    let signature_verifier = app_state.marketplace_jobs.verifier();
    let index = registry_client
        .fetch_index_with_policy(&registry_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch registry index: {err}"))
        })?;

    let plugin =
        index.plugins.into_iter().find(|plugin| plugin.id == plugin_id).ok_or_else(|| {
            (StatusCode::NOT_FOUND, format!("Plugin '{plugin_id}' not found in registry"))
        })?;

    let requested_version = query.version.as_deref().map(str::trim).filter(|val| !val.is_empty());
    let version_entry = if let Some(requested_version) = requested_version {
        plugin.versions.iter().find(|version| version.version == requested_version)
    } else if let Some(latest) = plugin.latest.as_ref() {
        plugin
            .versions
            .iter()
            .find(|version| version.version == *latest)
            .or_else(|| plugin.versions.first())
    } else {
        plugin.versions.first()
    }
    .cloned()
    .ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("No versions available for plugin '{plugin_id}'"))
    })?;

    let (manifest_url, signature_url) =
        validate_marketplace_plugin_urls(&policy, &registry_origin, &version_entry).await.map_err(
            |err| (StatusCode::BAD_GATEWAY, format!("Registry returned invalid URLs: {err}")),
        )?;

    let manifest_entry = registry_client
        .fetch_manifest_raw_with_policy(&manifest_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch plugin manifest: {err}"))
        })?;

    let signature = match registry_client
        .fetch_text_with_policy("signature url", &signature_url, &policy, &registry_origin)
        .await
    {
        Ok(signature_text) => {
            match signature_verifier.verify(manifest_entry.bytes.as_ref(), &signature_text) {
                Ok(verified_signature) => MarketplaceSignatureStatus {
                    verified: true,
                    key_id: Some(verified_signature.key_id),
                    error: None,
                },
                Err(err) => MarketplaceSignatureStatus {
                    verified: false,
                    key_id: None,
                    error: Some(err.to_string()),
                },
            }
        },
        Err(err) => MarketplaceSignatureStatus {
            verified: false,
            key_id: None,
            error: Some(err.to_string()),
        },
    };

    Ok(Json(MarketplacePluginDetails {
        registry: registry.to_string(),
        plugin,
        version: version_entry,
        manifest: manifest_entry.manifest,
        signature,
        allow_native_marketplace: app_state.config.plugins.marketplace.allow_native_marketplace,
    }))
}

async fn find_active_record_for_kind(
    plugin_dir: &std::path::Path,
    kind: &str,
) -> Option<(std::path::PathBuf, ActivePluginRecord)> {
    let active_dir = plugin_active_dir(plugin_dir);
    let mut entries = match tokio::fs::read_dir(&active_dir).await {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %err, dir = ?active_dir, "Failed to read active plugin records");
            }
            return None;
        },
    };

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, dir = ?active_dir, "Failed to iterate active plugin records");
                break;
            },
        };

        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(error = %err, file = ?path, "Failed to read active plugin record");
                continue;
            },
        };
        let record: ActivePluginRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(err) => {
                warn!(error = %err, file = ?path, "Failed to parse active plugin record");
                continue;
            },
        };
        if active_namespaced_kind(&record) == kind {
            return Some((path, record));
        }
    }

    None
}

async fn remove_active_record_and_bundle(
    plugin_dir: &std::path::Path,
    record_path: &std::path::Path,
    record: &ActivePluginRecord,
) -> Result<(), anyhow::Error> {
    tokio::fs::remove_file(record_path).await.with_context(|| {
        format!("Failed to remove active record {record_path}", record_path = record_path.display())
    })?;

    let base_real = plugin_paths::canonicalize_existing_dir(plugin_dir).await?;
    plugin_paths::validate_path_component("plugin id", &record.plugin_id)?;
    plugin_paths::validate_path_component("plugin version", &record.version)?;

    let bundles_root = plugin_dir.join("bundles").join(&record.plugin_id);
    let bundle_dir = bundles_root.join(&record.version);
    if tokio::fs::try_exists(&bundle_dir).await.unwrap_or(false) {
        let bundle_dir_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &bundle_dir, "bundle").await?;
        tokio::fs::remove_dir_all(&bundle_dir_real).await.with_context(|| {
            format!(
                "Failed to remove bundle directory {bundle_dir_real}",
                bundle_dir_real = bundle_dir_real.display()
            )
        })?;
    }

    if tokio::fs::try_exists(&bundles_root).await.unwrap_or(false) {
        let bundles_root_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &bundles_root, "bundles").await?;
        let mut entries = tokio::fs::read_dir(&bundles_root_real).await?;
        if entries.next_entry().await?.is_none() {
            let _ = tokio::fs::remove_dir(&bundles_root_real).await;
        }
    }

    let cache_root = plugin_dir.join("cache").join(&record.plugin_id);
    if tokio::fs::try_exists(&cache_root).await.unwrap_or(false) {
        let cache_root_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &cache_root, "cache").await?;
        let _ = tokio::fs::remove_dir_all(&cache_root_real).await;
    }

    Ok(())
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
    if !app_state.config.plugins.http_management.allow_http_management {
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

    let plugin_dir = std::path::PathBuf::from(&app_state.config.plugins.directory);
    let active_record = find_active_record_for_kind(&plugin_dir, &kind).await;

    info!(plugin_kind = %kind, keep_file = query.keep_file, "Deleting plugin");
    let summary = match tokio::task::spawn_blocking({
        let manager = Arc::clone(&app_state.plugin_manager);
        let kind = kind.clone();
        let remove_file = !query.keep_file;
        move || {
            let mut mgr = manager.blocking_lock();
            mgr.unload_plugin(&kind, remove_file)
        }
    })
    .await
    {
        Ok(Ok(summary)) => summary,
        Ok(Err(err)) => return Err(PluginHttpError::from(err)),
        Err(err) => {
            return Err(PluginHttpError::BadRequest(format!("Plugin unload task failed: {err}")));
        },
    };

    if let Some((record_path, record)) = active_record {
        if query.keep_file {
            info!(
                plugin_id = %record.plugin_id,
                version = %record.version,
                "Kept marketplace bundle and active record for unloaded plugin"
            );
        } else if let Err(err) =
            remove_active_record_and_bundle(&plugin_dir, &record_path, &record).await
        {
            return Err(PluginHttpError::BadRequest(format!(
                "Failed to uninstall marketplace plugin: {err}"
            )));
        }
    }

    Ok(Json(summary))
}

async fn get_job_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot view jobs".to_string()));
    }

    app_state
        .marketplace_jobs
        .get_job(&job_id)
        .await
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Job '{job_id}' not found")))
}

async fn cancel_job_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot cancel jobs".to_string()));
    }

    app_state
        .marketplace_jobs
        .cancel_job(&job_id)
        .await
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Job '{job_id}' not found")))
}

async fn list_packet_types_handler() -> impl IntoResponse {
    let registry = streamkit_core::packet_meta::packet_type_registry();
    Json(registry)
}

/// Axum handler to get MoQ WebTransport certificate fingerprints
///
/// Viewer role is denied - fingerprints are sensitive for MoQ connections.
#[cfg(feature = "moq")]
async fn get_moq_fingerprints_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Check auth and deny viewers
    if app_state.auth.is_enabled() {
        let auth_ctx = crate::auth::validate_token_from_headers(
            &headers,
            &app_state.auth,
            &app_state.config,
            &app_state.config.permissions,
        )
        .await?;

        if auth_ctx.role == "viewer" {
            return Err((StatusCode::FORBIDDEN, "Viewers cannot access fingerprints".to_string()));
        }
    } else {
        // Auth disabled - still check role for viewer restriction
        let (role, _) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
        if role == "viewer" {
            return Err((StatusCode::FORBIDDEN, "Viewers cannot access fingerprints".to_string()));
        }
    }

    if let Some(gateway) = &app_state.moq_gateway {
        let fingerprints = gateway.get_fingerprints().await;
        Ok(Json(serde_json::json!({
            "fingerprints": fingerprints
        })))
    } else {
        Err((StatusCode::SERVICE_UNAVAILABLE, "MoQ gateway not available".to_string()))
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
    let user_pipeline: UserPipeline =
        streamkit_api::yaml::parse_yaml(&req.yaml).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

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
    if app_state.event_tx.send(crate::state::BroadcastEvent::to_all(event)).is_err() {
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
    let node_view_data = session.get_node_view_data().await.unwrap_or_default();

    // Clone pipeline (short lock hold) and add runtime state to nodes.
    let mut api_pipeline = {
        let pipeline = session.pipeline.lock().await;
        pipeline.clone()
    };
    for (id, node) in &mut api_pipeline.nodes {
        node.state = node_states.get(id).cloned();
    }

    // Attach resolved view data so clients have accurate positions on initial load.
    if !node_view_data.is_empty() {
        api_pipeline.view_data = Some(node_view_data);
    }

    info!("Fetched pipeline with states for session '{}' via HTTP", session_id);
    Ok(Json(api_pipeline))
}

/// Binding between a multipart field and an http_input node.
struct HttpInputBinding {
    node_id: String,
    field_name: String,
    output_pin: String,
    required: bool,
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
            "Multipart fields must be ordered: 'config' first".to_string(),
        ));
    }

    let config_bytes = first_field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read config field: {e}")))?;
    let yaml_str = std::str::from_utf8(&config_bytes)
        .map_err(|e| AppError::BadRequest(format!("Config is not valid UTF-8: {e}")))?;
    streamkit_api::yaml::parse_yaml(yaml_str).map_err(AppError::BadRequest)
}

/// Build http_input bindings from the pipeline definition.
///
/// Defaults:
/// - Single http_input: field name defaults to "media"
/// - Multiple http_input: field names default to the node id
fn determine_http_input_bindings(
    pipeline_def: &Pipeline,
) -> Result<Vec<HttpInputBinding>, AppError> {
    // Record which output pins the pipeline references for each http_input node
    let mut pins_used: HashMap<String, HashSet<String>> = HashMap::new();
    for conn in &pipeline_def.connections {
        if let Some(node_def) = pipeline_def.nodes.get(&conn.from_node) {
            if node_def.kind == "streamkit::http_input" {
                pins_used.entry(conn.from_node.clone()).or_default().insert(conn.from_pin.clone());
            }
        }
    }

    let http_inputs: Vec<(&String, &streamkit_api::Node)> = pipeline_def
        .nodes
        .iter()
        .filter(|(_, node)| node.kind == "streamkit::http_input")
        .collect();

    let default_field = if http_inputs.len() == 1 { Some("media".to_string()) } else { None };
    let mut seen_fields: HashSet<String> = HashSet::new();
    let mut bindings = Vec::new();

    for (node_id, node_def) in http_inputs {
        let mut node_bindings: Vec<HttpInputBinding> = Vec::new();
        let mut single_field: Option<String> = None;
        let mut single_required = true;
        let mut has_fields_param = false;
        let mut has_single_field_param = false;

        if let Some(params) = &node_def.params {
            if let Some(fields_val) = params.get("fields") {
                has_fields_param = true;
                let fields = fields_val.as_array().ok_or_else(|| {
                    AppError::BadRequest(
                        "streamkit::http_input.params.fields must be an array of strings or objects"
                            .to_string(),
                    )
                })?;

                for entry in fields {
                    let (name, required) = match entry {
                        serde_json::Value::String(s) => (s.clone(), true),
                        serde_json::Value::Object(map) => {
                            let Some(name_val) = map.get("name") else {
                                return Err(AppError::BadRequest(
                                    "fields entries must include 'name'".to_string(),
                                ));
                            };
                            let name = name_val
                                .as_str()
                                .ok_or_else(|| {
                                    AppError::BadRequest("fields.name must be a string".to_string())
                                })?
                                .trim()
                                .to_string();
                            if name.is_empty() {
                                return Err(AppError::BadRequest(
                                    "fields.name must not be empty".to_string(),
                                ));
                            }
                            let required = map
                                .get("required")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true);
                            (name, required)
                        },
                        _ => {
                            return Err(AppError::BadRequest(
                                "fields entries must be strings or objects".to_string(),
                            ))
                        },
                    };

                    node_bindings.push(HttpInputBinding {
                        node_id: node_id.clone(),
                        field_name: name,
                        output_pin: String::new(),
                        required,
                    });
                }
            } else if let Some(field_val) = params.get("field").and_then(serde_json::Value::as_str)
            {
                has_single_field_param = true;
                let trimmed = field_val.trim();
                if !trimmed.is_empty() {
                    single_field = Some(trimmed.to_string());
                }
                if let Some(req_val) = params.get("required").and_then(serde_json::Value::as_bool) {
                    single_required = req_val;
                }
            }
        }

        if has_fields_param && has_single_field_param {
            return Err(AppError::BadRequest(
                "streamkit::http_input: use either 'field' or 'fields', not both".to_string(),
            ));
        }

        if has_fields_param && node_bindings.is_empty() {
            return Err(AppError::BadRequest(
                "streamkit::http_input.params.fields must include at least one field".to_string(),
            ));
        }

        if node_bindings.is_empty() {
            let field_name =
                single_field.or_else(|| default_field.clone()).unwrap_or_else(|| node_id.clone());
            node_bindings.push(HttpInputBinding {
                node_id: node_id.clone(),
                field_name,
                output_pin: String::new(),
                required: single_required,
            });
        }

        // Back-compat: allow implicit 'media' only when no fields array is provided.
        if !has_fields_param
            && default_field.as_deref() == Some("media")
            && !node_bindings.iter().any(|b| b.field_name == "media")
        {
            node_bindings.push(HttpInputBinding {
                node_id: node_id.clone(),
                field_name: "media".to_string(),
                output_pin: String::new(),
                required: false,
            });
        }

        // Decide pin names based on referenced connections. Keep field names for multi-field mode,
        // but allow legacy 'out' default when only one pin is referenced (steps format).
        let used_pins = pins_used.get(node_id.as_str()).cloned().unwrap_or_default();
        for binding in &mut node_bindings {
            let pin_name = if used_pins.contains(&binding.field_name) {
                binding.field_name.clone()
            } else if used_pins.len() == 1 && !has_fields_param {
                // Legacy steps pipelines reference 'out'
                used_pins.iter().next().cloned().unwrap_or_else(|| binding.field_name.clone())
            } else {
                binding.field_name.clone()
            };
            binding.output_pin = pin_name;
        }

        for binding in node_bindings {
            if !seen_fields.insert(binding.field_name.clone()) {
                return Err(AppError::BadRequest(format!(
                    "Duplicate multipart field name '{field_name}' across http_input nodes",
                    field_name = binding.field_name
                )));
            }
            bindings.push(binding);
        }
    }

    Ok(bindings)
}

/// Stream all chunks from a media field through the provided channel.
async fn stream_media_field_chunks(
    field: &mut raw_multer::Field<'_>,
    media_tx: &tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>,
    cancellation_token: Option<&CancellationToken>,
) {
    let mut chunk_count: usize = 0;
    let mut total_bytes: usize = 0;

    if let Some(token) = cancellation_token {
        loop {
            tokio::select! {
                () = token.cancelled() => {
                    tracing::info!(
                        "Stopped streaming media early after {} chunks ({} bytes) due to cancellation",
                        chunk_count,
                        total_bytes
                    );
                    break;
                }
                chunk_result = field.chunk() => {
                    match chunk_result {
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
        }
        return;
    }

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

/// Route multipart fields into pre-created channels based on expected names.
async fn route_multipart_fields(
    mut multipart: raw_multer::Multipart<'_>,
    mut field_senders: HashMap<String, tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>>,
    required_fields: HashSet<String>,
    mut required_seen_tx: Option<tokio::sync::oneshot::Sender<()>>,
    parse_done_tx: tokio::sync::oneshot::Sender<Result<(), AppError>>,
    cancellation_token: CancellationToken,
) {
    let mut seen_required: HashSet<String> = HashSet::new();

    let result = async {
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
        {
            let fname = field.name().map(std::string::ToString::to_string).unwrap_or_default();
            if fname.is_empty() {
                continue;
            }

            let Some(sender) = field_senders.remove(&fname) else {
                let expected = if field_senders.is_empty() {
                    "none".to_string()
                } else {
                    field_senders.keys().cloned().collect::<Vec<_>>().join(", ")
                };
                return Err(AppError::BadRequest(format!(
                    "Unexpected multipart field '{fname}'. Expected: {expected}"
                )));
            };

            if required_fields.contains(&fname) {
                seen_required.insert(fname.clone());
                if seen_required.len() == required_fields.len() {
                    if let Some(tx) = required_seen_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }

            stream_media_field_chunks(&mut field, &sender, Some(&cancellation_token)).await;
        }

        if !required_fields.is_empty() && seen_required.len() < required_fields.len() {
            let missing: Vec<_> = required_fields.difference(&seen_required).cloned().collect();
            return Err(AppError::BadRequest(format!(
                "Missing required multipart field(s): {}",
                missing.join(", ")
            )));
        }

        Ok(())
    }
    .await;

    drop(field_senders);

    if let Some(tx) = required_seen_tx.take() {
        let _ = tx.send(());
    }

    let _ = parse_done_tx.send(result);
}

/// Validate that the pipeline has the required nodes for oneshot processing.
/// Returns (has_http_input, has_file_read, has_http_output) for logging purposes.
///
/// Pipelines must have `streamkit::http_output`. For input, they must have at least one of:
/// - `streamkit::http_input` (HTTP streaming mode)
/// - `core::file_reader` (file-based mode)
/// - Neither (generator mode — the pipeline produces its own data, e.g. video::colorbars)
fn validate_pipeline_nodes(pipeline_def: &Pipeline) -> Result<(bool, bool, bool), AppError> {
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
            self.record("incomplete");
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
    // Enforce RBAC for oneshot execution. When built-in auth is enabled, the request is first
    // authenticated by `auth_guard_middleware`, which injects the resolved role into a trusted
    // header so existing handlers can apply RBAC without refactors.
    let headers = req.headers().clone();
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
    if !perms.create_sessions {
        return Err(AppError::Forbidden(
            "Permission denied: cannot execute oneshot pipelines".to_string(),
        ));
    }

    // Parse multipart: read boundary + config first
    let boundary = extract_multipart_boundary(req.headers())?;
    let body_stream = req.into_body().into_data_stream();
    let mut multipart = raw_multer::Multipart::new(body_stream, boundary);
    let user_pipeline = parse_config_field(&mut multipart).await?;

    // Compile pipeline definition
    tracing::debug!("Compiling user pipeline definition");
    let pipeline_def: Pipeline = compile(user_pipeline)?;
    tracing::debug!("Pipeline compilation completed");

    let input_bindings = determine_http_input_bindings(&pipeline_def)?;

    // Validate pipeline structure
    let (has_http_input, has_file_read, has_http_output) = validate_pipeline_nodes(&pipeline_def)?;

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

    // Validate file/script paths
    validate_file_reader_paths(&pipeline_def, &app_state.config.security)?;
    validate_file_writer_paths(&pipeline_def, &app_state.config.security)?;
    validate_script_paths(&pipeline_def, &app_state.config.security)?;

    tracing::info!(
        "Pipeline validation passed: mode={}, has_http_input={}, has_file_read={}, has_http_output={}",
        if has_http_input { "http-streaming" } else if has_file_read { "file-based" } else { "generator" },
        has_http_input,
        has_file_read,
        has_http_output
    );
    tracing::info!(role = %role_name, "Executing oneshot pipeline for role");

    // Prepare multipart routing
    let cancel_token = CancellationToken::new();
    let mut field_senders: HashMap<String, tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>> =
        HashMap::new();
    let mut engine_inputs = Vec::new();
    let mut required_fields: HashSet<String> = HashSet::new();

    let io_capacity = app_state
        .config
        .engine
        .oneshot
        .io_channel_capacity
        .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_IO_CAPACITY);

    for binding in &input_bindings {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, axum::Error>>(io_capacity);
        if binding.required {
            required_fields.insert(binding.field_name.clone());
        }
        field_senders.insert(binding.field_name.clone(), tx);

        let media_stream: MediaStream = Box::new(ReceiverStream::new(rx).map(|x| x));
        engine_inputs.push(OneshotInput {
            node_id: binding.node_id.clone(),
            output_pin: binding.output_pin.clone(),
            stream: media_stream,
            content_type: None,
            field_name: binding.field_name.clone(),
            required: binding.required,
            cancellation_token: Some(cancel_token.clone()),
        });
    }

    let (required_seen_tx, required_seen_rx) = tokio::sync::oneshot::channel();
    let mut required_seen_tx = Some(required_seen_tx);
    if required_fields.is_empty() {
        if let Some(tx) = required_seen_tx.take() {
            let _ = tx.send(());
        }
    }
    let (parse_done_tx, parse_done_rx) = tokio::sync::oneshot::channel();

    // Spawn multipart routing task
    let routing_task = tokio::spawn(route_multipart_fields(
        multipart,
        field_senders,
        required_fields.clone(),
        required_seen_tx,
        parse_done_tx,
        cancel_token.clone(),
    ));

    // Wait for required fields to appear (prevents hanging on missing uploads)
    tokio::time::timeout(Duration::from_secs(5), required_seen_rx)
        .await
        .map_err(|_| {
            cancel_token.cancel();
            AppError::BadRequest("Timed out waiting for required multipart fields".to_string())
        })?
        .map_err(|_| AppError::BadRequest("Failed to observe multipart state".into()))?;

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
                .with_boundaries(
                    streamkit_core::metrics::HISTOGRAM_BOUNDARIES_PIPELINE_DURATION.to_vec(),
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
            engine_inputs,
            Some(oneshot_config),
            Some(cancel_token.clone()),
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
            cancel_token.cancel();
            return Err(e.into());
        },
    };

    // Ensure multipart routing finished cleanly
    match parse_done_rx.await {
        Ok(Ok(())) => {},
        Ok(Err(err)) => {
            let labels = [KeyValue::new("status", "error")];
            oneshot_duration_histogram.record(oneshot_start_time.elapsed().as_secs_f64(), &labels);
            cancel_token.cancel();
            return Err(err);
        },
        Err(e) => {
            cancel_token.cancel();
            return Err(AppError::BadRequest(format!("Multipart routing task aborted: {e}")));
        },
    }
    let _ = routing_task.await;

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

    // Require auth when enabled (cookie or Authorization header)
    let (role_name, perms) = if app_state.auth.is_enabled() {
        match crate::auth::validate_token_from_headers(
            &headers,
            &app_state.auth,
            &app_state.config,
            &app_state.config.permissions,
        )
        .await
        {
            Ok(ctx) => (ctx.role, ctx.permissions),
            Err((status, msg)) => return (status, msg).into_response(),
        }
    } else {
        crate::role_extractor::get_role_and_permissions(&headers, &app_state)
    };
    ws.on_upgrade(move |socket| websocket::handle_websocket(socket, app_state, perms, role_name))
}

async fn static_handler(
    uri: axum::http::Uri,
    State(app_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let raw_path = uri.path();
    let stripped_path =
        strip_base_path_prefix(raw_path, app_state.config.server.base_path.as_deref());
    if stripped_path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path = stripped_path.trim_start_matches('/');

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

        // Inject a <base> tag into index.html for SPA routing.
        //
        // Vite builds use relative asset URLs (e.g. `./assets/...`). Without a `<base>` tag, those
        // URLs resolve relative to the current route (e.g. `/admin/assets/...`), which breaks when
        // users deep-link to multi-segment routes like `/admin/tokens`. Injecting `<base href="/">`
        // (or `<base href="/<base_path>/">`) fixes this.
        if path == "index.html" {
            let base_path = normalized_base_path_for_html(app_state.as_ref());
            let base_path = escape_html_attr(&base_path);
            let html = String::from_utf8_lossy(&content.data);
            let injected =
                html.replace("<head>", &format!("<head>\n    <base href=\"{base_path}/\">"));
            return (headers, injected.into_bytes()).into_response();
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

            let base_path = normalized_base_path_for_html(app_state.as_ref());
            let base_path = escape_html_attr(&base_path);
            let html = String::from_utf8_lossy(&content.data);
            let injected =
                html.replace("<head>", &format!("<head>\n    <base href=\"{base_path}/\">"));
            (headers, injected.into_bytes()).into_response()
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
                meter
                    .f64_histogram("http.server.duration")
                    .with_boundaries(
                        streamkit_core::metrics::HISTOGRAM_BOUNDARIES_HTTP_DURATION.to_vec(),
                    )
                    .build(),
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
/// # Arguments
///
/// * `config` - The server configuration
/// * `auth` - Optional pre-initialized AuthState. If None, creates a disabled auth state.
///
/// # Panics
///
/// Panics if the plugin manager fails to initialize. This can happen if:
/// - Plugin directories cannot be created due to filesystem permissions
/// - Plugin directories exist but are not accessible
/// - CORS configuration is invalid (wildcard with auth enabled)
///
/// Since this occurs during application initialization, a panic here is acceptable
/// as the server cannot function without proper configuration.
#[allow(clippy::expect_used)]
pub fn create_app(
    mut config: Config,
    auth: Option<Arc<crate::auth::AuthState>>,
) -> (Router, Arc<AppState>) {
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

    // Build server-level node constraints from config
    let mut constraints = streamkit_core::GlobalNodeConstraints::new();

    #[cfg(feature = "script")]
    {
        let global_fetch_allowlist = config
            .script
            .global_fetch_allowlist
            .iter()
            .map(|rule| streamkit_nodes::core::script::AllowlistRule {
                url: rule.url.clone(),
                methods: rule.methods.clone(),
            })
            .collect();

        let secrets = load_script_secrets(&config.script.secrets);

        constraints.insert(streamkit_nodes::core::script::GlobalScriptConfig {
            global_fetch_allowlist,
            secrets,
        });
    }

    #[cfg(feature = "compositor")]
    {
        constraints.insert(streamkit_nodes::video::compositor::config::GlobalCompositorConfig {
            max_canvas_dimension: config.compositor.max_canvas_dimension,
            max_font_size: config.compositor.max_font_size,
            max_text_length: config.compositor.max_text_length,
        });
    }

    let engine = Arc::new(Engine::with_resource_manager_and_constraints(
        resource_manager.clone(),
        &constraints,
    ));

    // Initialize plugin manager - panic on failure since we can't proceed without it
    // This expect is justified and documented in the function's # Panics section
    #[allow(clippy::expect_used)]
    let plugin_manager = UnifiedPluginManager::new(
        Arc::clone(&engine),
        resource_manager,
        plugin_base_dir,
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

    let marketplace_jobs = crate::marketplace_installer::InstallJobQueue::new(
        &config.plugins,
        Arc::clone(&plugin_manager),
    )
    .expect("Failed to initialize marketplace installer");

    #[cfg(feature = "moq")]
    let moq_gateway = {
        let gateway = Arc::new(crate::moq_gateway::MoqGateway::new());
        // Initialize global gateway registry so nodes can access it
        let trait_obj: Arc<dyn streamkit_core::moq_gateway::MoqGatewayTrait> = gateway.clone();
        streamkit_core::moq_gateway::init_moq_gateway(trait_obj);
        Some(gateway)
    };

    // Use provided auth state or create disabled auth
    let auth = auth.unwrap_or_else(|| Arc::new(crate::auth::AuthState::disabled()));

    // When built-in auth is enabled, treat the injected role header as the trusted role source.
    //
    // SECURITY: This header is overwritten by `auth_guard_middleware` for every API request.
    if auth.is_enabled() {
        config.permissions.role_header = Some(BUILTIN_AUTH_ROLE_HEADER.to_string());
    }

    let app_state = Arc::new(AppState {
        engine,
        session_manager: Arc::new(tokio::sync::Mutex::new(SessionManager::default())),
        config: Arc::new(config),
        event_tx,
        plugin_manager,
        marketplace_jobs,
        auth,
        shutdown_tracker: crate::state::ShutdownTracker::default(),
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
        .route("/.well-known/jwks.json", get(jwks_handler))
        .route("/api/v1/process", oneshot_route)
        .route("/api/v1/marketplace/registries", get(list_marketplace_registries_handler))
        .route("/api/v1/marketplace/plugins", get(list_marketplace_plugins_handler))
        .route("/api/v1/marketplace/plugins/{plugin_id}", get(get_marketplace_plugin_handler))
        .route("/api/v1/plugins/install", post(install_plugin_handler))
        .route(
            "/api/v1/plugins",
            get(list_plugins_handler)
                .post(upload_plugin_handler)
                // Plugin uploads are multipart; raise default body limit for realistic artifacts.
                .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size)),
        )
        .route("/api/v1/plugins/{kind}", delete(delete_plugin_handler))
        .route("/api/v1/jobs/{job_id}", get(get_job_handler))
        .route("/api/v1/jobs/{job_id}/cancel", post(cancel_job_handler))
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

    // Add auth routes
    router = router.nest("/api/v1/auth", crate::auth::auth_router());

    // Configure CORS with auth enabled state
    let auth_enabled = app_state.auth.is_enabled();
    let cors_layer = create_cors_layer(&app_state.config.server.cors, auth_enabled)
        .expect("CORS configuration error");

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
        .layer(middleware::from_fn_with_state(Arc::clone(&app_state), auth_guard_middleware))
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

    let auth_state = Arc::clone(&app_state.auth);

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
        let mut tls = ServerTlsConfig::default();
        tls.cert = vec![std::path::PathBuf::from(&config.server.cert_path)];
        tls.key = vec![std::path::PathBuf::from(&config.server.key_path)];
        tls
    } else {
        info!("Auto-generating self-signed certificate for MoQ WebTransport (14-day validity for local development)");
        let mut tls = ServerTlsConfig::default();
        tls.generate = vec!["localhost".to_string()];
        tls
    };

    let mut moq_config = MoqServerConfig::default();
    moq_config.bind = Some(addr);
    moq_config.tls = tls;

    info!(
        address = %addr,
        "Starting MoQ WebTransport acceptor on UDP (same port as HTTP server)"
    );

    tokio::spawn(async move {
        match moq_config.init() {
            Ok(mut server) => {
                // Store fingerprints in gateway for HTTP endpoint
                let fingerprints = server
                    .tls_info()
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fingerprints
                    .clone();
                gateway.set_fingerprints(fingerprints.clone()).await;

                for (i, fp) in fingerprints.iter().enumerate() {
                    info!("🔐 MoQ WebTransport certificate fingerprint #{}: {}", i + 1, fp);
                }
                info!("💡 Access fingerprints at: http://{}/api/v1/moq/fingerprints", addr);

                info!("MoQ WebTransport server listening for connections");

                // Accept connections in a loop
                while let Some(request) = server.accept().await {
                    let gateway = Arc::clone(&gateway);
                    let auth_state = Arc::clone(&auth_state);

                    tokio::spawn(async move {
                        // Extract URL data before consuming the request.
                        // request.url() borrows, so we copy what we need first.
                        let (path, jwt_param) = {
                            let Some(url) = request.url() else {
                                debug!("Received MoQ connection without URL (raw QUIC), ignoring");
                                return;
                            };
                            let path = url.path().to_string();
                            let jwt_param = url
                                .query_pairs()
                                .find(|(k, _)| k == "jwt")
                                .map(|(_, v)| v.to_string());
                            (path, jwt_param)
                        };

                        // SECURITY: Never log the full URL (may contain jwt)
                        debug!(path = %path, "Received MoQ connection request");

                        // Validate MoQ auth if enabled
                        let moq_auth = if auth_state.is_enabled() {
                            match validate_moq_auth(&auth_state, &path, jwt_param).await {
                                Ok(ctx) => Some(ctx),
                                Err(status) => {
                                    let _ = request.close(status.as_u16()).await;
                                    return;
                                },
                            }
                        } else {
                            None
                        };

                        if let Err(e) =
                            gateway.accept_connection(request, path.clone(), moq_auth).await
                        {
                            warn!(path = %path, error = %e, "Failed to route MoQ connection");
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

/// Validates MoQ auth for an incoming connection, returning the auth context on success
/// or the HTTP status code to reject with on failure.
#[cfg(feature = "moq")]
async fn validate_moq_auth(
    auth_state: &crate::auth::AuthState,
    path: &str,
    jwt_param: Option<String>,
) -> Result<Arc<dyn streamkit_core::moq_gateway::MoqAuthChecker>, axum::http::StatusCode> {
    let Some(jwt) = jwt_param else {
        warn!(path = %path, "MoQ auth failed: missing jwt parameter");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    };

    // Validate JWT
    let claims = auth_state.validate_moq_token(&jwt).map_err(|e| {
        warn!(path = %path, error = %e, "MoQ JWT validation failed");
        axum::http::StatusCode::UNAUTHORIZED
    })?;

    // Check audience
    if claims.aud != crate::auth::AUD_MOQ {
        warn!(path = %path, expected = crate::auth::AUD_MOQ, actual = %claims.aud, "MoQ auth failed: wrong audience");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    let token_hash = crate::auth::hash_token(&jwt);

    // Enforce "tokens we mint" policy (parity with HTTP API auth).
    let metadata_store = auth_state.token_metadata_store().cloned().ok_or_else(|| {
        warn!(path = %path, "MoQ auth failed: token metadata store not available");
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    })?;

    let meta = metadata_store.get(&claims.jti).await.map_err(|e| {
        warn!(path = %path, error = %e, "MoQ auth failed: metadata store error");
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    })?;

    let Some(meta) = meta else {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token not recognized (not minted by this server)");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    };

    // Extra robustness: ensure the presented token matches the stored hash.
    if meta.token_hash != token_hash {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token hash mismatch");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if meta.revoked {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    // Check revocation
    if auth_state.is_revoked(&token_hash) {
        warn!(path = %path, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    // Verify root matches path and reduce permissions
    crate::auth::verify_moq_token(&claims, path)
        .map_err(|e| {
            warn!(path = %path, error = %e, "MoQ path verification failed");
            axum::http::StatusCode::UNAUTHORIZED
        })
        .map(|ctx| Arc::new(ctx) as Arc<dyn streamkit_core::moq_gateway::MoqAuthChecker>)
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
#[allow(clippy::cognitive_complexity)]
pub async fn start_server(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = config.server.address.parse()?;

    // Determine if auth should be enabled based on config mode and bind address
    let auth_enabled = match config.auth.mode {
        crate::config::AuthMode::Auto => !addr.ip().is_loopback(),
        crate::config::AuthMode::Enabled => true,
        crate::config::AuthMode::Disabled => false,
    };

    // Deployment footgun: cookie-based auth without TLS.
    //
    // When TLS is disabled, session cookies are set without the `Secure` attribute, so browsers
    // may send them over plain HTTP. This is unsafe on untrusted networks.
    if auth_enabled && !config.server.tls {
        warn!(
            mode = ?config.auth.mode,
            address = %addr,
            "Auth is enabled but TLS is disabled; session cookies will be set without the Secure attribute. \
             Enable TLS (server.tls=true) or terminate TLS in a trusted reverse proxy and ensure cookies are only used over HTTPS."
        );
    }

    // Common migration footgun: deployments that previously relied on a reverse proxy setting a
    // trusted role header may have `auth.mode=auto` and bind to a non-loopback address.
    //
    // In that case, built-in auth will turn on implicitly and override `permissions.role_header`
    // (see `create_app`), which can break proxy-based auth unexpectedly.
    if matches!(config.auth.mode, crate::config::AuthMode::Auto)
        && auth_enabled
        && config.permissions.role_header.is_some()
    {
        warn!(
            mode = ?config.auth.mode,
            address = %addr,
            role_header = %config.permissions.role_header.as_deref().unwrap_or_default(),
            "auth.mode=auto enabled built-in auth due to a non-loopback bind address, but permissions.role_header is set. \
             Built-in auth overrides role_header and ignores reverse-proxy role headers. \
             If you rely on proxy auth, set auth.mode=disabled."
        );
    }

    // Validate CORS configuration early - fail if wildcard origins with auth enabled
    let has_wildcard = config.server.cors.allowed_origins.iter().any(|o| o == "*");
    if auth_enabled && has_wildcard {
        return Err(
            "CORS allowed_origins='*' is incompatible with auth (cookies require explicit origins). \
             Set allowed_origins to specific origins or disable auth.".into()
        );
    }

    // Common deploy footgun: auth enabled + localhost-only CORS allowlist.
    //
    // When auth is enabled, browser requests rely on cookie auth, which requires that the browser
    // `Origin` be on the allowlist for mutating endpoints and the WebSocket control plane. If the
    // server is reachable on a non-loopback address but the allowlist is still localhost-only,
    // the UI will fail with 403.
    if auth_enabled
        && !addr.ip().is_loopback()
        && cors_allowed_origins_are_loopback_only(&config.server.cors.allowed_origins)
    {
        warn!(
            allowed_origins = ?config.server.cors.allowed_origins,
            address = %addr,
            "Auth is enabled, but server.cors.allowed_origins appears to be loopback-only; \
             browser requests from non-local origins will be rejected. \
             Configure [server.cors].allowed_origins for your deployment origin(s)."
        );
    }

    // Initialize auth state
    let auth = if auth_enabled {
        info!(
            mode = ?config.auth.mode,
            state_dir = %config.auth.state_dir,
            "Initializing authentication"
        );
        match crate::auth::AuthState::new(&config.auth, true).await {
            Ok(state) => {
                info!("Authentication enabled and initialized");
                // Startup banner (no secrets): how to log in + verify tokens.
                let scheme = if config.server.tls { "https" } else { "http" };
                let base_path =
                    normalize_base_path(config.server.base_path.as_deref()).unwrap_or_default();
                let login_path = format!("{base_path}/login");
                let ui_host = if addr.ip().is_unspecified() {
                    format!("localhost:{}", addr.port())
                } else {
                    addr.to_string()
                };

                let token_path =
                    std::path::PathBuf::from(&config.auth.state_dir).join("admin.token");
                if token_path.exists() {
                    info!(path = %token_path.display(), "Bootstrap admin token file");
                } else {
                    warn!(path = %token_path.display(), "Bootstrap admin token file missing");
                }
                info!("To print the bootstrap token: skit auth print-admin-token");
                info!("Web UI login: {}://{}{}", scheme, ui_host, login_path);
                info!("JWKS (public): {}://{}/.well-known/jwks.json", scheme, ui_host);
                Arc::new(state)
            },
            Err(e) => {
                return Err(format!("Failed to initialize authentication: {e}").into());
            },
        }
    } else {
        info!(
            mode = ?config.auth.mode,
            is_loopback = addr.ip().is_loopback(),
            "Authentication disabled"
        );
        Arc::new(crate::auth::AuthState::disabled())
    };

    let (app, app_state) = create_app(config.clone(), Some(auth));
    #[cfg(not(feature = "moq"))]
    let _ = &app_state;

    // Legacy role_header check - only applies when auth is disabled
    if !auth_enabled && !addr.ip().is_loopback() && config.permissions.role_header.is_none() {
        if !config.permissions.allow_insecure_no_auth {
            return Err(format!(
                "Refusing to start: server.address is '{addr}' (non-loopback) but auth is disabled and permissions.role_header is not set. \
                 Without built-in auth or a trusted auth layer, all requests fall back to SK_ROLE/default_role ('{}'). \
                 Fix: enable auth (mode=enabled or mode=auto), put StreamKit behind an authenticating reverse proxy and set permissions.role_header, or (unsafe) set permissions.allow_insecure_no_auth = true to override.",
                config.permissions.default_role
            )
            .into());
        }
        warn!(
            address = %addr,
            default_role = %config.permissions.default_role,
            allow_http_management = config.plugins.http_management.allow_http_management,
            "Starting without built-in auth or a trusted role header on a non-loopback address; all requests fall back to SK_ROLE/default_role. \
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
            let tracker = app_state.shutdown_tracker.clone();
            async move {
                shutdown_signal.await;
                // Drain background shutdown tasks before stopping the server
                tracker.drain(std::time::Duration::from_secs(10)).await;
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
            let tracker = app_state.shutdown_tracker.clone();
            async move {
                shutdown_signal.await;
                // Drain background shutdown tasks before stopping the server
                tracker.drain(std::time::Duration::from_secs(10)).await;
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
