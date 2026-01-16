// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Cookie building helpers for session management.
//!
//! Cookies are used for browser-based authentication. The session cookie
//! is HttpOnly and SameSite=Strict for security.

use crate::config::Config;
use axum::http::HeaderValue;

fn normalize_cookie_path(base_path: Option<&str>) -> String {
    let path = base_path.unwrap_or("/").trim();
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }

    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }

    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Build a session cookie header value for login.
///
/// The cookie is configured with:
/// - HttpOnly: Prevents JavaScript access (XSS protection)
/// - SameSite=Strict: Prevents CSRF attacks
/// - Secure: Only sent over HTTPS (if TLS is enabled)
/// - Path: Set to base_path for subpath deployment safety
pub fn build_session_cookie(
    token: &str,
    config: &Config,
    max_age_secs: u64,
) -> Option<HeaderValue> {
    let cookie_name = &config.auth.cookie_name;
    let secure = config.server.tls;

    // Path = base_path for subpath safety (or "/" if not set)
    let path = normalize_cookie_path(config.server.base_path.as_deref());

    let cookie = format!(
        "{cookie_name}={token}; HttpOnly; SameSite=Strict; Path={path}{}; Max-Age={max_age_secs}",
        if secure { "; Secure" } else { "" },
    );

    HeaderValue::from_str(&cookie).ok()
}

/// Build a logout cookie header value that clears the session.
///
/// Sets Max-Age=0 to immediately expire the cookie.
pub fn build_logout_cookie(config: &Config) -> Option<HeaderValue> {
    let cookie_name = &config.auth.cookie_name;
    let path = normalize_cookie_path(config.server.base_path.as_deref());
    let secure = config.server.tls;

    let cookie = format!(
        "{cookie_name}=; HttpOnly; SameSite=Strict; Path={path}{}; Max-Age=0",
        if secure { "; Secure" } else { "" },
    );

    HeaderValue::from_str(&cookie).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_session_cookie_basic() {
        let config = Config::default();
        let cookie = build_session_cookie("test-token", &config, 3600).unwrap();
        let cookie_str = cookie.to_str().unwrap();

        assert!(cookie_str.contains("skit_session=test-token"));
        assert!(cookie_str.contains("HttpOnly"));
        assert!(cookie_str.contains("SameSite=Strict"));
        assert!(cookie_str.contains("Path=/"));
        assert!(cookie_str.contains("Max-Age=3600"));
        // No Secure flag when TLS is not configured
        assert!(!cookie_str.contains("Secure"));
    }

    #[test]
    fn test_session_cookie_with_base_path() {
        let mut config = Config::default();
        config.server.base_path = Some("/api/v1".to_string());

        let cookie = build_session_cookie("test-token", &config, 3600).unwrap();
        let cookie_str = cookie.to_str().unwrap();

        assert!(cookie_str.contains("Path=/api/v1"));
    }

    #[test]
    fn test_logout_cookie() {
        let config = Config::default();
        let cookie = build_logout_cookie(&config).unwrap();
        let cookie_str = cookie.to_str().unwrap();

        assert!(cookie_str.contains("skit_session="));
        assert!(cookie_str.contains("Max-Age=0"));
        assert!(cookie_str.contains("HttpOnly"));
        assert!(!cookie_str.contains("Secure"));
    }

    #[test]
    fn test_logout_cookie_secure() {
        let mut config = Config::default();
        config.server.tls = true;
        let cookie = build_logout_cookie(&config).unwrap();
        let cookie_str = cookie.to_str().unwrap();

        assert!(cookie_str.contains("Secure"));
    }
}
