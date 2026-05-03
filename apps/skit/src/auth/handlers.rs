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
