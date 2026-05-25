// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;

use axum::{
    body::Body,
    extract::{
        multipart::MultipartError, ws::WebSocketUpgrade, DefaultBodyLimit, MatchedPath, Path, State,
    },
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use futures::StreamExt;
use opentelemetry::{global, KeyValue};
use rust_embed::RustEmbed;
use serde::Serialize;
use tokio_stream::wrappers::ReceiverStream;
use tower::limit::ConcurrencyLimitLayer;
use tower::ServiceBuilder;
use tower_http::{
    cors::{AllowHeaders, AllowOrigin, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer},
};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::plugins::UnifiedPluginManager;
use crate::profiling;
use crate::session::SessionManager;
use crate::state::AppState;
use crate::websocket;
use streamkit_api::ApiPipeline;
use streamkit_core::error::StreamKitError;
use streamkit_engine::Engine;

#[cfg(feature = "profiling")]
use axum::extract::Query;

mod moq;
mod oneshot;
mod plugins;
mod sessions;
mod validation;

pub use sessions::{apply_batch_operations, tune_session_node, validate_batch_operations};

// consumed by crate::mcp (lib target only); unused in the binary target
#[cfg(feature = "mcp")]
#[allow(unused_imports)]
pub use sessions::{
    create_dynamic_session, populate_session_pipeline, send_pipeline_to_engine,
    tune_session_node_replace, CreateSessionError, CreateSessionResult,
};
#[cfg(feature = "mcp")]
#[allow(unused_imports)]
pub use validation::validate_pipeline_yaml;
#[allow(unused_imports)]
pub use validation::{
    check_file_path_security, is_synthetic_kind, synthetic_node_definitions, DiagnosticKind,
    PipelineMode, ValidateDiagnostic, ValidateGraph, ValidateGraphConnection, ValidateGraphNode,
    ValidateResponse,
};

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
        .and_then(|p| if p.is_empty() { None } else { Some(p) })
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
        if let Some(origin_val) = req.headers().get(header::ORIGIN) {
            let Ok(origin) = origin_val.to_str() else {
                warn!(
                    method = %method,
                    path = %path,
                    "Rejected request: Origin header is not valid UTF-8"
                );
                return (StatusCode::FORBIDDEN, "Invalid Origin header").into_response();
            };

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
// test fixtures use unwrap/unwrap_err for explicit setup failures
#[allow(clippy::unwrap_used)]
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
    fn cors_layer_does_not_panic_when_credentials_enabled() {
        let cors_config = crate::config::CorsConfig::default();
        let layer = create_cors_layer(&cors_config, false).unwrap();

        // `CorsLayer` validates its configuration when layered; this should not panic.
        let _app = axum::Router::<()>::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(layer);
    }

    #[test]
    fn origin_matches_pattern_wildcard_accepts_anything() {
        assert!(origin_matches_pattern("http://anything", "*"));
        assert!(origin_matches_pattern("https://example.com", "*"));
        assert!(origin_matches_pattern("", "*"));
    }

    #[test]
    fn cors_layer_rejects_wildcard_with_auth_enabled() {
        let cors = crate::config::CorsConfig { allowed_origins: vec!["*".to_string()] };
        let err = create_cors_layer(&cors, true).unwrap_err();
        assert!(err.contains("incompatible with auth"), "got: {err}");
    }

    #[test]
    fn cors_layer_wildcard_without_auth_succeeds() {
        let cors = crate::config::CorsConfig { allowed_origins: vec!["*".to_string()] };
        let layer = create_cors_layer(&cors, false).unwrap();
        let _app = axum::Router::<()>::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(layer);
    }

    #[test]
    fn cors_layer_empty_origins_yields_restrictive_layer() {
        let cors = crate::config::CorsConfig { allowed_origins: vec![] };
        let layer = create_cors_layer(&cors, true).unwrap();
        let _app = axum::Router::<()>::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(layer);
    }

    #[test]
    fn cors_layer_specific_origins_with_auth_succeeds() {
        let cors = crate::config::CorsConfig {
            allowed_origins: vec![
                "http://localhost:5173".to_string(),
                "http://localhost:*".to_string(),
            ],
        };
        let layer = create_cors_layer(&cors, true).unwrap();
        let _app = axum::Router::<()>::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(layer);
    }
}

#[cfg(test)]
// test fixtures use unwrap/expect for explicit setup failures
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod helper_tests {
    use super::{
        build_hash, cors_allowed_origins_are_loopback_only, escape_html_attr, normalize_base_path,
        strip_base_path_prefix,
    };

    #[test]
    fn escape_html_attr_replaces_dangerous_chars() {
        let cases = [
            ("", ""),
            ("plain", "plain"),
            ("&", "&amp;"),
            ("<", "&lt;"),
            (">", "&gt;"),
            ("\"", "&quot;"),
            ("'", "&#39;"),
            ("a<b>&'\"c", "a&lt;b&gt;&amp;&#39;&quot;c"),
        ];
        for (input, expected) in cases {
            assert_eq!(escape_html_attr(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn normalize_base_path_handles_edge_cases() {
        assert_eq!(normalize_base_path(None), None);
        assert_eq!(normalize_base_path(Some("")), None);
        assert_eq!(normalize_base_path(Some("   ")), None);

        assert_eq!(normalize_base_path(Some("/")), None);
        assert_eq!(normalize_base_path(Some("///")), None);

        assert_eq!(normalize_base_path(Some("/foo")).as_deref(), Some("/foo"));
        assert_eq!(normalize_base_path(Some("foo")).as_deref(), Some("/foo"));
        assert_eq!(normalize_base_path(Some("/foo/")).as_deref(), Some("/foo"));
        assert_eq!(normalize_base_path(Some("  /foo/bar/  ")).as_deref(), Some("/foo/bar"));
    }

    #[test]
    fn strip_base_path_prefix_passthrough_when_no_base() {
        assert_eq!(strip_base_path_prefix("/api/x", None), "/api/x");
        assert_eq!(strip_base_path_prefix("/api/x", Some("")), "/api/x");
        assert_eq!(strip_base_path_prefix("/api/x", Some("/")), "/api/x");
    }

    #[test]
    fn strip_base_path_prefix_strips_exact_match_to_root() {
        assert_eq!(strip_base_path_prefix("/admin", Some("/admin")), "/");
        assert_eq!(strip_base_path_prefix("/admin/", Some("/admin")), "/");
        assert_eq!(strip_base_path_prefix("/admin/api/x", Some("/admin")), "/api/x");
    }

    #[test]
    fn strip_base_path_prefix_only_strips_on_boundary() {
        assert_eq!(strip_base_path_prefix("/administrator", Some("/admin")), "/administrator");
        assert_eq!(strip_base_path_prefix("/other/path", Some("/admin")), "/other/path");
    }

    #[test]
    fn strip_base_path_prefix_handles_base_without_leading_slash() {
        assert_eq!(strip_base_path_prefix("/admin/api", Some("admin")), "/api");
        assert_eq!(strip_base_path_prefix("/admin", Some("admin")), "/");
        assert_eq!(strip_base_path_prefix("/administrator", Some("admin")), "/administrator");
        assert_eq!(strip_base_path_prefix("no-leading-slash", Some("admin")), "no-leading-slash");
    }

    #[test]
    fn cors_allowed_origins_are_loopback_only_basic_cases() {
        assert!(!cors_allowed_origins_are_loopback_only(&[]));
        assert!(cors_allowed_origins_are_loopback_only(&["http://localhost:80".to_string()]));
        assert!(cors_allowed_origins_are_loopback_only(&[
            "http://localhost:*".to_string(),
            "http://127.0.0.1:*".to_string(),
        ]));
        // A wildcard pattern matches every test origin (including loopbacks) — still considered loopback-only.
        assert!(cors_allowed_origins_are_loopback_only(&["*".to_string()]));
        // Mixed list: anything non-loopback flips the result.
        assert!(!cors_allowed_origins_are_loopback_only(&[
            "http://localhost:*".to_string(),
            "https://example.com".to_string(),
        ]));
    }

    #[test]
    fn build_hash_returns_non_empty_string() {
        let h = build_hash();
        assert!(!h.is_empty());
        assert!(h.len() >= 7 || h == "unknown", "unexpected build_hash value: {h:?}");
    }
}

#[cfg(test)]
// test fixtures use unwrap/expect for explicit setup failures
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod app_integration_tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use tower::ServiceExt;

    fn default_config() -> Config {
        let mut config = Config::default();
        // Loopback origins so origin_guard accepts requests from "http://localhost:5173".
        config.server.cors.allowed_origins = vec!["http://localhost:5173".to_string()];
        // Pin a role header so tests are hermetic regardless of $SK_ROLE.
        config.permissions.role_header = Some("x-test-role".to_string());
        config
    }

    async fn read_body(body: Body) -> Vec<u8> {
        to_bytes(body, 16 * 1024 * 1024).await.unwrap().to_vec()
    }

    async fn read_json(body: Body) -> serde_json::Value {
        let bytes = read_body(body).await;
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_handler_returns_ok_json() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Security headers from the response-header layers.
        assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(resp.headers().get("referrer-policy").unwrap(), "no-referrer");
        assert_eq!(resp.headers().get("x-frame-options").unwrap(), "SAMEORIGIN");
        let json = read_json(resp.into_body()).await;
        assert_eq!(json["status"], "ok");
        assert!(json["version"].is_string());
        assert!(json["build_hash"].is_string());
    }

    #[tokio::test]
    async fn jwks_returns_404_when_auth_disabled() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/.well-known/jwks.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn jwks_returns_keys_when_auth_enabled() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut config = default_config();
        config.auth.mode = crate::config::AuthMode::Enabled;
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth = Arc::new(
            crate::auth::AuthState::new(&config.auth, true).await.expect("init auth state"),
        );

        let (app, _state) = create_app(config, Some(auth.clone()));

        // Read the admin token so we can pass auth_guard for protected endpoints below.
        let admin_token = tokio::fs::read_to_string(temp.path().join("admin.token")).await.unwrap();
        let _admin_token = admin_token.trim().to_string();

        let resp = app
            .oneshot(Request::builder().uri("/.well-known/jwks.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert!(json["keys"].is_array(), "expected JWKS shape, got: {json}");
    }

    #[tokio::test]
    async fn permissions_handler_returns_default_role() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/permissions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert!(json["role"].is_string(), "got: {json}");
        assert!(json["permissions"].is_object(), "got: {json}");
    }

    #[tokio::test]
    async fn config_handler_allowed_for_admin_role() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .header("x-test-role", "admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn config_handler_forbidden_for_viewer_role() {
        let mut config = default_config();
        config
            .permissions
            .roles
            .insert("viewer".to_string(), crate::permissions::Permissions::viewer());

        let (app, _state) = create_app(config, None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .header("x-test-role", "viewer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_node_definitions_returns_array_including_synthetics() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/schema/nodes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        let arr = json.as_array().expect("expected array");
        // At minimum the two synthetic nodes appear.
        let kinds: Vec<&str> = arr.iter().filter_map(|n| n["kind"].as_str()).collect();
        assert!(kinds.contains(&"streamkit::http_input"), "got: {kinds:?}");
        assert!(kinds.contains(&"streamkit::http_output"), "got: {kinds:?}");
    }

    #[tokio::test]
    async fn list_packet_types_returns_array() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/schema/packets").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert!(!json.as_array().expect("expected array").is_empty());
    }

    #[tokio::test]
    async fn get_pipeline_handler_returns_404_for_unknown_session() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sessions/does-not-exist/pipeline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_pipeline_handler_returns_403_when_caller_lacks_list_sessions() {
        let mut config = default_config();
        let mut limited = crate::permissions::Permissions::viewer();
        limited.list_sessions = false;
        config.permissions.roles.insert("limited".to_string(), limited);

        let (app, _state) = create_app(config, None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sessions/anything/pipeline")
                    .header("x-test-role", "limited")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn mse_stream_handler_returns_404_for_unregistered_path() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/mse/does-not-exist").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_handler_serves_index_html_at_root() {
        let (app, _state) = create_app(default_config(), None);
        let resp =
            app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let body = read_body(resp.into_body()).await;
        let html = String::from_utf8(body).unwrap();
        if status == StatusCode::OK {
            assert!(html.contains("<base href=\"/\">"), "expected base injection in: {html}");
        } else {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[tokio::test]
    async fn static_handler_falls_back_to_index_for_unknown_spa_route() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/some/unknown/spa/route").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = read_body(resp.into_body()).await;
        let html = String::from_utf8(body).unwrap();
        if status == StatusCode::OK {
            assert!(
                html.contains("<base href=\"/\">"),
                "SPA fallback should serve index.html with base tag, got: {html}"
            );
        } else {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[tokio::test]
    async fn static_handler_returns_404_for_unknown_asset_with_extension() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(Request::builder().uri("/missing-asset.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_handler_returns_404_for_api_route_in_fallback() {
        // When an /api/* route isn't matched by an explicit handler, static_handler returns 404
        // (not the SPA fallback) so callers get a clear error instead of HTML.
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder().uri("/api/v1/this/does/not/exist").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn base_path_nesting_serves_health_under_prefix() {
        let mut config = default_config();
        config.server.base_path = Some("/admin".to_string());
        let (app, _state) = create_app(config, None);

        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/admin/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // The root path also still works (router is nested *and* served at /).
        let resp = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_guard_allows_protected_route_when_auth_disabled() {
        let (app, _state) = create_app(default_config(), None);
        // Default Config has auth disabled, so /api/v1/permissions is reachable without a token.
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/permissions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_guard_rejects_protected_route_without_token_when_enabled() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut config = default_config();
        config.auth.mode = crate::config::AuthMode::Enabled;
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth = Arc::new(
            crate::auth::AuthState::new(&config.auth, true).await.expect("init auth state"),
        );
        let (app, _state) = create_app(config, Some(auth));

        let resp = app
            .oneshot(Request::builder().uri("/api/v1/permissions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_guard_permits_protected_route_with_admin_token() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut config = default_config();
        config.auth.mode = crate::config::AuthMode::Enabled;
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth = Arc::new(
            crate::auth::AuthState::new(&config.auth, true).await.expect("init auth state"),
        );
        let (app, _state) = create_app(config, Some(auth));

        let token = tokio::fs::read_to_string(temp.path().join("admin.token")).await.unwrap();
        let token = token.trim().to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/permissions")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_guard_skips_auth_subrouter_when_enabled() {
        // /api/v1/auth/* paths must be reachable without a token so login can happen.
        let temp = tempfile::TempDir::new().unwrap();
        let mut config = default_config();
        config.auth.mode = crate::config::AuthMode::Enabled;
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth = Arc::new(
            crate::auth::AuthState::new(&config.auth, true).await.expect("init auth state"),
        );
        let (app, _state) = create_app(config, Some(auth));

        let resp = app
            .oneshot(Request::builder().uri("/api/v1/auth/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // me_handler returns 200 + JSON { authenticated: false } when no token
        // is provided. auth_guard_middleware would return 401 + plain text.
        assert_eq!(resp.status(), StatusCode::OK);
        let json = read_json(resp.into_body()).await;
        assert_eq!(json["authenticated"], false);
        assert_eq!(json["auth_enabled"], true);
    }

    #[tokio::test]
    async fn origin_guard_allows_get_with_disallowed_origin() {
        // GETs are not gated by origin_guard, regardless of Origin header value.
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/permissions")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn origin_guard_blocks_mutating_request_from_disallowed_origin() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .body(Body::from("{\"yaml\":\"nodes: {}\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn origin_guard_allows_mutating_request_from_allowed_origin() {
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .header("x-test-role", "admin")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::from("{\"yaml\":\"nodes: {}\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn origin_guard_allows_mutating_request_without_origin_header() {
        // Non-browser clients (curl, server-to-server) may omit Origin entirely.
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .header("x-test-role", "admin")
                    .body(Body::from("{\"yaml\":\"nodes: {}\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn origin_guard_rejects_mutating_request_with_non_utf8_origin() {
        // Invalid Origin bytes should be rejected rather than treated like an absent header.
        let (app, _state) = create_app(default_config(), None);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/validate")
                    .header("content-type", "application/json")
                    .header("x-test-role", "admin")
                    .header(
                        header::ORIGIN,
                        axum::http::HeaderValue::from_bytes(b"https://evil\x80.example.com")
                            .unwrap(),
                    )
                    .body(Body::from("{\"yaml\":\"nodes: {}\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn read_registry_returns_guard() {
        let state = create_app_state(default_config(), None);
        let count = {
            let guard = super::read_registry(&state).expect("registry read");
            guard.definitions().len()
        };
        assert!(count > 0);
    }

    #[tokio::test]
    async fn normalized_base_path_for_html_default_is_empty() {
        let state = create_app_state(default_config(), None);
        assert_eq!(super::normalized_base_path_for_html(&state), "");
    }

    #[tokio::test]
    async fn normalized_base_path_for_html_uses_normalized_value() {
        let mut config = default_config();
        config.server.base_path = Some("/admin/".to_string());
        let state = create_app_state(config, None);
        assert_eq!(super::normalized_base_path_for_html(&state), "/admin");
    }

    #[tokio::test]
    async fn app_error_into_response_maps_variants() {
        use axum::response::IntoResponse;
        use streamkit_core::error::StreamKitError;

        let bad = super::AppError::BadRequest("nope".to_string()).into_response();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

        let forb = super::AppError::Forbidden("denied".to_string()).into_response();
        assert_eq!(forb.status(), StatusCode::FORBIDDEN);

        let compile = super::AppError::PipelineCompilation("oops".to_string()).into_response();
        assert_eq!(compile.status(), StatusCode::BAD_REQUEST);

        // Force a real serde_saphyr error to exercise the Serde -> 400 mapping.
        let serde_err: serde_saphyr::Error =
            serde_saphyr::from_str::<serde_json::Value>("::: not yaml :::").unwrap_err();
        let resp = super::AppError::Serde(serde_err).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let engine =
            super::AppError::Engine(StreamKitError::Runtime("boom".to_string())).into_response();
        assert_eq!(engine.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = String::from_utf8(read_body(engine.into_body()).await).unwrap();
        assert!(!body.is_empty(), "Engine error should produce a response body");

        // MultipartError's constructor is crate-private (wraps multer::Error),
        // so AppError::Multipart cannot be exercised here.
    }

    #[tokio::test]
    async fn static_handler_serves_assets_under_base_path() {
        let mut config = default_config();
        config.server.base_path = Some("/admin".to_string());
        let (app, _state) = create_app(config, None);
        let resp = app
            .oneshot(Request::builder().uri("/admin/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = read_body(resp.into_body()).await;
        let html = String::from_utf8(body).unwrap();
        if status == StatusCode::OK {
            assert!(html.contains("<base href=\"/admin/\">"), "missing base tag in: {html}");
        } else {
            // ui/dist not built — tolerate 500 like sibling tests.
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        }
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

#[cfg(feature = "moq")]
pub mod preview;

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

        let secrets = validation::load_script_secrets(&config.script.secrets);

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

    let asset_root = config.asset_root.clone().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

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
        asset_root,
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

    let mut oneshot_route = post(oneshot::process_oneshot_pipeline_handler)
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
        .route("/api/v1/marketplace/registries", get(plugins::list_marketplace_registries_handler))
        .route("/api/v1/marketplace/plugins", get(plugins::list_marketplace_plugins_handler))
        .route(
            "/api/v1/marketplace/plugins/{plugin_id}",
            get(plugins::get_marketplace_plugin_handler),
        )
        .route("/api/v1/plugins/install", post(plugins::install_plugin_handler))
        .route(
            "/api/v1/plugins",
            get(plugins::list_plugins_handler)
                .post(plugins::upload_plugin_handler)
                // Plugin uploads are multipart; raise default body limit for realistic artifacts.
                .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size)),
        )
        .route("/api/v1/plugins/{kind}", delete(plugins::delete_plugin_handler))
        .route("/api/v1/jobs/{job_id}", get(plugins::get_job_handler))
        .route("/api/v1/jobs/{job_id}/cancel", post(plugins::cancel_job_handler))
        .route("/api/v1/control", get(websocket_handler))
        .route("/api/v1/permissions", get(get_permissions_handler))
        .route("/api/v1/config", get(get_config_handler))
        .route("/api/v1/schema/nodes", get(list_node_definitions_handler))
        .route("/api/v1/schema/packets", get(list_packet_types_handler))
        .route(
            "/api/v1/validate",
            post(validation::validate_pipeline_handler)
                .layer(DefaultBodyLimit::max(app_state.config.server.max_body_size)),
        )
        .route("/api/v1/logs", get(crate::log_viewer::get_logs_handler))
        .route("/api/v1/logs/stream", get(crate::log_viewer::stream_logs_handler))
        .route(
            "/api/v1/sessions",
            get(sessions::list_sessions_handler).post(sessions::create_session_handler),
        )
        .route("/api/v1/sessions/{id}", delete(sessions::destroy_session_handler))
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
    let base_path = normalize_base_path(app_state.config.server.base_path.as_deref());

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
    moq::start_moq_webtransport_acceptor(&app_state, config)?;

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
