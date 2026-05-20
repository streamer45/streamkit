// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! HTTP handlers for authentication endpoints.
//!
//! These handlers provide:
//! - `/api/v1/auth/login` - Verify token and set session cookie
//! - `/api/v1/auth/logout` - Clear session cookie
//! - `/api/v1/auth/me` - Get current auth status and role
//! - `/api/v1/auth/tokens` - List/create/revoke API tokens (admin only)
//! - `/api/v1/auth/moq-tokens` - Create MoQ tokens (admin only)

use crate::auth::{
    build_logout_cookie, build_session_cookie, validate_token, validate_token_from_headers,
    AuthContext,
};
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{header::SET_COOKIE, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::limit::ConcurrencyLimitLayer;

const AUTH_MAX_BODY_BYTES: usize = 64 * 1024;
const AUTH_MAX_CONCURRENCY: usize = 64;
const RELOAD_KEYS_MAX_CONCURRENCY: usize = 1;

/// Request body for login endpoint.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// The API token to validate and set as session cookie
    pub token: String,
}

/// Response for /me endpoint.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub authenticated: bool,
    pub auth_enabled: bool,
    pub role: Option<String>,
    pub jti: Option<String>,
}

/// Request body for creating an API token.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateApiTokenRequest {
    pub role: String,
    #[serde(default)]
    pub label: Option<String>,
    /// TTL in seconds (uses default if not specified)
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

/// Request body for creating a MoQ token.
#[derive(Debug, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct CreateMoqTokenRequest {
    pub root: String,
    #[serde(default)]
    pub subscribe: Vec<String>,
    #[serde(default)]
    pub publish: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// TTL in seconds (uses default if not specified)
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

/// Response for token creation.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTokenResponse {
    pub token: String,
    pub jti: String,
    pub exp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_template: Option<String>,
}

/// Token info for listing.
#[derive(Debug, Serialize)]
pub struct TokenInfo {
    pub jti: String,
    pub token_type: String,
    pub role: Option<String>,
    pub label: Option<String>,
    pub created_at: u64,
    pub exp: u64,
    pub revoked: bool,
    pub created_by: String,
}

/// Helper to get auth context from headers, returning appropriate errors.
async fn get_auth_context(
    headers: &HeaderMap,
    app_state: &AppState,
) -> Result<AuthContext, (StatusCode, String)> {
    if !app_state.auth.is_enabled() {
        let (role, permissions) =
            crate::role_extractor::get_role_and_permissions(headers, &Arc::new(app_state.clone()));
        return Ok(AuthContext {
            claims: crate::auth::ApiClaims::anonymous(&role),
            role,
            permissions,
        });
    }

    validate_token_from_headers(
        headers,
        &app_state.auth,
        &app_state.config,
        &app_state.config.permissions,
    )
    .await
}

/// Helper to require admin role.
fn require_admin(auth: &AuthContext) -> Result<(), (StatusCode, String)> {
    if auth.role != "admin" {
        return Err((StatusCode::FORBIDDEN, "Admin role required".to_string()));
    }
    Ok(())
}

/// POST /api/v1/auth/login
///
/// Validates a token and sets it as a session cookie.
pub async fn login_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Option<Json<LoginRequest>>,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let token_from_header = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer ").map(str::to_string));
    let token_from_body = payload.map(|Json(req)| req.token);
    let token = match token_from_header.or(token_from_body) {
        Some(token) if !token.trim().is_empty() => token,
        _ => return (StatusCode::BAD_REQUEST, "Missing token".to_string()).into_response(),
    };

    let auth_ctx =
        match validate_token(&token, &app_state.auth, &app_state.config.permissions).await {
            Ok(ctx) => ctx,
            Err((status, msg)) => return (status, msg).into_response(),
        };

    let now = match crate::auth::now_secs() {
        Ok(now) => now,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let Some(cookie) =
        build_session_cookie(&token, &app_state.config, auth_ctx.claims.exp.saturating_sub(now))
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build cookie".to_string())
            .into_response();
    };

    (StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response()
}

/// POST /api/v1/auth/logout
///
/// Clears the session cookie.
pub async fn logout_handler(State(app_state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(cookie) = build_logout_cookie(&app_state.config) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build cookie".to_string())
            .into_response();
    };

    (StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response()
}

/// GET /api/v1/auth/me
///
/// Returns current authentication status.
pub async fn me_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let auth_enabled = app_state.auth.is_enabled();

    if !auth_enabled {
        let (role, _) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
        return Json(MeResponse {
            authenticated: true, // Consider everyone authenticated when auth is disabled
            auth_enabled: false,
            role: Some(role),
            jti: None,
        });
    }

    match get_auth_context(&headers, &app_state).await {
        Ok(auth_ctx) => Json(MeResponse {
            authenticated: true,
            auth_enabled: true,
            role: Some(auth_ctx.role),
            jti: Some(auth_ctx.claims.jti),
        }),
        Err(_) => {
            Json(MeResponse { authenticated: false, auth_enabled: true, role: None, jti: None })
        },
    }
}

/// POST /api/v1/auth/tokens
///
/// Create a new API token (admin only).
pub async fn create_token_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateApiTokenRequest>,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let auth_ctx = match get_auth_context(&headers, &app_state).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = require_admin(&auth_ctx) {
        return e.into_response();
    }

    if !app_state.config.permissions.roles.contains_key(&req.role) {
        return (StatusCode::BAD_REQUEST, "Unknown role".to_string()).into_response();
    }

    let ttl = req.ttl_secs.unwrap_or(app_state.config.auth.api_default_ttl_secs);

    let (token, meta) = match app_state
        .auth
        .mint_api_token(&req.role, req.label.as_deref(), ttl, &auth_ctx.claims.jti)
        .await
    {
        Ok(result) => result,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Failed to create token: {e}"))
                .into_response()
        },
    };

    Json(CreateTokenResponse { token, jti: meta.jti, exp: meta.exp, url_template: None })
        .into_response()
}

/// GET /api/v1/auth/tokens
///
/// List all minted tokens (admin only).
pub async fn list_tokens_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let auth_ctx = match get_auth_context(&headers, &app_state).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = require_admin(&auth_ctx) {
        return e.into_response();
    }

    let Some(store) = app_state.auth.token_metadata_store() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Token metadata store not available".to_string(),
        )
            .into_response();
    };

    let tokens = match store.list().await {
        Ok(t) => t,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list tokens: {e}"))
                .into_response()
        },
    };

    let token_infos: Vec<TokenInfo> = tokens
        .into_iter()
        .map(|t| TokenInfo {
            jti: t.jti,
            token_type: t.token_type.to_string(),
            role: t.role,
            label: t.label,
            created_at: t.created_at,
            exp: t.exp,
            revoked: t.revoked,
            created_by: t.created_by,
        })
        .collect();

    Json(token_infos).into_response()
}

/// DELETE /api/v1/auth/tokens/:jti
///
/// Revoke a token by its jti (admin only).
pub async fn revoke_token_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(jti): Path<String>,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let auth_ctx = match get_auth_context(&headers, &app_state).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = require_admin(&auth_ctx) {
        return e.into_response();
    }

    // Prevent revoking own token
    if auth_ctx.claims.jti == jti {
        return (StatusCode::BAD_REQUEST, "Cannot revoke your own token".to_string())
            .into_response();
    }

    // Revoke the token
    match app_state.auth.revoke_token(&jti).await {
        Ok(()) => {},
        Err(crate::auth::AuthError::UnknownToken) => {
            return (StatusCode::NOT_FOUND, "Token not found".to_string()).into_response();
        },
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to revoke token: {e}"))
                .into_response();
        },
    }

    (StatusCode::OK, "Token revoked").into_response()
}

/// POST /api/v1/auth/moq-tokens
///
/// Create a new MoQ token (admin only).
#[cfg(feature = "moq")]
pub async fn create_moq_token_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateMoqTokenRequest>,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let auth_ctx = match get_auth_context(&headers, &app_state).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = require_admin(&auth_ctx) {
        return e.into_response();
    }

    let ttl = req.ttl_secs.unwrap_or(app_state.config.auth.moq_default_ttl_secs);

    let (token, meta) = match app_state
        .auth
        .mint_moq_token(
            &req.root,
            req.subscribe,
            req.publish,
            req.label.as_deref(),
            ttl,
            &auth_ctx.claims.jti,
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Failed to create MoQ token: {e}"))
                .into_response()
        },
    };

    let root_path =
        if req.root.starts_with('/') { req.root.clone() } else { format!("/{}", req.root) };

    // Prefer a full URL when the server is configured with a MoQ gateway URL (this matches what the
    // Stream view expects). Otherwise fall back to a relative path.
    let url_template = app_state
        .config
        .server
        .moq_gateway_url
        .as_deref()
        .and_then(|gateway_url| {
            let uri: axum::http::Uri = gateway_url.parse().ok()?;
            let scheme = uri.scheme_str()?;
            let authority = uri.authority()?.as_str();

            let mut query = String::new();
            if let Some(existing) = uri.query() {
                if !existing.is_empty() {
                    query.push_str(existing);
                    query.push('&');
                }
            }
            query.push_str("jwt=");
            query.push_str(&token);

            let path_and_query = format!("{root_path}?{query}");
            let uri = axum::http::Uri::builder()
                .scheme(scheme)
                .authority(authority)
                .path_and_query(path_and_query)
                .build()
                .ok()?;
            Some(uri.to_string())
        })
        .or_else(|| Some(format!("{root_path}?jwt={token}")));
    Json(CreateTokenResponse { token, jti: meta.jti, exp: meta.exp, url_template }).into_response()
}

/// POST /api/v1/auth/reload-keys
///
/// Reload signing/verification keys from disk (admin only).
///
/// Call this after an out-of-band key rotation (e.g. `skit auth rotate-key`)
/// so the running server picks up the new keys without a restart.
pub async fn reload_keys_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !app_state.auth.is_enabled() {
        return (StatusCode::BAD_REQUEST, "Authentication is disabled".to_string()).into_response();
    }

    let auth_ctx = match get_auth_context(&headers, &app_state).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = require_admin(&auth_ctx) {
        return e.into_response();
    }

    match app_state.auth.reload_keys().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to reload auth keys");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to reload keys: {e}"))
                .into_response()
        },
    }
}

/// Build the auth router with all authentication endpoints.
pub fn auth_router() -> axum::Router<Arc<AppState>> {
    use axum::routing::{delete, get, post};

    #[cfg_attr(not(feature = "moq"), allow(unused_mut))]
    let mut router = axum::Router::new()
        .route(
            "/login",
            post(login_handler).layer(ConcurrencyLimitLayer::new(AUTH_MAX_CONCURRENCY)),
        )
        .route("/logout", post(logout_handler))
        .route("/me", get(me_handler))
        .route(
            "/tokens",
            post(create_token_handler).layer(ConcurrencyLimitLayer::new(AUTH_MAX_CONCURRENCY)),
        )
        .route("/tokens", get(list_tokens_handler))
        .route(
            "/tokens/{jti}",
            delete(revoke_token_handler).layer(ConcurrencyLimitLayer::new(AUTH_MAX_CONCURRENCY)),
        )
        .route(
            "/reload-keys",
            post(reload_keys_handler)
                .layer(ConcurrencyLimitLayer::new(RELOAD_KEYS_MAX_CONCURRENCY)),
        );

    #[cfg(feature = "moq")]
    {
        router = router.route(
            "/moq-tokens",
            post(create_moq_token_handler).layer(ConcurrencyLimitLayer::new(AUTH_MAX_CONCURRENCY)),
        );
    }

    router.layer(DefaultBodyLimit::max(AUTH_MAX_BODY_BYTES))
}

#[cfg(test)]
mod tests {
    // Tests intentionally use unwrap/expect so any failure points directly at
    // the failed precondition (state setup, token decode, JSON parse) rather
    // than a propagated `?` from deep inside the test body.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::auth::ApiClaims;
    use crate::config::{AuthMode, Config};
    use axum::body::{to_bytes, Body};
    use axum::http::header::AUTHORIZATION as REQ_AUTHORIZATION;
    use axum::http::Request;
    use serde_json::json;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_state_auth_disabled() -> (Arc<AppState>, TempDir) {
        let temp = TempDir::new().unwrap();
        let mut config = Config::default();
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth = Arc::new(crate::auth::AuthState::disabled());
        let state = crate::server::create_app_state(config, Some(auth));
        (state, temp)
    }

    async fn make_state_auth_enabled() -> (Arc<AppState>, String, TempDir) {
        let temp = TempDir::new().unwrap();
        let mut config = Config::default();
        config.auth.mode = AuthMode::Enabled;
        config.auth.state_dir = temp.path().to_string_lossy().to_string();
        let auth_state =
            crate::auth::AuthState::new(&config.auth, true).await.expect("init auth state");
        let admin_token = tokio::fs::read_to_string(temp.path().join("admin.token")).await.unwrap();
        let admin_token = admin_token.trim().to_string();

        let state = crate::server::create_app_state(config, Some(Arc::new(auth_state)));
        (state, admin_token, temp)
    }

    fn build_router(state: Arc<AppState>) -> axum::Router {
        auth_router().with_state(state)
    }

    async fn body_to_string(body: Body) -> String {
        let bytes = to_bytes(body, 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn require_admin_accepts_admin_role() {
        let ctx = AuthContext {
            claims: ApiClaims::anonymous("admin"),
            role: "admin".to_string(),
            permissions: crate::permissions::Permissions::admin(),
        };
        require_admin(&ctx).expect("admin should be allowed");
    }

    #[test]
    fn require_admin_rejects_non_admin_role() {
        for role in ["viewer", "user", "guest", ""] {
            let ctx = AuthContext {
                claims: ApiClaims::anonymous(role),
                role: role.to_string(),
                permissions: crate::permissions::Permissions::viewer(),
            };
            let (status, msg) = require_admin(&ctx).expect_err("non-admin must be rejected");
            assert_eq!(status, StatusCode::FORBIDDEN, "role {role}");
            assert!(msg.contains("Admin"), "role {role}: {msg}");
        }
    }

    #[test]
    fn login_request_deserializes_token_field() {
        let parsed: LoginRequest = serde_json::from_str(r#"{"token":"abc"}"#).unwrap();
        assert_eq!(parsed.token, "abc");
    }

    #[test]
    fn create_api_token_request_defaults_optional_fields() {
        let parsed: CreateApiTokenRequest = serde_json::from_str(r#"{"role":"viewer"}"#).unwrap();
        assert_eq!(parsed.role, "viewer");
        assert!(parsed.label.is_none());
        assert!(parsed.ttl_secs.is_none());

        let full: CreateApiTokenRequest =
            serde_json::from_str(r#"{"role":"admin","label":"ci-bot","ttl_secs":3600}"#).unwrap();
        assert_eq!(full.role, "admin");
        assert_eq!(full.label.as_deref(), Some("ci-bot"));
        assert_eq!(full.ttl_secs, Some(3600));
    }

    #[cfg(feature = "moq")]
    #[test]
    fn create_moq_token_request_defaults_arrays_to_empty() {
        let parsed: CreateMoqTokenRequest = serde_json::from_str(r#"{"root":"/demo"}"#).unwrap();
        assert_eq!(parsed.root, "/demo");
        assert!(parsed.subscribe.is_empty());
        assert!(parsed.publish.is_empty());
        assert!(parsed.label.is_none());
        assert!(parsed.ttl_secs.is_none());
    }

    #[test]
    fn create_token_response_round_trips_through_serde() {
        let resp = CreateTokenResponse {
            token: "jwt.body.sig".into(),
            jti: "jti-1".into(),
            exp: 1_700_000_000,
            url_template: Some("/r?jwt=jwt.body.sig".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: CreateTokenResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token, "jwt.body.sig");
        assert_eq!(back.exp, 1_700_000_000);
        assert_eq!(back.url_template.as_deref(), Some("/r?jwt=jwt.body.sig"));
    }

    #[test]
    fn create_token_response_omits_null_url_template() {
        let resp =
            CreateTokenResponse { token: "t".into(), jti: "j".into(), exp: 0, url_template: None };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("url_template"), "url_template suppressed when None: {json}");
    }

    #[tokio::test]
    async fn me_returns_authenticated_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp =
            app.oneshot(Request::builder().uri("/me").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        assert_eq!(body["authenticated"], true);
        assert_eq!(body["auth_enabled"], false);
        assert!(body["jti"].is_null());
        assert!(body["role"].is_string());
    }

    #[tokio::test]
    async fn login_rejected_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"token":"anything"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_to_string(resp.into_body()).await;
        assert!(body.contains("Authentication is disabled"), "got: {body}");
    }

    #[tokio::test]
    async fn create_token_rejected_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"admin"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_tokens_rejected_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(Request::builder().uri("/tokens").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn revoke_token_rejected_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/tokens/some-jti")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reload_keys_rejected_when_auth_disabled() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/reload-keys").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn logout_returns_no_content_with_clearing_cookie() {
        let (state, _temp) = make_state_auth_disabled();
        let app = build_router(state);

        let resp = app
            .oneshot(Request::builder().method("POST").uri("/logout").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let cookie = resp.headers().get(SET_COOKIE).expect("Set-Cookie present");
        let cookie_str = cookie.to_str().unwrap();
        // Logout cookies must clear the session — usually an empty value plus an
        // immediate expiry.  Don't pin a specific spelling, just confirm both
        // markers appear so an accidental "set live cookie" regression fails here.
        assert!(
            cookie_str.contains("Max-Age=0")
                || cookie_str.contains("Expires=")
                || cookie_str.to_lowercase().contains("expires"),
            "logout cookie should expire: {cookie_str}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn me_with_valid_token_reports_role_and_jti() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        assert_eq!(body["authenticated"], true);
        assert_eq!(body["auth_enabled"], true);
        assert_eq!(body["role"], "admin");
        assert!(body["jti"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn me_without_token_when_auth_enabled_reports_unauthenticated() {
        let (state, _admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp =
            app.oneshot(Request::builder().uri("/me").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        assert_eq!(body["authenticated"], false);
        assert_eq!(body["auth_enabled"], true);
        assert!(body["role"].is_null());
        assert!(body["jti"].is_null());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_with_valid_token_sets_session_cookie() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let cookie_name = state.config.auth.cookie_name.clone();
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "token": admin_token }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let cookie = resp.headers().get(SET_COOKIE).expect("login should set session cookie");
        assert!(
            cookie.to_str().unwrap().starts_with(&format!("{cookie_name}=")),
            "cookie prefix matches configured name: {cookie:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_with_bearer_header_also_works() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        // No body — token must be picked up from Authorization header.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(resp.headers().get(SET_COOKIE).is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_without_token_returns_bad_request() {
        let (state, _admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"token":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_to_string(resp.into_body()).await;
        assert!(body.contains("Missing token"), "got: {body}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_with_invalid_token_returns_unauthorized() {
        let (state, _admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"token":"not.a.real.jwt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_token_requires_admin_token() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        // Mint a viewer token first using the admin token.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: CreateTokenResponse =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        let viewer_token = body.token;

        // Viewer token must be rejected with 403 when calling admin-only endpoint.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {viewer_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_token_rejects_unknown_role() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"definitely_not_a_role"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(body_to_string(resp.into_body()).await.contains("Unknown role"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_tokens_returns_minted_tokens() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;

        // Mint one viewer token via the API so list has at least two entries
        // (bootstrap admin + viewer).
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer","label":"ci"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // TokenInfo is serialize-only on the wire — parse to a Value to inspect
        // it without needing a parallel deserializable copy of the struct.
        let body: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        let tokens = body.as_array().expect("list response is a JSON array");
        let found = tokens.iter().any(|t| {
            t.get("label").and_then(|v| v.as_str()) == Some("ci")
                && t.get("role").and_then(|v| v.as_str()) == Some("viewer")
        });
        assert!(found, "list contains freshly minted viewer token; got: {body}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revoke_token_round_trip_then_revalidation_fails() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;

        // Mint a viewer token.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let minted: CreateTokenResponse =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();

        // Revoke it.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tokens/{}", minted.jti))
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Revoked /me must report unauthenticated.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(REQ_AUTHORIZATION, format!("Bearer {}", minted.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        assert_eq!(body["authenticated"], false);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revoke_unknown_token_returns_not_found() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/tokens/definitely-not-a-real-jti")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// Admins must not lock themselves out by revoking the token they're using.
    #[tokio::test(flavor = "multi_thread")]
    async fn revoke_own_token_is_rejected() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;

        // Fetch the admin token's jti via /me.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let me: serde_json::Value =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        let jti = me["jti"].as_str().unwrap().to_string();

        // Try to revoke own token.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tokens/{jti}"))
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_to_string(resp.into_body()).await;
        assert!(body.contains("Cannot revoke your own token"), "got: {body}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reload_keys_returns_no_content_for_admin() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reload-keys")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[cfg(feature = "moq")]
    #[tokio::test(flavor = "multi_thread")]
    async fn create_moq_token_round_trip() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/moq-tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"root":"/demo","subscribe":["/demo/audio"],"publish":[]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: CreateTokenResponse =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
        assert!(!body.token.is_empty());
        // Without a moq_gateway_url configured the handler falls back to a
        // relative URL template containing the jwt.
        let url = body.url_template.expect("url_template populated");
        assert!(url.starts_with("/demo?jwt="), "got: {url}");
        assert!(url.contains(&body.token));
    }

    #[cfg(feature = "moq")]
    #[tokio::test(flavor = "multi_thread")]
    async fn create_moq_token_requires_admin() {
        let (state, admin_token, _temp) = make_state_auth_enabled().await;
        let app = build_router(state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let viewer: CreateTokenResponse =
            serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/moq-tokens")
                    .header(REQ_AUTHORIZATION, format!("Bearer {}", viewer.token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"root":"/demo"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
