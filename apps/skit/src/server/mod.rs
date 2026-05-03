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

    let patterns: Vec<String> = config.allowed_origins.clone();

    info!(
        allowed_origins = ?patterns,
        auth_enabled,
        "CORS configured with origin allowlist"
    );

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
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    let mut definitions = read_registry(&app_state)?.definitions();

    definitions.extend(synthetic_node_definitions());

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
    pub(crate) valid: bool,
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
struct ValidateRequest {
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
fn validate_nodes(
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
fn validate_connections(
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
fn check_mode(
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
fn validate_pin_types(
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
async fn validate_pipeline_handler(
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

#[cfg(test)]
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

    // Unregister any asset types owned by this plugin so the CRUD endpoints
    // stop serving stale types.  User-uploaded files are intentionally left
    // in place — they will become available again if the plugin is re-installed.
    //
    // For marketplace plugins use the authoritative `plugin_id` from the
    // active record (matches `manifest.id` used during registration).
    // For non-marketplace plugins fall back to `original_kind` which, by
    // convention, equals the manifest `id`.
    let unregister_id =
        active_record.as_ref().map_or(&summary.original_kind, |(_, record)| &record.plugin_id);
    app_state.plugin_asset_registry.unregister_plugin(unregister_id).await;

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

/// Handler for MSE streaming — serves WebM media data to HTTP clients.
///
/// When an HttpMse node registers a path via the MSE gateway, this handler
/// connects incoming HTTP GET requests to the node's output stream.
/// Each client receives the WebM init segment followed by live media clusters.
///
/// # Security model
///
/// This endpoint is intentionally **unauthenticated**, matching the MoQ
/// WebTransport endpoint's model. MSE streams are consumed by browser
/// `<video>` elements and `fetch()` streaming, which cannot send custom
/// auth headers. If authentication is needed in the future, consider
/// query-parameter token auth (e.g. `/mse/{path}?token=...`).
async fn mse_stream_handler(
    State(app_state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Response {
    use crate::mse_gateway::MseConnectError;

    let full_path = format!("/mse/{path}");

    match app_state.mse_gateway.connect_client(&full_path).await {
        Ok((content_type, body_rx, guard)) => {
            // Wrap the stream to keep the MseClientGuard alive for the entire
            // duration of the HTTP response. When the stream ends (client
            // disconnects or node stops), the guard is dropped, decrementing
            // the active client counter in the gateway.
            let stream = ReceiverStream::new(body_rx).map(Ok::<_, Infallible>);
            let guarded_stream = GuardedStream { inner: stream, _guard: guard };

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, "no-cache, no-store")
                .body(Body::from_stream(guarded_stream))
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build response").into_response()
                })
        },
        Err(MseConnectError::NotFound) => {
            debug!(path = %full_path, "MSE stream not found");
            (StatusCode::NOT_FOUND, "No MSE stream registered for this path").into_response()
        },
        Err(MseConnectError::AtCapacity) => {
            warn!(path = %full_path, "MSE stream at capacity");
            (StatusCode::SERVICE_UNAVAILABLE, "MSE stream at maximum client capacity")
                .into_response()
        },
        Err(MseConnectError::NodeStopped) => {
            debug!(path = %full_path, "MSE stream node stopped");
            (StatusCode::GONE, "MSE stream node is no longer running").into_response()
        },
    }
}

/// A stream wrapper that holds an [`MseClientGuard`] alongside the inner stream.
/// The guard decrements the gateway's active-client counter when this stream
/// (and therefore the HTTP response) is dropped.
struct GuardedStream<S> {
    inner: S,
    _guard: crate::mse_gateway::MseClientGuard,
}

impl<S: futures::Stream + Unpin> futures::Stream for GuardedStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
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

/// Helper function to send all node and connection control messages to the engine actor.
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

    let node_states = session.get_node_states().await.unwrap_or_default();
    let node_view_data = session.get_node_view_data().await.unwrap_or_default();
    let runtime_schemas = session.get_runtime_schemas().await.unwrap_or_default();

    let mut api_pipeline = {
        let pipeline = session.pipeline.lock().await;
        pipeline.clone()
    };
    for (id, node) in &mut api_pipeline.nodes {
        node.state = node_states.get(id).cloned();
    }

    // Attach resolved view data so clients have accurate positions on initial load.
    if !node_view_data.is_empty() {
        api_pipeline.view_data = Some(Arc::unwrap_or_clone(node_view_data));
    }

    // Attach runtime param schemas so the UI can merge them with static schemas.
    if !runtime_schemas.is_empty() {
        api_pipeline.runtime_schemas = Some(runtime_schemas);
    }

    info!("Fetched pipeline with states for session '{}' via HTTP", session_id);
    Ok(Json(api_pipeline))
}

// ---------------------------------------------------------------------------
// Preview API — engine-native pipeline tap
// ---------------------------------------------------------------------------

#[cfg(feature = "moq")]
pub mod preview;

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

    let headers = req.headers().clone();
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
    if !perms.create_sessions {
        return Err(AppError::Forbidden(
            "Permission denied: cannot execute oneshot pipelines".to_string(),
        ));
    }

    let boundary = extract_multipart_boundary(req.headers())?;
    let body_stream = req.into_body().into_data_stream();
    let mut multipart = raw_multer::Multipart::new(body_stream, boundary);
    let user_pipeline = parse_config_field(&mut multipart).await?;

    let pipeline_def: Pipeline = compile(user_pipeline)?;

    let input_bindings = determine_http_input_bindings(&pipeline_def)?;

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

    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let mut headers = HeaderMap::new();
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

    // Create session (engine allocation).
    let session = crate::session::Session::create(
        &app_state.engine,
        &app_state.config,
        name,
        app_state.event_tx.clone(),
        Some(role_name),
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
async fn check_batch_node_id_uniqueness(
    session: &crate::session::Session,
    operations: &[streamkit_api::BatchOperation],
) -> Vec<String> {
    let mut live_ids: std::collections::HashSet<String> =
        session.pipeline.lock().await.nodes.keys().cloned().collect();
    let mut duplicates = Vec::new();
    for op in operations {
        match op {
            streamkit_api::BatchOperation::AddNode { node_id, .. } => {
                if !live_ids.insert(node_id.clone()) {
                    duplicates.push(node_id.clone());
                }
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

    // Send control messages to the engine.
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

async fn tune_session_node_inner(
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

/// Build the shared [`AppState`] without constructing any HTTP router.
///
/// This is the common initialisation path used by both the HTTP server
/// (`create_app`) and the STDIO MCP server (`start_mcp_stdio`).
///
/// # Panics
///
/// See the panic documentation on [`create_app`] — the same invariants apply.
#[allow(clippy::expect_used)]
pub fn create_app_state(
    mut config: Config,
    auth: Option<Arc<crate::auth::AuthState>>,
) -> Arc<AppState> {
    let (event_tx, _) = tokio::sync::broadcast::channel(128);

    let resource_policy = streamkit_core::ResourcePolicy {
        keep_loaded: config.resources.keep_models_loaded,
        max_memory_mb: config.resources.max_memory_mb,
    };
    let resource_manager = Arc::new(streamkit_core::ResourceManager::new(resource_policy));

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

    let plugin_base_dir = std::path::PathBuf::from(&config.plugins.directory);
    let wasm_plugin_dir = plugin_base_dir.join("wasm");
    let native_plugin_dir = plugin_base_dir.join("native");

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

    #[allow(clippy::expect_used)]
    let plugin_manager = UnifiedPluginManager::new(
        Arc::clone(&engine),
        resource_manager,
        plugin_base_dir,
        wasm_plugin_dir,
        native_plugin_dir,
        config.plugins.native_call_timeout_secs.map(std::time::Duration::from_secs),
    )
    .expect("Failed to initialize unified plugin manager");
    let plugin_manager = Arc::new(tokio::sync::Mutex::new(plugin_manager));

    let plugin_asset_registry = crate::plugin_assets::PluginAssetRegistry::new();

    UnifiedPluginManager::spawn_load_existing(
        Arc::clone(&plugin_manager),
        config.resources.prewarm.clone(),
        plugin_asset_registry.clone(),
    );

    let marketplace_jobs = crate::marketplace_installer::InstallJobQueue::new(
        &config.plugins,
        Arc::clone(&plugin_manager),
        plugin_asset_registry.clone(),
    )
    .expect("Failed to initialize marketplace installer");

    #[cfg(feature = "moq")]
    let moq_gateway = {
        let gateway = Arc::new(crate::moq_gateway::MoqGateway::new());
        let trait_obj: Arc<dyn streamkit_core::moq_gateway::MoqGatewayTrait> = gateway.clone();
        streamkit_core::moq_gateway::init_moq_gateway(trait_obj);
        Some(gateway)
    };

    let mse_gateway = {
        let gateway = Arc::new(crate::mse_gateway::MseGateway::new());
        let trait_obj: Arc<dyn streamkit_core::mse_gateway::MseGatewayTrait> = gateway.clone();
        streamkit_core::mse_gateway::init_mse_gateway(trait_obj);
        gateway
    };

    let auth = auth.unwrap_or_else(|| Arc::new(crate::auth::AuthState::disabled()));

    if auth.is_enabled() {
        config.permissions.role_header = Some(BUILTIN_AUTH_ROLE_HEADER.to_string());
    }

    Arc::new(AppState {
        engine,
        session_manager: Arc::new(tokio::sync::Mutex::new(SessionManager::default())),
        config: Arc::new(config),
        event_tx,
        plugin_manager,
        marketplace_jobs,
        auth,
        shutdown_tracker: crate::state::ShutdownTracker::default(),
        plugin_asset_registry,
        #[cfg(feature = "moq")]
        moq_gateway,
        mse_gateway,
    })
}

/// Create the full Axum application router and shared application state.
///
/// # Panics
///
/// - The unified plugin manager cannot be initialized (missing plugin directories, etc.)
/// - Plugin directories cannot be created due to filesystem permissions
/// - Plugin directories exist but are not accessible
/// - CORS configuration is invalid (wildcard with auth enabled)
///
/// Since this occurs during application initialization, a panic here is acceptable
/// as the server cannot function without proper configuration.
#[allow(clippy::expect_used)]
pub fn create_app(
    config: Config,
    auth: Option<Arc<crate::auth::AuthState>>,
) -> (Router, Arc<AppState>) {
    let app_state = create_app_state(config, auth);

    let mut oneshot_route = post(process_oneshot_pipeline_handler)
        // Use configurable body limit for oneshot processing
        .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size));
    if let Some(max) = app_state.config.permissions.max_concurrent_oneshots {
        oneshot_route = oneshot_route.layer(ConcurrencyLimitLayer::new(max));
    }

    #[cfg_attr(not(any(feature = "moq", feature = "mcp")), allow(unused_mut))]
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
        .route(
            "/api/v1/validate",
            post(validate_pipeline_handler)
                .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size)),
        )
        .route("/api/v1/logs", get(crate::log_viewer::get_logs_handler))
        .route("/api/v1/logs/stream", get(crate::log_viewer::stream_logs_handler))
        .route("/api/v1/sessions", get(list_sessions_handler).post(create_session_handler))
        .route("/api/v1/sessions/{id}", delete(destroy_session_handler))
        .route("/api/v1/sessions/{id}/pipeline", get(get_pipeline_handler));

    #[cfg(feature = "moq")]
    {
        router = router
            .route(
                "/api/v1/sessions/{id}/preview",
                get(preview::list_previews_handler).post(preview::start_preview_handler),
            )
            .route(
                "/api/v1/sessions/{id}/preview/{preview_id}",
                delete(preview::stop_preview_handler),
            );
    }

    router = router
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
        .merge(crate::assets::assets_router())
        .merge(crate::assets::image_assets_router())
        .merge(crate::assets::font_assets_router())
        .merge(crate::plugin_assets::plugin_assets_router());

    #[cfg(feature = "moq")]
    {
        router = router.route("/api/v1/moq/fingerprints", get(get_moq_fingerprints_handler));
        router = router.route("/certificate.sha256", get(get_certificate_sha256_handler));
    }

    // The endpoint MUST live under /api/ so auth_guard_middleware, origin_guard_middleware,
    // CORS, tracing, and metrics all apply. Enforced at config-load time (McpConfig::validate).
    #[cfg(feature = "mcp")]
    {
        if app_state.config.mcp.enabled {
            info!(
                endpoint = %app_state.config.mcp.endpoint,
                "MCP endpoint enabled"
            );
            router = router.nest_service(
                &app_state.config.mcp.endpoint,
                crate::mcp::streamable_http_service(Arc::clone(&app_state)),
            );
        }
    }

    // Warn if mcp.enabled is set but the binary was compiled without the mcp feature.
    #[cfg(not(feature = "mcp"))]
    if app_state.config.mcp.enabled {
        warn!(
            "mcp.enabled is true but the binary was compiled without the 'mcp' feature. \
             The MCP endpoint will not be available. Rebuild with --features mcp."
        );
    }

    // Outside /api/ so auth_guard_middleware doesn't apply — matches the MoQ WebTransport model.
    router = router.route("/mse/{*path}", get(mse_stream_handler));

    router = router.nest("/api/v1/auth", crate::auth::auth_router());

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

    let addr: SocketAddr =
        config.server.moq_address.as_deref().unwrap_or(&config.server.address).parse()?;

    // TLS priority: moq_cert_path/moq_key_path → server cert_path/key_path (when tls=true) → self-signed.
    let moq_cert = config.server.moq_cert_path.as_deref().filter(|s| !s.is_empty());
    let moq_key = config.server.moq_key_path.as_deref().filter(|s| !s.is_empty());

    if moq_cert.is_some() != moq_key.is_some() {
        return Err(format!(
            "Invalid MoQ TLS config: both moq_cert_path and moq_key_path must be set (got cert={:?}, key={:?})",
            config.server.moq_cert_path, config.server.moq_key_path
        ).into());
    }

    let tls = if let (Some(cert), Some(key)) = (moq_cert, moq_key) {
        info!(cert_path = %cert, key_path = %key, "Using MoQ-specific TLS certificates for WebTransport");
        let mut tls = ServerTlsConfig::default();
        tls.cert = vec![std::path::PathBuf::from(cert)];
        tls.key = vec![std::path::PathBuf::from(key)];
        tls
    } else if config.server.tls
        && !config.server.cert_path.is_empty()
        && !config.server.key_path.is_empty()
    {
        info!(
            cert_path = %config.server.cert_path,
            key_path = %config.server.key_path,
            "Using server TLS certificates for MoQ WebTransport"
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
    moq_config.bind = Some(addr.to_string());
    moq_config.tls = tls;

    let moq_public_paths: Arc<[String]> = config
        .auth
        .moq_public_paths
        .iter()
        .filter(|p| {
            if p.is_empty() {
                warn!("Ignoring empty string in moq_public_paths (would bypass all MoQ auth)");
                false
            } else {
                true
            }
        })
        .cloned()
        .collect::<Vec<_>>()
        .into();

    info!(
        address = %addr,
        moq_public_paths = ?moq_public_paths,
        "Starting MoQ WebTransport acceptor on UDP"
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
                info!("💡 Access fingerprints at: /api/v1/moq/fingerprints (served by the HTTP server)");

                info!("MoQ WebTransport server listening for connections");

                // Accept connections in a loop
                while let Some(request) = server.accept().await {
                    let gateway = Arc::clone(&gateway);
                    let auth_state = Arc::clone(&auth_state);
                    let moq_public_paths = Arc::clone(&moq_public_paths);

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

                        // Validate MoQ auth if enabled (skipped for paths matching moq_public_paths).
                        // Segment-based: "/moq" matches "/moq" and "/moq/foo" but NOT "/moq2".
                        let is_public = moq_public_paths.iter().any(|prefix| {
                            path == prefix.as_str() || path.starts_with(&format!("{prefix}/"))
                        });
                        let moq_auth = if auth_state.is_enabled() && !is_public {
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

    let claims = auth_state.validate_moq_token(&jwt).map_err(|e| {
        warn!(path = %path, error = %e, "MoQ JWT validation failed");
        axum::http::StatusCode::UNAUTHORIZED
    })?;

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

    if meta.token_hash != token_hash {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token hash mismatch");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if meta.revoked {
        warn!(path = %path, jti = %claims.jti, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if auth_state.is_revoked(&token_hash) {
        warn!(path = %path, "MoQ auth failed: token revoked");
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

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

    #[cfg(feature = "moq")]
    start_moq_webtransport_acceptor(&app_state, config)?;

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

        tokio::spawn({
            let handle = handle.clone();
            let tracker = app_state.shutdown_tracker.clone();
            async move {
                shutdown_signal.await;
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

        tokio::spawn({
            let handle = handle.clone();
            let tracker = app_state.shutdown_tracker.clone();
            async move {
                shutdown_signal.await;
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

/// Start the MCP server over STDIO transport (stdin/stdout).
///
/// This is the entry point for `skit mcp`.  It initialises the engine, node
/// registry, and plugin manager (reusing [`create_app_state`]) and then
/// serves the MCP protocol over stdin/stdout until the client disconnects
/// or a shutdown signal (Ctrl-C / SIGTERM) is received.
///
/// # Startup cost
///
/// `create_app_state` spawns full plugin loading and initialises MoQ + MSE
/// gateways even though STDIO never serves media.  This is intentional —
/// plugins must be loaded for `list_nodes` to surface them — but makes
/// cold-start heavier than a minimal RPC server.
///
/// # Shutdown behaviour
///
/// On exit the process terminates without draining in-flight session
/// shutdowns.  For STDIO this is acceptable (the OS reclaims resources),
/// but a `destroy_session` issued moments before exit may not complete
/// engine cleanup.
///
/// # Errors
///
/// Returns an error if the MCP STDIO server fails to initialise or encounters
/// a runtime error.
///
/// # Panics
///
/// Panics if the Ctrl-C or SIGTERM signal handler cannot be installed
/// (critical OS failure).
#[cfg(feature = "mcp")]
pub async fn start_mcp_stdio(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = create_app_state(config.clone(), None);

    let mcp = crate::mcp::StreamKitMcp::new(app_state);

    let ct = tokio_util::sync::CancellationToken::new();

    // Listen for Ctrl-C / SIGTERM and cancel the token so the STDIO
    // transport shuts down gracefully.
    // These expect() calls are justified and documented in the function's # Panics section.
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            #[allow(clippy::expect_used)]
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {},
                _ = sigterm.recv() => {},
            }
        }

        #[cfg(not(unix))]
        {
            #[allow(clippy::expect_used)]
            ctrl_c.await.expect("failed to install Ctrl+C handler");
        }

        info!("Received shutdown signal, stopping MCP STDIO server");
        ct_clone.cancel();
    });

    info!("Starting MCP server over STDIO transport");

    let service = rmcp::service::serve_server_with_ct(mcp, rmcp::transport::io::stdio(), ct)
        .await
        .map_err(|e| format!("Failed to initialize MCP STDIO server: {e}"))?;

    service.waiting().await?;

    info!("MCP STDIO server stopped");
    Ok(())
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
