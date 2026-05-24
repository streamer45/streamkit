// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! JWT claim structures for StreamKit authentication.
//!
//! StreamKit uses two types of JWTs:
//! - **API tokens** (`aud: "skit-api"`): For HTTP API and WebSocket control plane
//! - **MoQ tokens** (`aud: "skit-moq"`): For MoQ/WebTransport connections
//!
//! Both token types require `jti` (JWT ID) for revocation support.

use serde::{Deserialize, Serialize};

/// Audience value for API tokens.
pub const AUD_API: &str = "skit-api";

/// Audience value for MoQ tokens.
#[allow(dead_code)]
pub const AUD_MOQ: &str = "skit-moq";

/// JWT claims for API tokens (HTTP API and WebSocket control plane).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClaims {
    /// Must be [`AUD_API`].
    pub aud: String,
    pub sub: String,
    /// E.g. `"admin"`, `"user"`, `"viewer"`.
    pub role: String,
    /// Unix timestamp (seconds).
    pub iat: u64,
    /// Unix timestamp (seconds). Required.
    pub exp: u64,
    /// Required for revocation.
    pub jti: String,
}

impl ApiClaims {
    /// Create anonymous claims for when auth is disabled.
    pub fn anonymous(role: &str) -> Self {
        Self {
            aud: AUD_API.to_string(),
            sub: "anonymous".to_string(),
            role: role.to_string(),
            iat: 0,
            exp: u64::MAX,
            jti: "anonymous".to_string(),
        }
    }

    /// Validate claims structure (not cryptographic verification).
    ///
    /// # Errors
    ///
    /// Returns [`ClaimsValidationError`] on bad audience, missing `jti`, or missing role.
    pub fn validate(&self) -> Result<(), ClaimsValidationError> {
        if self.aud != AUD_API {
            return Err(ClaimsValidationError::InvalidAudience {
                expected: AUD_API.to_string(),
                actual: self.aud.clone(),
            });
        }
        if self.jti.is_empty() {
            return Err(ClaimsValidationError::MissingJti);
        }
        if self.role.is_empty() {
            return Err(ClaimsValidationError::MissingRole);
        }
        Ok(())
    }
}

/// JWT claims for MoQ tokens (WebTransport connections).
///
/// Compatible with moq-token format. The `subscribe` and `publish` fields
/// are **broadcast path prefixes**, not URL paths.
///
/// Path semantics:
/// - `[""]` (empty string in array) = all broadcasts allowed
/// - `[]` (empty array) = no broadcasts allowed
/// - `["foo", "bar"]` = broadcasts starting with "foo" or "bar" allowed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct MoqClaims {
    /// Must be [`AUD_MOQ`].
    pub aud: String,
    /// URL path prefix this token is valid for (e.g. `"/moq/session1"`).
    pub root: String,

    /// Broadcast prefixes for subscribing (JWT claim: `get`).
    /// `[""]` = all, `[]` = none.
    #[serde(default, rename = "get", alias = "subscribe")]
    pub subscribe: Vec<String>,

    /// Broadcast prefixes for publishing (JWT claim: `put`).
    /// `[""]` = all, `[]` = none.
    #[serde(default, rename = "put", alias = "publish")]
    pub publish: Vec<String>,

    /// Unix timestamp (seconds).
    pub iat: u64,
    /// Unix timestamp (seconds). Required.
    pub exp: u64,
    /// Required for revocation.
    pub jti: String,
}

#[allow(dead_code)]
impl MoqClaims {
    /// Validate claims structure (not cryptographic verification).
    ///
    /// # Errors
    ///
    /// Returns [`ClaimsValidationError`] on bad audience, missing `jti`, or missing root.
    pub fn validate(&self) -> Result<(), ClaimsValidationError> {
        if self.aud != AUD_MOQ {
            return Err(ClaimsValidationError::InvalidAudience {
                expected: AUD_MOQ.to_string(),
                actual: self.aud.clone(),
            });
        }
        if self.jti.is_empty() {
            return Err(ClaimsValidationError::MissingJti);
        }
        if self.root.is_empty() {
            return Err(ClaimsValidationError::MissingRoot);
        }
        Ok(())
    }
}

/// Errors that can occur during claims validation.
#[derive(Debug, thiserror::Error)]
pub enum ClaimsValidationError {
    #[error("Invalid audience: expected {expected}, got {actual}")]
    InvalidAudience { expected: String, actual: String },

    #[error("Missing jti claim (required for revocation)")]
    MissingJti,

    #[error("Missing role claim")]
    MissingRole,

    #[error("Missing root claim")]
    #[allow(dead_code)]
    MissingRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_claims_validation() {
        let valid = ApiClaims {
            aud: AUD_API.to_string(),
            sub: "user123".to_string(),
            role: "admin".to_string(),
            iat: 1000,
            exp: 2000,
            jti: "abc-123".to_string(),
        };
        assert!(valid.validate().is_ok());

        // Wrong audience
        let wrong_aud = ApiClaims { aud: AUD_MOQ.to_string(), ..valid.clone() };
        assert!(matches!(wrong_aud.validate(), Err(ClaimsValidationError::InvalidAudience { .. })));

        // Missing jti
        let no_jti = ApiClaims { jti: String::new(), ..valid.clone() };
        assert!(matches!(no_jti.validate(), Err(ClaimsValidationError::MissingJti)));

        // Missing role
        let no_role = ApiClaims { role: String::new(), ..valid };
        assert!(matches!(no_role.validate(), Err(ClaimsValidationError::MissingRole)));
    }

    #[test]
    fn test_moq_claims_validation() {
        let valid = MoqClaims {
            aud: AUD_MOQ.to_string(),
            root: "/moq/session1".to_string(),
            subscribe: vec![String::new()], // Allow all
            publish: vec![String::new()],   // Allow all
            iat: 1000,
            exp: 2000,
            jti: "moq-123".to_string(),
        };
        assert!(valid.validate().is_ok());

        // Wrong audience
        let wrong_aud = MoqClaims { aud: AUD_API.to_string(), ..valid.clone() };
        assert!(matches!(wrong_aud.validate(), Err(ClaimsValidationError::InvalidAudience { .. })));

        // Missing root
        let no_root = MoqClaims { root: String::new(), ..valid };
        assert!(matches!(no_root.validate(), Err(ClaimsValidationError::MissingRoot)));
    }

    #[test]
    fn test_anonymous_claims() {
        let anon = ApiClaims::anonymous("viewer");
        assert_eq!(anon.aud, AUD_API);
        assert_eq!(anon.sub, "anonymous");
        assert_eq!(anon.role, "viewer");
        assert_eq!(anon.jti, "anonymous");
    }
}
