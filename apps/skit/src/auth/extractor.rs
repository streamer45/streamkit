// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Authentication context and token extraction utilities.
//!
//! This module provides utilities for extracting and validating JWT tokens
//! from HTTP requests. The main types are:
//!
//! - `AuthContext`: Represents an authenticated request with validated claims
//! - `MaybeAuth`: Optional authentication that never fails
//!
//! These can be used by handlers to check authentication status.

use super::{ApiClaims, AuthState};
use crate::config::Config;
use crate::permissions::Permissions;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{HeaderMap, StatusCode};

/// Authenticated request context.
///
/// Contains the validated JWT claims, role name, and resolved permissions.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// The validated JWT claims
    pub claims: ApiClaims,
    /// The role name from the token
    pub role: String,
    /// The permissions associated with this role
    #[allow(dead_code)]
    pub permissions: Permissions,
}

#[allow(dead_code)]
impl AuthContext {
    /// Get the JWT ID (for revocation tracking)
    pub fn jti(&self) -> &str {
        &self.claims.jti
    }

    /// Get the subject (token holder identifier)
    pub fn sub(&self) -> &str {
        &self.claims.sub
    }
}

/// Optional authenticated request context.
///
/// This type never fails - when authentication is not provided or fails,
/// it contains `None`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MaybeAuth(pub Option<AuthContext>);

#[allow(dead_code)]
impl MaybeAuth {
    /// Get the auth context if present
    pub const fn context(&self) -> Option<&AuthContext> {
        self.0.as_ref()
    }

    /// Check if the request is authenticated
    pub const fn is_authenticated(&self) -> bool {
        self.0.is_some()
    }

    /// Get a reference to the inner Option
    #[allow(clippy::ref_option)]
    pub const fn as_option(&self) -> &Option<AuthContext> {
        &self.0
    }

    /// Unwrap or return an unauthorized error.
    ///
    /// # Errors
    ///
    /// Returns `(StatusCode::UNAUTHORIZED, ...)` if not authenticated.
    pub fn require(self) -> Result<AuthContext, (StatusCode, String)> {
        self.0.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Authentication required".to_string()))
    }
}

/// Extract token from Authorization header or cookie.
///
/// Checks the Authorization header first (Bearer token format),
/// then falls back to the configured cookie name.
pub fn extract_token(headers: &HeaderMap, config: &Config) -> Option<String> {
    if let Some(auth_header) = headers.get(AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }

    if let Some(cookie_header) = headers.get(COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            let cookie_name = &config.auth.cookie_name;
            for cookie in cookie_str.split(';') {
                let cookie = cookie.trim();
                if let Some(value) = cookie.strip_prefix(&format!("{cookie_name}=")) {
                    return Some(value.to_string());
                }
            }
        }
    }

    None
}

/// Validate a token and return the AuthContext.
///
/// This function performs:
/// 1. Token extraction from headers
/// 2. JWT signature verification
/// 3. Revocation check
/// 4. "Tokens we mint" enforcement
/// 5. Permission resolution
///
/// # Errors
///
/// Returns `(StatusCode, String)` on authentication failure.
pub async fn validate_token_from_headers(
    headers: &HeaderMap,
    auth_state: &AuthState,
    config: &Config,
    permissions_config: &crate::permissions::PermissionsConfig,
) -> Result<AuthContext, (StatusCode, String)> {
    let token = extract_token(headers, config).ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, "No authentication token provided".to_string())
    })?;

    validate_token(&token, auth_state, permissions_config).await
}

/// Validate a raw token string and return the AuthContext.
///
/// # Errors
///
/// Returns `(StatusCode, String)` on authentication failure.
pub async fn validate_token(
    token: &str,
    auth_state: &AuthState,
    permissions_config: &crate::permissions::PermissionsConfig,
) -> Result<AuthContext, (StatusCode, String)> {
    let claims = auth_state
        .validate_api_token(token)
        .map_err(|e| (StatusCode::UNAUTHORIZED, format!("Invalid token: {e}")))?;

    let token_hash = super::hash_token(token);

    if let Some(revocation_store) = auth_state.revocation_store() {
        if revocation_store.is_revoked(&token_hash) {
            return Err((StatusCode::UNAUTHORIZED, "Token has been revoked".to_string()));
        }
    }

    if let Some(metadata_store) = auth_state.token_metadata_store() {
        if !metadata_store.exists(&claims.jti).await {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Token not recognized (not minted by this server)".to_string(),
            ));
        }
    }

    if !permissions_config.roles.contains_key(&claims.role) {
        return Err((StatusCode::UNAUTHORIZED, "Token has unknown role".to_string()));
    }

    let permissions = permissions_config.get_role(&claims.role);

    Ok(AuthContext { role: claims.role.clone(), claims, permissions })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::http::header::HeaderValue;

    fn make_headers(auth: Option<&str>, cookie: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(auth) = auth {
            headers.insert(AUTHORIZATION, HeaderValue::from_str(auth).unwrap());
        }
        if let Some(cookie) = cookie {
            headers.insert(COOKIE, HeaderValue::from_str(cookie).unwrap());
        }
        headers
    }

    #[test]
    fn test_extract_bearer_token() {
        let config = Config::default();
        let headers = make_headers(Some("Bearer my-token-123"), None);

        let token = extract_token(&headers, &config);
        assert_eq!(token, Some("my-token-123".to_string()));
    }

    #[test]
    fn test_extract_cookie_token() {
        let mut config = Config::default();
        config.auth.cookie_name = "skit_session".to_string();

        let headers =
            make_headers(None, Some("other=value; skit_session=cookie-token-456; another=x"));

        let token = extract_token(&headers, &config);
        assert_eq!(token, Some("cookie-token-456".to_string()));
    }

    #[test]
    fn test_bearer_takes_precedence() {
        let mut config = Config::default();
        config.auth.cookie_name = "skit_session".to_string();

        let headers = make_headers(Some("Bearer bearer-token"), Some("skit_session=cookie-token"));

        let token = extract_token(&headers, &config);
        assert_eq!(token, Some("bearer-token".to_string()));
    }

    #[test]
    fn test_no_token() {
        let config = Config::default();
        let headers = make_headers(None, None);

        let token = extract_token(&headers, &config);
        assert!(token.is_none());
    }

    #[test]
    fn test_invalid_auth_header_format() {
        let config = Config::default();
        let headers = make_headers(Some("Basic dXNlcjpwYXNz"), None);

        let token = extract_token(&headers, &config);
        assert!(token.is_none());
    }

    #[test]
    fn test_maybe_auth_require() {
        let auth = MaybeAuth(None);
        assert!(auth.require().is_err());

        let ctx = AuthContext {
            claims: ApiClaims::anonymous("admin"),
            role: "admin".to_string(),
            permissions: crate::permissions::Permissions::admin(),
        };
        let auth = MaybeAuth(Some(ctx));
        assert!(auth.require().is_ok());
    }
}
