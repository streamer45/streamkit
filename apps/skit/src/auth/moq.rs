// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ authentication context and path verification.
//!
//! This module implements moq-relay compatible path reduction logic:
//! - The JWT `root` claim specifies the URL path prefix the token is valid for
//! - `subscribe`/`publish` are broadcast path prefixes (not URL paths)
//! - Permissions are reduced based on connection path depth
//!
//! # Path Matching
//!
//! Path matching uses segment-based comparison (like moq-relay), not string
//! `starts_with`. This prevents path confusion attacks:
//! - Token with `root: "/moq"` does NOT match `/moq2` (different segment)
//! - Token with `root: "/moq"` DOES match `/moq/session1` (prefix segments)

use super::{AuthError, MoqClaims};
use moq_lite::{Path, PathOwned};
use streamkit_core::moq_gateway::MoqAuthChecker;

/// Verified MoQ auth context after path reduction.
///
/// This struct represents the reduced permissions for a specific connection,
/// after validating the JWT and reducing permissions based on the connection
/// path depth (similar to moq-relay's `AuthToken`).
#[derive(Debug, Clone)]
pub struct MoqAuthContext {
    /// The actual connection path (after root validation)
    #[allow(dead_code)]
    pub root: PathOwned,
    /// Reduced subscribe permissions (broadcast paths relative to connection)
    pub subscribe: Vec<PathOwned>,
    /// Reduced publish permissions (broadcast paths relative to connection)
    pub publish: Vec<PathOwned>,
}

impl MoqAuthContext {
    fn check_permission(broadcast: &str, allowed: &[PathOwned]) -> bool {
        if allowed.is_empty() {
            return false;
        }

        let broadcast_path = Path::new(broadcast);

        // Check if any allowed path is a prefix of (or matches) the broadcast
        allowed.iter().any(|allowed_path| {
            if allowed_path.is_empty() {
                // [""] = root allowed = any broadcast
                true
            } else {
                // Segment-based prefix check: allowed_path must be a prefix of broadcast_path
                // This means broadcast_path should be able to strip allowed_path as prefix
                broadcast_path.strip_prefix(allowed_path).is_some()
            }
        })
    }
}

/// Implement the core trait for permission checking.
/// This allows nodes to check permissions without knowing the full MoqAuthContext type.
impl MoqAuthChecker for MoqAuthContext {
    fn can_subscribe(&self, broadcast: &str) -> bool {
        Self::check_permission(broadcast, &self.subscribe)
    }

    fn can_publish(&self, broadcast: &str) -> bool {
        Self::check_permission(broadcast, &self.publish)
    }
}

/// Verify MoQ JWT and reduce permissions based on connection path.
///
/// This function implements moq-relay style path reduction:
/// 1. Verify the connection URL path starts with the token's `root` (segment-based)
/// 2. Compute the suffix (connection path minus root)
/// 3. Reduce subscribe/publish permissions based on suffix depth
///
/// # Arguments
/// * `claims` - The validated MoQ JWT claims
/// * `connection_path` - The URL path from the WebTransport connection (e.g., "/moq/session1")
///
/// # Returns
/// * `Ok(MoqAuthContext)` - Reduced permissions for this connection
/// * `Err(AuthError)` - If root doesn't match connection path
///
/// # Errors
///
/// Returns `AuthError::Moq` if the connection path doesn't match the token's root.
///
/// # Example
/// ```ignore
/// // Token claims:
/// //   root: "/moq"
/// //   subscribe: ["session1/output", ""]
/// //   publish: ["session1/input"]
///
/// // Connection to "/moq/session1":
/// //   suffix = "session1"
/// //   subscribe reduced to: ["output", ""] (empty string = allow all)
/// //   publish reduced to: ["input"]
/// ```
pub fn verify_moq_token(
    claims: &MoqClaims,
    connection_path: &str,
) -> Result<MoqAuthContext, AuthError> {
    // Parse paths using moq_lite::Path for segment-based matching
    let root = Path::new(&claims.root);
    let url_path = Path::new(connection_path);

    // URL path must start with root (segment-based, not string starts_with)
    let suffix = url_path.strip_prefix(root).ok_or_else(|| {
        AuthError::Moq(format!(
            "Connection path '{}' does not match token root '{}'",
            connection_path, claims.root
        ))
    })?;

    // Reduce subscribe permissions based on connection depth
    let subscribe = claims.subscribe.iter().filter_map(|p| reduce_permission(p, &suffix)).collect();

    // Reduce publish permissions the same way
    let publish = claims.publish.iter().filter_map(|p| reduce_permission(p, &suffix)).collect();

    Ok(MoqAuthContext { root: url_path.to_owned(), subscribe, publish })
}

/// Reduce a permission path based on connection suffix.
///
/// If the permission is empty (root = allow all), it stays allowed.
/// Otherwise, strip the suffix from the permission and return the remainder.
fn reduce_permission(permission: &str, suffix: &Path) -> Option<PathOwned> {
    let p = Path::new(permission);

    if p.is_empty() {
        // [""] = root allowed, stays allowed at any depth
        Some(p.to_owned())
    } else if suffix.is_empty() {
        // No suffix = keep permission as-is
        Some(p.to_owned())
    } else {
        // Only keep if suffix is a prefix of the permission path
        p.strip_prefix(suffix).map(|reduced| reduced.to_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn make_claims(root: &str, subscribe: Vec<&str>, publish: Vec<&str>) -> MoqClaims {
        MoqClaims {
            aud: "skit-moq".to_string(),
            root: root.to_string(),
            subscribe: subscribe.into_iter().map(String::from).collect(),
            publish: publish.into_iter().map(String::from).collect(),
            iat: 0,
            exp: u64::MAX,
            jti: "test".to_string(),
        }
    }

    #[test]
    fn test_exact_root_match() {
        let claims = make_claims("/moq", vec![""], vec![""]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        assert!(ctx.can_subscribe("anything"));
        assert!(ctx.can_publish("anything"));
    }

    #[test]
    fn test_root_prefix_match() {
        let claims = make_claims("/moq", vec!["session1/output"], vec!["session1/input"]);
        let ctx = verify_moq_token(&claims, "/moq/session1").unwrap();

        // Permissions reduced by stripping "session1" prefix
        assert!(ctx.can_subscribe("output"));
        assert!(ctx.can_publish("input"));
        assert!(!ctx.can_subscribe("other"));
        assert!(!ctx.can_publish("other"));
    }

    #[test]
    fn test_root_mismatch_segment() {
        // Token root "/moq" should NOT match "/moq2" (different segment)
        let claims = make_claims("/moq", vec![""], vec![""]);
        let result = verify_moq_token(&claims, "/moq2");
        assert!(result.is_err());
    }

    #[test]
    fn test_root_mismatch_completely_different() {
        let claims = make_claims("/moq/session1", vec![""], vec![""]);
        let result = verify_moq_token(&claims, "/other/path");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_permissions_deny_all() {
        let claims = make_claims("/moq", vec![], vec![]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        assert!(!ctx.can_subscribe("anything"));
        assert!(!ctx.can_publish("anything"));
    }

    #[test]
    fn test_subscribe_only() {
        let claims = make_claims("/moq", vec![""], vec![]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        assert!(ctx.can_subscribe("anything"));
        assert!(!ctx.can_publish("anything"));
    }

    #[test]
    fn test_publish_only() {
        let claims = make_claims("/moq", vec![], vec![""]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        assert!(!ctx.can_subscribe("anything"));
        assert!(ctx.can_publish("anything"));
    }

    #[test]
    fn test_deep_path_reduction() {
        // Token allows publishing to "a/b/c/input" under root "/moq"
        let claims = make_claims("/moq", vec![], vec!["a/b/c/input"]);

        // Connect to "/moq/a/b"
        let ctx = verify_moq_token(&claims, "/moq/a/b").unwrap();

        // Permission should be reduced to "c/input"
        assert!(ctx.can_publish("c/input"));
        assert!(ctx.can_publish("c/input/more")); // prefix match
        assert!(!ctx.can_publish("input")); // doesn't match reduced permission
    }

    #[test]
    fn test_multiple_permissions() {
        let claims = make_claims("/moq", vec!["output1", "output2"], vec!["input"]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        assert!(ctx.can_subscribe("output1"));
        assert!(ctx.can_subscribe("output2"));
        assert!(!ctx.can_subscribe("output3"));
        assert!(ctx.can_publish("input"));
    }

    #[test]
    fn test_broadcast_prefix_matching() {
        let claims = make_claims("/moq", vec!["audio"], vec![]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        // "audio" prefix allows "audio", "audio/left", "audio/right", etc.
        assert!(ctx.can_subscribe("audio"));
        assert!(ctx.can_subscribe("audio/left"));
        assert!(ctx.can_subscribe("audio/stereo/left"));
        assert!(!ctx.can_subscribe("video"));
        // Note: "audiovisual" should NOT match because it's not a segment prefix
        // moq_lite uses segment-based matching, not string prefix
        assert!(!ctx.can_subscribe("audiovisual"));
    }

    #[test]
    fn test_moq_auth_context_debug() {
        let claims = make_claims("/moq", vec![""], vec![""]);
        let ctx = verify_moq_token(&claims, "/moq").unwrap();

        // Just verify Debug is implemented
        let debug_str = format!("{ctx:?}");
        assert!(debug_str.contains("MoqAuthContext"));
    }
}
