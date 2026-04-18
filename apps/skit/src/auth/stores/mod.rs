// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Auth store traits for pluggable storage backends.
//!
//! This module defines the traits for key management, token revocation,
//! and token metadata storage. The default implementation uses the filesystem,
//! but these traits allow for alternative backends (e.g., Redis) in the future.

mod file;

pub use file::{FileKeyProvider, FileRevocationStore, FileTokenMetadataStore};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Active signing key material for JWT signing.
#[derive(Clone)]
pub struct SigningKeyMaterial {
    /// Key identifier (for JWT `kid` header)
    pub kid: String,
    /// Ed25519 private key in PKCS#8 DER format (used for EdDSA signing).
    pub pkcs8: Arc<[u8]>,
}

/// Verification key material for JWT validation.
#[derive(Clone)]
pub struct VerificationKeyMaterial {
    /// Ed25519 public key bytes (32 bytes, raw).
    pub public_key: Arc<[u8]>,
}

/// Errors that can occur in auth stores.
#[derive(Debug, thiserror::Error)]
pub enum AuthStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Key not found: {0}")]
    #[allow(dead_code)]
    KeyNotFound(String),

    #[error("Invalid file permissions on {path}: expected 0600, got {actual:o}")]
    InsecurePermissions { path: String, actual: u32 },

    #[error("Invalid key data: {0}")]
    InvalidKey(String),
}

/// Public JWKS (JSON Web Key Set) served for verifier-only clients.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// Public JWK for Ed25519 verification.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    #[serde(rename = "use")]
    pub public_key_use: String,
    pub alg: String,
    pub kid: String,
    pub x: String,
}

/// Provides signing keys for JWT operations.
///
/// Implementations must be thread-safe. Key rotation is supported by
/// maintaining multiple keys: one active key for signing new tokens,
/// and older keys for verifying existing tokens until they expire.
#[async_trait]
pub trait KeyProvider: Send + Sync {
    /// Get the active signing key.
    fn active_key(&self) -> SigningKeyMaterial;

    /// Get a verification key by kid.
    ///
    /// Returns `None` if the kid is not known (token might be from
    /// a different server or the key has been removed).
    fn verification_key(&self, kid: &str) -> Option<VerificationKeyMaterial>;

    /// Get all valid key IDs (for JWT validation header checks).
    fn valid_kids(&self) -> Vec<String>;

    /// Get the public JWKS representing all verification keys.
    fn jwks(&self) -> Jwks;

    /// Rotate keys: generate a new active key, keep old keys for verification.
    ///
    /// Returns the new active key material.
    async fn rotate(&self) -> Result<SigningKeyMaterial, AuthStoreError>;

    /// Reload keys from the backing store.
    ///
    /// Re-reads the active private key and the full JWKS from persistent
    /// storage, replacing the in-memory cache.  This is used to pick up
    /// key rotations performed by an external process (e.g. the CLI
    /// `rotate-key` command) without restarting the server.
    async fn reload(&self) -> Result<(), AuthStoreError>;
}

/// Revocation store for invalidated tokens.
///
/// The `is_revoked` method MUST be fast (in-memory lookup) as it's called
/// on every authenticated request. Implementations should load revocations
/// into memory at startup and persist changes atomically.
#[async_trait]
pub trait RevocationStore: Send + Sync {
    /// Check if a token is revoked by its SHA-256 hash.
    ///
    /// This method must be fast (in-memory lookup) as it's called on every request.
    fn is_revoked(&self, token_hash: &str) -> bool;

    /// Revoke a token by its SHA-256 hash.
    ///
    /// The `exp` parameter is the token's expiration time, which can be used
    /// to automatically clean up expired revocations.
    async fn revoke(&self, token_hash: &str, exp: u64) -> Result<(), AuthStoreError>;

    /// Reload revocations from persistent storage.
    async fn reload(&self) -> Result<(), AuthStoreError>;
}

/// Token type distinguishes API tokens from MoQ tokens.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    Api,
    Moq,
}

impl std::fmt::Display for TokenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api => write!(f, "api"),
            Self::Moq => write!(f, "moq"),
        }
    }
}

/// Metadata about a minted token (stored, never contains raw token).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenMetadata {
    /// Unique token identifier (JWT `jti` claim)
    pub jti: String,
    /// SHA-256 hash of the token (hex-encoded)
    pub token_hash: String,
    /// Token type (API or MoQ)
    pub token_type: TokenType,
    /// Role name (for API tokens)
    pub role: Option<String>,
    /// Human-readable label
    pub label: Option<String>,
    /// Creation timestamp (Unix seconds)
    pub created_at: u64,
    /// Expiration timestamp (Unix seconds)
    pub exp: u64,
    /// Whether the token has been revoked
    pub revoked: bool,
    /// Identity that created this token (jti of the parent token, or "bootstrap")
    pub created_by: String,
}

/// Store for token metadata ("tokens we minted").
///
/// This store tracks all tokens minted by this server, enabling:
/// - Listing active tokens for admin UI
/// - Enforcing "tokens we mint" policy (reject unknown jtis)
/// - Tracking revocation status alongside other metadata
#[async_trait]
pub trait TokenMetadataStore: Send + Sync {
    /// Store metadata when minting a new token.
    async fn store(&self, meta: TokenMetadata) -> Result<(), AuthStoreError>;

    /// Check if a jti exists in our store.
    ///
    /// Used for "tokens we mint" enforcement: reject tokens whose
    /// jti is not in our store.
    async fn exists(&self, jti: &str) -> bool;

    /// List all tokens (for admin UI).
    async fn list(&self) -> Result<Vec<TokenMetadata>, AuthStoreError>;

    /// Mark a token as revoked in metadata.
    async fn mark_revoked(&self, jti: &str) -> Result<(), AuthStoreError>;

    /// Get metadata for a specific token.
    async fn get(&self, jti: &str) -> Result<Option<TokenMetadata>, AuthStoreError>;

    /// Reload token metadata from persistent storage, replacing the in-memory
    /// cache.  Call this after external mutations (e.g. CLI `rotate-key` minting
    /// a new admin token) so the running server recognises the new JTIs.
    async fn reload(&self) -> Result<(), AuthStoreError>;
}
