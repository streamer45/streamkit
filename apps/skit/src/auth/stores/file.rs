// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! File-based implementations of auth stores.

use super::{
    AuthStoreError, Jwk, Jwks, KeyProvider, RevocationStore, SigningKeyMaterial, TokenMetadata,
    TokenMetadataStore, VerificationKeyMaterial,
};
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

const PRIVATE_JWK_FILENAME: &str = "auth.jwk";
const PUBLIC_JWKS_FILENAME: &str = "jwks.json";

/// Private JWK persisted to disk (contains the Ed25519 seed in `d`).
#[derive(Clone, Serialize, Deserialize)]
struct PrivateJwk {
    kty: String,
    crv: String,
    #[serde(rename = "use")]
    public_key_use: String,
    alg: String,
    kid: String,
    x: String,
    d: String,
}

impl PrivateJwk {
    fn validate(&self) -> Result<(), AuthStoreError> {
        if self.kty != "OKP" {
            return Err(AuthStoreError::InvalidKey(format!("Unsupported kty: {}", self.kty)));
        }
        if self.crv != "Ed25519" {
            return Err(AuthStoreError::InvalidKey(format!("Unsupported crv: {}", self.crv)));
        }
        if self.alg != "EdDSA" {
            return Err(AuthStoreError::InvalidKey(format!("Unsupported alg: {}", self.alg)));
        }
        if self.public_key_use != "sig" {
            return Err(AuthStoreError::InvalidKey(format!(
                "Unsupported JWK use: {}",
                self.public_key_use
            )));
        }
        if self.kid.trim().is_empty() {
            return Err(AuthStoreError::InvalidKey("Missing kid".to_string()));
        }
        if self.x.trim().is_empty() || self.d.trim().is_empty() {
            return Err(AuthStoreError::InvalidKey("Missing key material (x/d)".to_string()));
        }
        Ok(())
    }

    fn to_public_jwk(&self) -> Jwk {
        Jwk {
            kty: self.kty.clone(),
            crv: self.crv.clone(),
            public_key_use: self.public_key_use.clone(),
            alg: self.alg.clone(),
            kid: self.kid.clone(),
            x: self.x.clone(),
        }
    }
}

fn lock_read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// File-based key provider with rotation support.
///
/// Stores private signing key in `auth.jwk` (0600) and public verification keys in `jwks.json`.
pub struct FileKeyProvider {
    state_dir: PathBuf,
    active: RwLock<SigningKeyMaterial>,
    /// kid -> raw Ed25519 public key bytes (32 bytes)
    public_keys: RwLock<HashMap<String, Arc<[u8]>>>,
    jwks: RwLock<Jwks>,
}

impl FileKeyProvider {
    /// Load existing keys or initialize with a new key.
    ///
    /// # Errors
    ///
    /// Returns errors for I/O failures, invalid permissions, or JSON parsing errors.
    ///
    /// # Panics
    ///
    /// Panics if the system random number generator fails (critical security failure).
    #[allow(clippy::expect_used)]
    pub async fn load_or_init(state_dir: &Path) -> Result<Self, AuthStoreError> {
        // Ensure state directory exists
        tokio::fs::create_dir_all(state_dir).await?;

        let private_path = state_dir.join(PRIVATE_JWK_FILENAME);
        let jwks_path = state_dir.join(PUBLIC_JWKS_FILENAME);

        let (private_jwk, active_signing_key, public_key_bytes) = if private_path.exists() {
            Self::verify_permissions(&private_path)?;
            let content = tokio::fs::read_to_string(&private_path).await?;
            let private: PrivateJwk = serde_json::from_str(&content)?;
            private.validate()?;

            let seed = base64url_decode(&private.d)?;
            let public_from_file = base64url_decode(&private.x)?;

            if seed.len() != 32 {
                return Err(AuthStoreError::InvalidKey(
                    "Ed25519 seed must be 32 bytes".to_string(),
                ));
            }
            if public_from_file.len() != 32 {
                return Err(AuthStoreError::InvalidKey(
                    "Ed25519 public key must be 32 bytes".to_string(),
                ));
            }

            let (derived_public, pkcs8) = derive_keypair(&seed)?;
            if derived_public != public_from_file {
                return Err(AuthStoreError::InvalidKey(
                    "Public key in JWK does not match derived key".to_string(),
                ));
            }

            let kid = private.kid.clone();
            (
                private,
                SigningKeyMaterial { kid, pkcs8: Arc::from(pkcs8.into_boxed_slice()) },
                Arc::from(public_from_file.into_boxed_slice()),
            )
        } else {
            let (private, signing_key, public_key) = generate_new_private_key()?;
            Self::write_secure(&private_path, &serde_json::to_string_pretty(&private)?).await?;
            info!(path = %private_path.display(), "Created new Ed25519 signing key");
            (private, signing_key, public_key)
        };

        let mut jwks = if jwks_path.exists() {
            let content = tokio::fs::read_to_string(&jwks_path).await?;
            serde_json::from_str::<Jwks>(&content)?
        } else {
            Jwks { keys: vec![] }
        };

        // Ensure active key is present in JWKS.
        if !jwks.keys.iter().any(|k| k.kid == private_jwk.kid) {
            jwks.keys.push(private_jwk.to_public_jwk());
            Self::write_secure(&jwks_path, &serde_json::to_string_pretty(&jwks)?).await?;
        }

        // Build public key map (kid -> raw bytes)
        let mut public_keys: HashMap<String, Arc<[u8]>> = HashMap::new();
        for jwk in &jwks.keys {
            let bytes = base64url_decode(&jwk.x)?;
            if bytes.len() != 32 {
                return Err(AuthStoreError::InvalidKey(format!(
                    "Invalid public key length for kid {}",
                    jwk.kid
                )));
            }
            public_keys.insert(jwk.kid.clone(), Arc::from(bytes.into_boxed_slice()));
        }

        // Ensure the active public key matches the private key.
        if let Some(existing) = public_keys.get(&private_jwk.kid) {
            if existing.as_ref() != public_key_bytes.as_ref() {
                return Err(AuthStoreError::InvalidKey(
                    "JWKS entry for active kid does not match private key".to_string(),
                ));
            }
        } else {
            public_keys.insert(private_jwk.kid.clone(), public_key_bytes);
        }

        debug!(active_kid = %active_signing_key.kid, num_keys = jwks.keys.len(), "Loaded JWKS");

        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            active: RwLock::new(active_signing_key),
            public_keys: RwLock::new(public_keys),
            jwks: RwLock::new(jwks),
        })
    }

    /// Verify file has secure permissions (0600).
    #[cfg(unix)]
    fn verify_permissions(path: &Path) -> Result<(), AuthStoreError> {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(path)?;
        let mode = metadata.permissions().mode() & 0o777;

        if mode != 0o600 {
            return Err(AuthStoreError::InsecurePermissions {
                path: path.display().to_string(),
                actual: mode,
            });
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn verify_permissions(_path: &Path) -> Result<(), AuthStoreError> {
        // Non-Unix platforms: skip permission check
        Ok(())
    }

    /// Write file with secure permissions (0600).
    pub(crate) async fn write_secure(path: &Path, content: &str) -> Result<(), AuthStoreError> {
        use tokio::io::AsyncWriteExt;

        // Write to a unique temp file first to avoid partially-written files.
        let temp_path = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));

        #[cfg(unix)]
        {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_path)
                .await?;

            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
            drop(file);
        }

        #[cfg(not(unix))]
        {
            let mut file =
                tokio::fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).await?;

            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
            drop(file);
        }

        // Atomic rename (same directory).
        tokio::fs::rename(&temp_path, path).await?;
        Ok(())
    }
}

#[async_trait]
#[allow(clippy::expect_used)]
impl KeyProvider for FileKeyProvider {
    fn active_key(&self) -> SigningKeyMaterial {
        lock_read(&self.active).clone()
    }

    fn verification_key(&self, kid: &str) -> Option<VerificationKeyMaterial> {
        let decoded = lock_read(&self.public_keys);
        decoded
            .get(kid)
            .map(|public_key| VerificationKeyMaterial { public_key: public_key.clone() })
    }

    fn valid_kids(&self) -> Vec<String> {
        lock_read(&self.public_keys).keys().cloned().collect()
    }

    fn jwks(&self) -> Jwks {
        lock_read(&self.jwks).clone()
    }

    async fn rotate(&self) -> Result<SigningKeyMaterial, AuthStoreError> {
        let private_path = self.state_dir.join(PRIVATE_JWK_FILENAME);
        let jwks_path = self.state_dir.join(PUBLIC_JWKS_FILENAME);

        let (private_jwk, new_signing_key, public_key_bytes) = generate_new_private_key()?;

        let mut jwks = lock_read(&self.jwks).clone();
        if !jwks.keys.iter().any(|k| k.kid == private_jwk.kid) {
            jwks.keys.push(private_jwk.to_public_jwk());
        }

        // Persist JWKS first so the new kid becomes verifiable before switching active key.
        Self::write_secure(&jwks_path, &serde_json::to_string_pretty(&jwks)?).await?;
        Self::write_secure(&private_path, &serde_json::to_string_pretty(&private_jwk)?).await?;

        {
            let mut active = lock_write(&self.active);
            *active = new_signing_key.clone();
        }
        {
            let mut public_keys = lock_write(&self.public_keys);
            public_keys.insert(private_jwk.kid.clone(), public_key_bytes);
        }
        {
            let mut jwks_lock = lock_write(&self.jwks);
            *jwks_lock = jwks.clone();
        }

        info!(kid = %new_signing_key.kid, total_keys = jwks.keys.len(), "Rotated signing key");

        Ok(new_signing_key)
    }
}

fn base64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn base64url_decode(encoded: &str) -> Result<Vec<u8>, AuthStoreError> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded)?)
}

fn derive_keypair(seed: &[u8]) -> Result<(Vec<u8>, Vec<u8>), AuthStoreError> {
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};

    let key_pair = Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|e| AuthStoreError::InvalidKey(format!("Invalid Ed25519 seed: {e}")))?;

    let public_key = key_pair.public_key().as_ref().to_vec();
    let pkcs8 = key_pair
        .to_pkcs8()
        .map_err(|e| AuthStoreError::InvalidKey(format!("Failed to encode PKCS#8: {e}")))?
        .as_ref()
        .to_vec();

    Ok((public_key, pkcs8))
}

fn generate_new_private_key() -> Result<(PrivateJwk, SigningKeyMaterial, Arc<[u8]>), AuthStoreError>
{
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|e| AuthStoreError::InvalidKey(format!("RNG failure: {e}")))?;
    let kid = uuid::Uuid::new_v4().to_string();

    let (public_key, pkcs8) = derive_keypair(&seed)?;

    let private = PrivateJwk {
        kty: "OKP".to_string(),
        crv: "Ed25519".to_string(),
        public_key_use: "sig".to_string(),
        alg: "EdDSA".to_string(),
        kid: kid.clone(),
        x: base64url_encode(&public_key),
        d: base64url_encode(&seed),
    };

    let signing = SigningKeyMaterial { kid, pkcs8: Arc::from(pkcs8.into_boxed_slice()) };
    let public_key_arc: Arc<[u8]> = Arc::from(public_key.into_boxed_slice());

    Ok((private, signing, public_key_arc))
}

/// File-based revocation store with in-memory lookup.
///
/// Revocations are stored in `revoked.json` and loaded into memory at startup.
/// The `is_revoked` check is a fast HashSet lookup.
pub struct FileRevocationStore {
    state_dir: PathBuf,
    /// In-memory map for fast lookups (token_hash -> exp)
    revoked: RwLock<HashMap<String, u64>>,
}

impl FileRevocationStore {
    /// Create a new store and load existing revocations.
    ///
    /// # Errors
    ///
    /// Returns errors for I/O failures or JSON parsing errors.
    pub async fn new(state_dir: &Path) -> Result<Self, AuthStoreError> {
        tokio::fs::create_dir_all(state_dir).await?;

        let store =
            Self { state_dir: state_dir.to_path_buf(), revoked: RwLock::new(HashMap::new()) };
        store.load().await?;
        Ok(store)
    }

    fn now_secs_lossy() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
    }

    fn prune_expired_locked(map: &mut HashMap<String, u64>) {
        let now = Self::now_secs_lossy();
        map.retain(|_, exp| *exp == 0 || *exp > now);
    }

    /// Persist revocations atomically.
    async fn persist(&self) -> Result<(), AuthStoreError> {
        let data = {
            let revoked = lock_read(&self.revoked);
            serde_json::to_string_pretty(&*revoked)?
        };
        let path = self.state_dir.join("revoked.json");
        FileKeyProvider::write_secure(&path, &data).await?;

        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RevokedOnDisk {
    Map(HashMap<String, u64>),
    Set(HashSet<String>),
}

#[async_trait]
impl RevocationStore for FileRevocationStore {
    fn is_revoked(&self, token_hash: &str) -> bool {
        // Sync in-memory check (fast path, no await)
        lock_read(&self.revoked).contains_key(token_hash)
    }

    async fn revoke(&self, token_hash: &str, exp: u64) -> Result<(), AuthStoreError> {
        lock_write(&self.revoked).insert(token_hash.to_string(), exp);
        Self::prune_expired_locked(&mut lock_write(&self.revoked));
        self.persist().await?;
        debug!(token_hash = %token_hash, "Token revoked");
        Ok(())
    }

    async fn load(&self) -> Result<(), AuthStoreError> {
        let path = self.state_dir.join("revoked.json");
        if path.exists() {
            FileKeyProvider::verify_permissions(&path)?;
            let data = tokio::fs::read_to_string(&path).await?;
            let revoked: RevokedOnDisk = serde_json::from_str(&data)?;
            let mut map = match revoked {
                RevokedOnDisk::Map(map) => map,
                RevokedOnDisk::Set(set) => set.into_iter().map(|h| (h, 0)).collect(),
            };
            Self::prune_expired_locked(&mut map);
            let count = map.len();
            *lock_write(&self.revoked) = map;
            debug!(count, "Loaded revocations from disk");
        }
        Ok(())
    }
}

/// File-based token metadata store.
///
/// Stores metadata in `tokens.json`. This is used to:
/// - List all minted tokens for admin UI
/// - Enforce "tokens we mint" policy
/// - Track revocation status
pub struct FileTokenMetadataStore {
    state_dir: PathBuf,
    /// In-memory cache of all tokens (jti -> metadata)
    tokens: RwLock<HashMap<String, TokenMetadata>>,
}

impl FileTokenMetadataStore {
    /// Create a new store and load existing metadata.
    ///
    /// # Errors
    ///
    /// Returns errors for I/O failures or JSON parsing errors.
    pub async fn new(state_dir: &Path) -> Result<Self, AuthStoreError> {
        tokio::fs::create_dir_all(state_dir).await?;

        let store =
            Self { state_dir: state_dir.to_path_buf(), tokens: RwLock::new(HashMap::new()) };

        // Load existing tokens
        let path = state_dir.join("tokens.json");
        if path.exists() {
            FileKeyProvider::verify_permissions(&path)?;
            let data = tokio::fs::read_to_string(&path).await?;
            let tokens: Vec<TokenMetadata> = serde_json::from_str(&data)?;
            let count = tokens.len();
            {
                let mut map = lock_write(&store.tokens);
                for token in tokens {
                    map.insert(token.jti.clone(), token);
                }
            }
            debug!(count, "Loaded token metadata from disk");
        }

        Ok(store)
    }

    /// Persist tokens atomically.
    async fn persist(&self) -> Result<(), AuthStoreError> {
        let list: Vec<TokenMetadata> = lock_read(&self.tokens).values().cloned().collect();
        let data = serde_json::to_string_pretty(&list)?;

        let path = self.state_dir.join("tokens.json");
        FileKeyProvider::write_secure(&path, &data).await?;

        Ok(())
    }
}

#[async_trait]
impl TokenMetadataStore for FileTokenMetadataStore {
    async fn store(&self, meta: TokenMetadata) -> Result<(), AuthStoreError> {
        let jti = meta.jti.clone();
        lock_write(&self.tokens).insert(jti.clone(), meta);
        self.persist().await?;
        debug!(jti = %jti, "Stored token metadata");
        Ok(())
    }

    async fn exists(&self, jti: &str) -> bool {
        lock_read(&self.tokens).contains_key(jti)
    }

    async fn list(&self) -> Result<Vec<TokenMetadata>, AuthStoreError> {
        Ok(lock_read(&self.tokens).values().cloned().collect())
    }

    async fn mark_revoked(&self, jti: &str) -> Result<(), AuthStoreError> {
        {
            let mut tokens = lock_write(&self.tokens);
            if let Some(token) = tokens.get_mut(jti) {
                token.revoked = true;
            } else {
                warn!(jti = %jti, "Attempted to mark non-existent token as revoked");
            }
        }
        self.persist().await?;
        Ok(())
    }

    async fn get(&self, jti: &str) -> Result<Option<TokenMetadata>, AuthStoreError> {
        Ok(lock_read(&self.tokens).get(jti).cloned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_key_provider_init_and_active() {
        let temp_dir = TempDir::new().unwrap();
        let provider = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());

        let provider_clone = provider.clone();
        let key = tokio::task::spawn_blocking(move || provider_clone.active_key()).await.unwrap();
        assert!(!key.kid.is_empty());
        assert!(!key.pkcs8.is_empty());

        // Active key must be present in JWKS and verifiable via its kid.
        let jwks = tokio::task::spawn_blocking({
            let provider = provider.clone();
            move || provider.jwks()
        })
        .await
        .unwrap();
        assert!(jwks.keys.iter().any(|jwk| jwk.kid == key.kid));

        let verification = tokio::task::spawn_blocking({
            let provider = provider.clone();
            let kid = key.kid.clone();
            move || provider.verification_key(&kid)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(verification.public_key.len(), 32);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_key_provider_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let provider = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());

        let old_key = tokio::task::spawn_blocking({
            let provider = provider.clone();
            move || provider.active_key()
        })
        .await
        .unwrap();
        let old_kid = old_key.kid.clone();

        let old_public = tokio::task::spawn_blocking({
            let provider = provider.clone();
            let kid = old_kid.clone();
            move || provider.verification_key(&kid).unwrap().public_key
        })
        .await
        .unwrap();

        let new_key = provider.rotate().await.unwrap();
        assert_ne!(old_kid, new_key.kid);

        // Old key should still be available for verification
        let verification_key = tokio::task::spawn_blocking({
            let provider = provider.clone();
            let kid = old_kid.clone();
            move || provider.verification_key(&kid)
        })
        .await
        .unwrap();
        assert!(verification_key.is_some());
        assert_eq!(verification_key.unwrap().public_key.as_ref(), old_public.as_ref());

        // New key should be active
        let active = tokio::task::spawn_blocking({
            let provider = provider.clone();
            move || provider.active_key()
        })
        .await
        .unwrap();
        assert_eq!(active.kid, new_key.kid);

        // JWKS should contain both keys.
        let jwks = tokio::task::spawn_blocking({
            let provider = provider.clone();
            move || provider.jwks()
        })
        .await
        .unwrap();
        assert!(jwks.keys.iter().any(|jwk| jwk.kid == old_kid));
        assert!(jwks.keys.iter().any(|jwk| jwk.kid == new_key.kid));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_revocation_store() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());

        let token_hash = "test-hash-123";
        let store_clone = store.clone();
        let hash_clone = token_hash.to_string();
        let is_revoked =
            tokio::task::spawn_blocking(move || store_clone.is_revoked(&hash_clone)).await.unwrap();
        assert!(!is_revoked);

        store.revoke(token_hash, 0).await.unwrap();

        let store_clone = store.clone();
        let hash_clone = token_hash.to_string();
        let is_revoked =
            tokio::task::spawn_blocking(move || store_clone.is_revoked(&hash_clone)).await.unwrap();
        assert!(is_revoked);

        // Reload and verify persistence
        let store2 = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());
        let hash_clone = token_hash.to_string();
        let is_revoked =
            tokio::task::spawn_blocking(move || store2.is_revoked(&hash_clone)).await.unwrap();
        assert!(is_revoked);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_token_metadata_store() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileTokenMetadataStore::new(temp_dir.path()).await.unwrap();

        let meta = TokenMetadata {
            jti: "test-jti".to_string(),
            token_hash: "abc123".to_string(),
            token_type: super::super::TokenType::Api,
            role: Some("admin".to_string()),
            label: Some("Test token".to_string()),
            created_at: 1_234_567_890,
            exp: 1_234_657_890,
            revoked: false,
            created_by: "bootstrap".to_string(),
        };

        store.store(meta.clone()).await.unwrap();
        assert!(store.exists("test-jti").await);
        assert!(!store.exists("nonexistent").await);

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].jti, "test-jti");

        store.mark_revoked("test-jti").await.unwrap();
        let retrieved = store.get("test-jti").await.unwrap().unwrap();
        assert!(retrieved.revoked);
    }
}
