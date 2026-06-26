// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Built-in JWT authentication for StreamKit.
//!
//! This module provides:
//! - JWT-based authentication for HTTP API, WebSocket, and MoQ/WebTransport
//! - Pluggable storage backends (file-based by default)
//! - Token revocation support
//! - Cookie-based browser sessions
//!
//! # Token Types
//!
//! - **API tokens** (`aud: "skit-api"`): For HTTP API and WebSocket control plane
//! - **MoQ tokens** (`aud: "skit-moq"`): For MoQ/WebTransport connections
//!
//! # Security Model
//!
//! - All tokens require `jti` claim for revocation support
//! - "Tokens we mint" policy: Only accept tokens whose jti is in our metadata store
//! - Raw tokens are never stored; only SHA-256 hashes are persisted
//! - Key material is stored with 0600 permissions

pub mod claims;
pub mod cookie;
pub mod extractor;
pub mod handlers;
pub mod stores;

#[cfg(feature = "moq")]
pub mod moq;
#[cfg(feature = "moq")]
#[allow(unused_imports)]
pub use moq::{verify_moq_token, MoqAuthContext};

pub use claims::{ApiClaims, AUD_API};
#[cfg(feature = "moq")]
pub use claims::{MoqClaims, AUD_MOQ};
pub use cookie::{build_logout_cookie, build_session_cookie};
pub use extractor::{validate_token, validate_token_from_headers, AuthContext};
pub use handlers::auth_router;
pub use stores::{
    AuthStoreError, FileKeyProvider, FileRevocationStore, FileTokenMetadataStore, KeyProvider,
    RevocationStore, SigningKeyMaterial, TokenMetadata, TokenMetadataStore, TokenType,
};

use crate::config::{AuthConfig, AuthMode};
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};
use tracing::{debug, info};

/// Errors that can occur during authentication.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AuthError {
    #[error("Authentication is disabled")]
    Disabled,

    #[error("Store error: {0}")]
    Store(#[from] AuthStoreError),

    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error("Token is missing the `kid` header")]
    MissingKid,

    #[error("Claims validation error: {0}")]
    Claims(#[from] claims::ClaimsValidationError),

    #[error("Token not found in metadata store (not minted by this server)")]
    UnknownToken,

    #[error("Token has been revoked")]
    Revoked,

    #[error("Token expired")]
    Expired,

    #[error("Invalid audience: expected {expected}, got {actual}")]
    InvalidAudience { expected: String, actual: String },

    #[error("TTL exceeds maximum allowed ({max} seconds)")]
    TtlExceedsMax { max: u64 },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("System time error: {0}")]
    Time(#[from] SystemTimeError),

    #[error("Task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[cfg(feature = "moq")]
    #[error("MoQ auth error: {0}")]
    Moq(String),
}

/// Resolve the verification key from the token's `kid` header and decode it.
///
/// Every token we mint stamps a `kid`, so the key is resolved by a direct
/// lookup. A kid-less token was not minted here; reject it instead of probing
/// the whole (post-rotation) key set. Returns the decoded claims and the `kid`.
fn decode_with_kid<T: DeserializeOwned>(
    key_provider: &dyn KeyProvider,
    token: &str,
    validation: &Validation,
) -> Result<(T, String), AuthError> {
    let kid = decode_header(token)?.kid.ok_or(AuthError::MissingKid)?;
    let key_material = key_provider
        .verification_key(&kid)
        .ok_or_else(|| AuthError::Jwt(jsonwebtoken::errors::ErrorKind::InvalidSignature.into()))?;
    let decoding_key = DecodingKey::from_ed_der(&key_material.public_key);
    let claims = decode::<T>(token, &decoding_key, validation)?.claims;
    Ok((claims, kid))
}

/// Central authentication state (key material, revocation, token metadata).
pub struct AuthState {
    enabled: bool,
    config: AuthConfig,
    key_provider: Option<Arc<dyn KeyProvider>>,
    revocation_store: Option<Arc<dyn RevocationStore>>,
    token_metadata_store: Option<Arc<dyn TokenMetadataStore>>,
}

impl AuthState {
    /// Create a disabled `AuthState` (synchronous).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            config: AuthConfig::default(),
            key_provider: None,
            revocation_store: None,
            token_metadata_store: None,
        }
    }

    /// Create a new AuthState, initializing stores if auth is enabled.
    ///
    /// # Errors
    ///
    /// Returns errors for store initialization failures, I/O errors, or
    /// bootstrap token creation failures.
    pub async fn new(config: &AuthConfig, enabled: bool) -> Result<Self, AuthError> {
        if !enabled {
            info!("Authentication disabled");
            return Ok(Self {
                enabled: false,
                config: config.clone(),
                key_provider: None,
                revocation_store: None,
                token_metadata_store: None,
            });
        }

        let state_dir = PathBuf::from(&config.state_dir);
        info!(state_dir = %state_dir.display(), "Initializing authentication");

        let key_provider = Arc::new(FileKeyProvider::load_or_init(&state_dir).await?);
        let revocation_store = Arc::new(FileRevocationStore::new(&state_dir).await?);
        let token_metadata_store = Arc::new(FileTokenMetadataStore::new(&state_dir).await?);

        let tokens = token_metadata_store.list().await?;
        if tokens.is_empty() {
            info!("No tokens found, creating bootstrap admin token");
            let state = Self {
                enabled: true,
                config: config.clone(),
                key_provider: Some(key_provider.clone()),
                revocation_store: Some(revocation_store.clone()),
                token_metadata_store: Some(token_metadata_store.clone()),
            };

            let (token, _meta) = state
                .mint_api_token(
                    "admin",
                    Some("Bootstrap admin token"),
                    config.api_max_ttl_secs,
                    "bootstrap",
                )
                .await?;

            let token_path = state_dir.join("admin.token");
            FileKeyProvider::write_secure(&token_path, &token).await?;

            info!(path = %token_path.display(), "Bootstrap admin token written");

            return Ok(state);
        }

        Ok(Self {
            enabled: true,
            config: config.clone(),
            key_provider: Some(key_provider),
            revocation_store: Some(revocation_store),
            token_metadata_store: Some(token_metadata_store),
        })
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn revocation_store(&self) -> Option<&Arc<dyn RevocationStore>> {
        self.revocation_store.as_ref()
    }

    /// Returns false if auth is disabled or the revocation store is unavailable.
    #[allow(dead_code)]
    pub fn is_revoked(&self, token_hash: &str) -> bool {
        self.revocation_store.as_ref().is_some_and(|store| store.is_revoked(token_hash))
    }

    pub fn token_metadata_store(&self) -> Option<&Arc<dyn TokenMetadataStore>> {
        self.token_metadata_store.as_ref()
    }

    #[allow(dead_code)]
    pub fn key_provider(&self) -> Option<&Arc<dyn KeyProvider>> {
        self.key_provider.as_ref()
    }

    /// Verify JWT signature, expiration, audience, and claims structure.
    ///
    /// Revocation and "tokens we mint" checks are the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError`] if auth is disabled, the token is malformed,
    /// signature verification fails, or claims validation fails.
    pub fn validate_api_token(&self, token: &str) -> Result<ApiClaims, AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_audience(&[AUD_API]);
        validation.set_required_spec_claims(&["exp", "aud", "jti"]);

        let (claims, kid) =
            decode_with_kid::<ApiClaims>(key_provider.as_ref(), token, &validation)?;
        claims.validate()?;
        debug!(jti = %claims.jti, role = %claims.role, kid = %kid, "API token validated");
        Ok(claims)
    }

    /// Validate a MoQ token and return its claims.
    ///
    /// # Errors
    ///
    /// Returns errors for invalid tokens, expired tokens, signature verification
    /// failures, or disabled auth.
    #[cfg(feature = "moq")]
    pub fn validate_moq_token(&self, token: &str) -> Result<MoqClaims, AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_audience(&[AUD_MOQ]);
        validation.set_required_spec_claims(&["exp", "aud", "jti"]);

        let (claims, kid) =
            decode_with_kid::<MoqClaims>(key_provider.as_ref(), token, &validation)?;
        claims.validate()?;
        debug!(jti = %claims.jti, root = %claims.root, kid = %kid, "MoQ token validated");
        Ok(claims)
    }

    /// Mint a new API token.
    ///
    /// Returns the raw token string and its metadata.
    ///
    /// # Errors
    ///
    /// Returns errors if auth is disabled, TTL exceeds max, or token storage fails.
    pub async fn mint_api_token(
        &self,
        role: &str,
        label: Option<&str>,
        ttl_secs: u64,
        created_by: &str,
    ) -> Result<(String, TokenMetadata), AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;
        let metadata_store = self.token_metadata_store.as_ref().ok_or(AuthError::Disabled)?;

        if ttl_secs > self.config.api_max_ttl_secs {
            return Err(AuthError::TtlExceedsMax { max: self.config.api_max_ttl_secs });
        }

        let now = now_secs()?;

        let jti = uuid::Uuid::new_v4().to_string();
        let exp = now + ttl_secs;

        let claims = ApiClaims {
            aud: AUD_API.to_string(),
            sub: format!("token:{jti}"),
            role: role.to_string(),
            iat: now,
            exp,
            jti: jti.clone(),
        };

        // Sign the token (key access uses std::sync locks, so keep it off core async tasks)
        let key_provider_clone = key_provider.clone();
        let key_material =
            tokio::task::spawn_blocking(move || key_provider_clone.active_key()).await?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(key_material.kid.clone());

        let encoding_key = EncodingKey::from_ed_der(&key_material.pkcs8);
        let token = encode(&header, &claims, &encoding_key)?;

        let token_hash = hash_token(&token);
        let meta = TokenMetadata {
            jti: jti.clone(),
            token_hash,
            token_type: TokenType::Api,
            role: Some(role.to_string()),
            label: label.map(String::from),
            created_at: now,
            exp,
            revoked: false,
            created_by: created_by.to_string(),
        };

        metadata_store.store(meta.clone()).await?;

        info!(jti = %jti, role = %role, ttl_secs, "Minted API token");

        Ok((token, meta))
    }

    /// Mint a new MoQ token.
    ///
    /// # Errors
    ///
    /// Returns errors if auth is disabled, TTL exceeds max, or token storage fails.
    #[cfg(feature = "moq")]
    pub async fn mint_moq_token(
        &self,
        root: &str,
        subscribe: Vec<String>,
        publish: Vec<String>,
        label: Option<&str>,
        ttl_secs: u64,
        created_by: &str,
    ) -> Result<(String, TokenMetadata), AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;
        let metadata_store = self.token_metadata_store.as_ref().ok_or(AuthError::Disabled)?;

        if ttl_secs > self.config.moq_max_ttl_secs {
            return Err(AuthError::TtlExceedsMax { max: self.config.moq_max_ttl_secs });
        }

        let now = now_secs()?;

        let jti = uuid::Uuid::new_v4().to_string();
        let exp = now + ttl_secs;

        let claims = MoqClaims {
            aud: AUD_MOQ.to_string(),
            root: root.to_string(),
            subscribe,
            publish,
            iat: now,
            exp,
            jti: jti.clone(),
        };

        // Sign the token (key access uses std::sync locks, so keep it off core async tasks)
        let key_provider_clone = key_provider.clone();
        let key_material =
            tokio::task::spawn_blocking(move || key_provider_clone.active_key()).await?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(key_material.kid.clone());

        let encoding_key = EncodingKey::from_ed_der(&key_material.pkcs8);
        let token = encode(&header, &claims, &encoding_key)?;

        let token_hash = hash_token(&token);
        let meta = TokenMetadata {
            jti: jti.clone(),
            token_hash,
            token_type: TokenType::Moq,
            role: None,
            label: label.map(String::from),
            created_at: now,
            exp,
            revoked: false,
            created_by: created_by.to_string(),
        };

        metadata_store.store(meta.clone()).await?;

        info!(jti = %jti, root = %root, ttl_secs, "Minted MoQ token");

        Ok((token, meta))
    }

    /// Revoke a token by its jti.
    ///
    /// # Errors
    ///
    /// Returns errors if auth is disabled, token not found, or store operation fails.
    pub async fn revoke_token(&self, jti: &str) -> Result<(), AuthError> {
        let revocation_store = self.revocation_store.as_ref().ok_or(AuthError::Disabled)?;
        let metadata_store = self.token_metadata_store.as_ref().ok_or(AuthError::Disabled)?;

        let meta = metadata_store.get(jti).await?;
        let meta = meta.ok_or(AuthError::UnknownToken)?;
        let token_hash = meta.token_hash;
        let exp = meta.exp;

        revocation_store.revoke(&token_hash, exp).await?;
        metadata_store.mark_revoked(jti).await?;

        info!(jti = %jti, "Token revoked");

        Ok(())
    }

    /// Rotate the signing key.
    ///
    /// # Errors
    ///
    /// Returns errors if auth is disabled or key provider rotation fails.
    pub async fn rotate_key(&self) -> Result<SigningKeyMaterial, AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;
        let new_key = key_provider.rotate().await?;
        info!(kid = %new_key.kid, "Signing key rotated");
        Ok(new_key)
    }

    /// Reload all auth state from persistent storage.
    ///
    /// Re-reads keys, token metadata, and revocations from the backing
    /// store, replacing every in-memory cache.  Call this after an
    /// external mutation (e.g. the CLI `rotate-key` command) to pick up
    /// the new state without restarting the server.
    ///
    /// # Errors
    ///
    /// Returns errors if auth is disabled or any reload step fails.
    pub async fn reload_keys(&self) -> Result<(), AuthError> {
        let key_provider = self.key_provider.as_ref().ok_or(AuthError::Disabled)?;

        // Reload metadata and revocations before keys.  Neither order
        // is fully atomic: metadata-first means a new-kid token arriving
        // mid-reload fails signature verification (transient, retryable);
        // keys-first means it passes signature but fails the JTI "minted
        // by us" check (looks like a permanent auth error).  We choose
        // metadata-first as the lesser evil.
        if let Some(token_meta) = self.token_metadata_store.as_ref() {
            token_meta.reload().await?;
        }
        if let Some(revocation) = self.revocation_store.as_ref() {
            revocation.reload().await?;
        }

        key_provider.reload().await?;

        let active_kid = key_provider.active_key().kid;
        info!(kid = %active_kid, "Auth state reloaded from disk (keys, token metadata, revocations)");
        Ok(())
    }

    /// Whether auth should be enabled based on config and bind address.
    #[allow(dead_code)]
    pub const fn should_enable(config: &AuthConfig, bind_addr: &std::net::SocketAddr) -> bool {
        match config.mode {
            AuthMode::Auto => !bind_addr.ip().is_loopback(),
            AuthMode::Enabled => true,
            AuthMode::Disabled => false,
        }
    }
}

/// Compute SHA-256 hash of a token (hex-encoded).
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Get current Unix timestamp in seconds.
///
/// # Errors
///
/// Returns an error if the system clock is before Unix epoch.
pub fn now_secs() -> Result<u64, AuthError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test fixtures use unwrap to fail loudly on setup errors
pub mod test_support {
    use super::*;
    use tempfile::TempDir;

    /// Builds an enabled [`AuthState`] backed by a temporary state dir for tests.
    ///
    /// # Panics
    ///
    /// Panics if the temporary directory or auth state cannot be created.
    pub async fn create_test_auth_state() -> (AuthState, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = AuthConfig {
            mode: AuthMode::Enabled,
            state_dir: temp_dir.path().to_string_lossy().to_string(),
            cookie_name: "test_session".to_string(),
            api_default_ttl_secs: 3600,
            api_max_ttl_secs: 86400,
            moq_default_ttl_secs: 3600,
            moq_max_ttl_secs: 86400,
            moq_public_paths: Vec::new(),
        };

        let state = AuthState::new(&config, true).await.unwrap();
        (state, temp_dir)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test fixtures use unwrap to fail loudly on setup errors
mod tests {
    use super::test_support::create_test_auth_state;
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_auth_state_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let config = AuthConfig {
            mode: AuthMode::Disabled,
            state_dir: temp_dir.path().to_string_lossy().to_string(),
            ..Default::default()
        };

        let state = AuthState::new(&config, false).await.unwrap();
        assert!(!state.is_enabled());
        assert!(state.key_provider().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mint_and_validate_api_token() {
        let (state, _temp_dir) = create_test_auth_state().await;
        let state = Arc::new(state);

        // Mint a token
        let (token, meta) =
            state.mint_api_token("admin", Some("Test token"), 3600, "test").await.unwrap();

        assert!(!token.is_empty());
        assert_eq!(meta.role, Some("admin".to_string()));
        assert!(!meta.revoked);

        // Validate the token (uses blocking_read internally)
        let state_clone = state.clone();
        let token_clone = token.clone();
        let claims =
            tokio::task::spawn_blocking(move || state_clone.validate_api_token(&token_clone))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(claims.jti, meta.jti);
        assert_eq!(claims.role, "admin");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn validate_api_token_rejects_token_without_kid() {
        let (state, _temp_dir) = create_test_auth_state().await;
        let state = Arc::new(state);

        let result = tokio::task::spawn_blocking(move || {
            let active = state.key_provider().unwrap().active_key();
            let now = now_secs().unwrap();
            let claims = ApiClaims {
                aud: AUD_API.to_string(),
                sub: "token:test".to_string(),
                role: "admin".to_string(),
                iat: now,
                exp: now + 3600,
                jti: uuid::Uuid::new_v4().to_string(),
            };
            let token = encode(
                &Header::new(Algorithm::EdDSA),
                &claims,
                &EncodingKey::from_ed_der(&active.pkcs8),
            )
            .unwrap();
            state.validate_api_token(&token)
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(AuthError::MissingKid)));
    }

    #[cfg(feature = "moq")]
    #[tokio::test(flavor = "multi_thread")]
    async fn validate_moq_token_rejects_token_without_kid() {
        let (state, _temp_dir) = create_test_auth_state().await;
        let state = Arc::new(state);

        let result = tokio::task::spawn_blocking(move || {
            let active = state.key_provider().unwrap().active_key();
            let now = now_secs().unwrap();
            let claims = MoqClaims {
                aud: AUD_MOQ.to_string(),
                root: "/moq/test".to_string(),
                subscribe: vec![String::new()],
                publish: vec![],
                iat: now,
                exp: now + 3600,
                jti: uuid::Uuid::new_v4().to_string(),
            };
            let token = encode(
                &Header::new(Algorithm::EdDSA),
                &claims,
                &EncodingKey::from_ed_der(&active.pkcs8),
            )
            .unwrap();
            state.validate_moq_token(&token)
        })
        .await
        .unwrap();

        assert!(matches!(result, Err(AuthError::MissingKid)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_token_revocation() {
        let (state, _temp_dir) = create_test_auth_state().await;
        let state = Arc::new(state);

        // Mint and validate
        let (token, meta) = state.mint_api_token("user", None, 3600, "test").await.unwrap();

        let state_clone = state.clone();
        let token_clone = token.clone();
        let claims =
            tokio::task::spawn_blocking(move || state_clone.validate_api_token(&token_clone))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(claims.jti, meta.jti);

        let revocation_store = state.revocation_store().unwrap().clone();
        let hash_clone = hash_token(&token);
        let is_revoked =
            tokio::task::spawn_blocking(move || revocation_store.is_revoked(&hash_clone))
                .await
                .unwrap();
        assert!(!is_revoked);

        // Revoke
        state.revoke_token(&meta.jti).await.unwrap();

        let revocation_store = state.revocation_store().unwrap().clone();
        let hash_clone = hash_token(&token);
        let is_revoked =
            tokio::task::spawn_blocking(move || revocation_store.is_revoked(&hash_clone))
                .await
                .unwrap();
        assert!(is_revoked);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_ttl_max_enforcement() {
        let (state, _temp_dir) = create_test_auth_state().await;

        // Try to mint with TTL exceeding max
        let result = state.mint_api_token("admin", None, 1_000_000, "test").await;

        assert!(matches!(result, Err(AuthError::TtlExceedsMax { .. })));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_bootstrap_token_created() {
        let temp_dir = TempDir::new().unwrap();
        let config = AuthConfig {
            mode: AuthMode::Enabled,
            state_dir: temp_dir.path().to_string_lossy().to_string(),
            api_max_ttl_secs: 86400,
            ..Default::default()
        };

        // First initialization should create bootstrap token
        let _state = AuthState::new(&config, true).await.unwrap();

        // Check bootstrap token file exists
        let token_path = temp_dir.path().join("admin.token");
        assert!(token_path.exists());

        let token = tokio::fs::read_to_string(&token_path).await.unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_hash_token() {
        let hash1 = hash_token("test-token-1");
        let hash2 = hash_token("test-token-1");
        let hash3 = hash_token("test-token-2");

        // Same input = same hash
        assert_eq!(hash1, hash2);
        // Different input = different hash
        assert_ne!(hash1, hash3);
        // Hash is hex-encoded SHA-256 (64 chars)
        assert_eq!(hash1.len(), 64);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_reload_keys_picks_up_external_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let config = AuthConfig {
            mode: AuthMode::Enabled,
            state_dir: temp_dir.path().to_string_lossy().to_string(),
            api_max_ttl_secs: 86400,
            ..Default::default()
        };

        // "Server" auth state — stays alive the whole time.
        let server = Arc::new(AuthState::new(&config, true).await.unwrap());

        // "CLI" auth state — separate instance sharing the same state dir.
        let cli = AuthState::new(&config, true).await.unwrap();

        // CLI rotates the key and mints a token with the new key.
        let new_key = cli.rotate_key().await.unwrap();
        let (new_token, meta) =
            cli.mint_api_token("admin", Some("post-rotate"), 3600, "test").await.unwrap();

        // Before reload: server cannot validate the new token.
        let server_clone = server.clone();
        let token_clone = new_token.clone();
        let result =
            tokio::task::spawn_blocking(move || server_clone.validate_api_token(&token_clone))
                .await
                .unwrap();
        assert!(result.is_err(), "server should reject token signed with unknown key");

        // Before reload: server's token metadata store does not know the
        // CLI-minted JTI.
        let meta_store = server.token_metadata_store().unwrap();
        assert!(
            !meta_store.exists(&meta.jti).await,
            "server should not know CLI-minted JTI before reload"
        );

        // Reload keys on the server side.
        server.reload_keys().await.unwrap();

        // After reload: server accepts the new token (JWT signature).
        let server_clone = server.clone();
        let token_clone = new_token.clone();
        let claims =
            tokio::task::spawn_blocking(move || server_clone.validate_api_token(&token_clone))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(claims.role, "admin");

        // After reload: server's token metadata store recognises the
        // CLI-minted JTI (the "tokens we mint" enforcement path).
        assert!(
            meta_store.exists(&meta.jti).await,
            "server should recognise CLI-minted JTI after reload"
        );

        // The server's key provider should report the new active kid.
        let active_kid = {
            let kp = server.key_provider().unwrap().clone();
            tokio::task::spawn_blocking(move || kp.active_key().kid).await.unwrap()
        };
        assert_eq!(active_kid, new_key.kid);
    }

    #[test]
    fn test_should_enable() {
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4545);
        let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4545);

        // Auto mode
        let auto_config = AuthConfig { mode: AuthMode::Auto, ..Default::default() };
        assert!(!AuthState::should_enable(&auto_config, &loopback));
        assert!(AuthState::should_enable(&auto_config, &any));

        // Enabled mode
        let enabled_config = AuthConfig { mode: AuthMode::Enabled, ..Default::default() };
        assert!(AuthState::should_enable(&enabled_config, &loopback));
        assert!(AuthState::should_enable(&enabled_config, &any));

        // Disabled mode
        let disabled_config = AuthConfig { mode: AuthMode::Disabled, ..Default::default() };
        assert!(!AuthState::should_enable(&disabled_config, &loopback));
        assert!(!AuthState::should_enable(&disabled_config, &any));
    }
}
