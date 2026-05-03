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

/// RAII guard for an advisory file lock (`flock`).
///
/// The lock is released when the guard is dropped (file close).
struct FileLockGuard {
    _file: std::fs::File,
}

/// Acquire an exclusive advisory file lock on a companion `.lock` file.
///
/// Provides cross-process serialization of read-modify-write operations
/// so the CLI and server cannot clobber each other's writes when running
/// concurrently (e.g. during `rotate-key`).
async fn acquire_file_lock(data_path: &Path) -> Result<FileLockGuard, AuthStoreError> {
    use fs2::FileExt;

    let lock_path = lock_path_for(data_path);
    tokio::task::spawn_blocking(move || -> Result<FileLockGuard, AuthStoreError> {
        let file =
            std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(&lock_path)?;
        file.lock_exclusive()?;
        Ok(FileLockGuard { _file: file })
    })
    .await
    .map_err(|e| AuthStoreError::Io(std::io::Error::other(e)))?
}

fn lock_path_for(data_path: &Path) -> PathBuf {
    let mut s = data_path.as_os_str().to_os_string();
    s.push(".lock");
    PathBuf::from(s)
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
    /// Serializes rotate/reload to prevent lost-update races.
    mutation_lock: tokio::sync::Mutex<()>,
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
            mutation_lock: tokio::sync::Mutex::new(()),
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
    ///
    /// Ensures crash durability by calling `sync_all()` on the file
    /// before the atomic rename and on the parent directory afterwards.
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
            file.sync_all().await?;
        }

        #[cfg(not(unix))]
        {
            let mut file =
                tokio::fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).await?;

            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
            file.sync_all().await?;
        }

        // Atomic rename (same directory).
        if let Err(e) = tokio::fs::rename(&temp_path, path).await {
            // Best-effort cleanup of the temp file.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e.into());
        }

        // Fsync parent directory so the new directory entry is durable.
        if let Some(parent) = path.parent() {
            if let Ok(dir) = tokio::fs::File::open(parent).await {
                let _ = dir.sync_all().await;
            }
        }

        Ok(())
    }

    /// Atomically swap all three in-memory caches.
    ///
    /// All three write locks are held simultaneously so no reader can
    /// observe a partially-swapped state (e.g. an active kid whose
    /// verification key isn't yet in the map).
    //
    // Clippy wants to tighten each guard's scope, but releasing any
    // lock before the others are updated breaks the atomicity
    // guarantee — a reader between releases could see a new active
    // kid whose verification key isn't in the map yet.
    #[allow(clippy::significant_drop_tightening)]
    fn swap_state(
        &self,
        new_active: SigningKeyMaterial,
        new_public_keys_delta: Option<(String, Arc<[u8]>)>,
        full_public_keys: Option<HashMap<String, Arc<[u8]>>>,
        new_jwks: Jwks,
    ) {
        let mut active = lock_write(&self.active);
        let mut public_keys = lock_write(&self.public_keys);
        let mut jwks_lock = lock_write(&self.jwks);

        if let Some(full) = full_public_keys {
            *public_keys = full;
        }
        if let Some((kid, key)) = new_public_keys_delta {
            public_keys.insert(kid, key);
        }
        *jwks_lock = new_jwks;
        *active = new_active;
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
        // Serialize mutations so two concurrent rotations cannot
        // each snapshot the same base JWKS and clobber each other.
        let _guard = self.mutation_lock.lock().await;

        let private_path = self.state_dir.join(PRIVATE_JWK_FILENAME);
        let jwks_path = self.state_dir.join(PUBLIC_JWKS_FILENAME);

        // Cross-process file lock so the CLI and server cannot
        // clobber each other's key writes during concurrent rotation.
        let _flock = acquire_file_lock(&jwks_path).await?;

        let (private_jwk, new_signing_key, public_key_bytes) = generate_new_private_key()?;

        // Re-read JWKS from disk (not in-memory) to pick up any
        // changes persisted by a concurrent process.
        let mut jwks: Jwks = if jwks_path.exists() {
            let content = tokio::fs::read_to_string(&jwks_path).await?;
            serde_json::from_str(&content)?
        } else {
            Jwks { keys: vec![] }
        };
        if !jwks.keys.iter().any(|k| k.kid == private_jwk.kid) {
            jwks.keys.push(private_jwk.to_public_jwk());
        }

        Self::write_secure(&jwks_path, &serde_json::to_string_pretty(&jwks)?).await?;
        Self::write_secure(&private_path, &serde_json::to_string_pretty(&private_jwk)?).await?;

        self.swap_state(
            new_signing_key.clone(),
            Some((private_jwk.kid.clone(), public_key_bytes)),
            None,
            jwks.clone(),
        );

        info!(kid = %new_signing_key.kid, total_keys = jwks.keys.len(), "Rotated signing key");

        Ok(new_signing_key)
    }

    async fn reload(&self) -> Result<(), AuthStoreError> {
        let _guard = self.mutation_lock.lock().await;

        let private_path = self.state_dir.join(PRIVATE_JWK_FILENAME);
        let jwks_path = self.state_dir.join(PUBLIC_JWKS_FILENAME);

        // Cross-process file lock so reload doesn't read a
        // partially-written file during a concurrent rotation.
        let _flock = acquire_file_lock(&jwks_path).await?;

        Self::verify_permissions(&private_path)?;
        let content = tokio::fs::read_to_string(&private_path).await?;
        let private: PrivateJwk = serde_json::from_str(&content)?;
        private.validate()?;

        let seed = base64url_decode(&private.d)?;
        let public_from_file = base64url_decode(&private.x)?;

        if seed.len() != 32 {
            return Err(AuthStoreError::InvalidKey("Ed25519 seed must be 32 bytes".to_string()));
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

        let new_active = SigningKeyMaterial {
            kid: private.kid.clone(),
            pkcs8: Arc::from(pkcs8.into_boxed_slice()),
        };

        // jwks.json is public data — no verify_permissions check needed.
        let jwks: Jwks = if jwks_path.exists() {
            let content = tokio::fs::read_to_string(&jwks_path).await?;
            serde_json::from_str(&content)?
        } else {
            Jwks { keys: vec![] }
        };

        let mut new_public_keys: HashMap<String, Arc<[u8]>> = HashMap::new();
        for jwk in &jwks.keys {
            let bytes = base64url_decode(&jwk.x)?;
            if bytes.len() != 32 {
                return Err(AuthStoreError::InvalidKey(format!(
                    "Invalid public key length for kid {}",
                    jwk.kid
                )));
            }
            new_public_keys.insert(jwk.kid.clone(), Arc::from(bytes.into_boxed_slice()));
        }

        // Ensure the active public key is in the map (mirrors load_or_init behaviour).
        let active_pub: Arc<[u8]> = Arc::from(public_from_file.into_boxed_slice());
        if let Some(existing) = new_public_keys.get(&private.kid) {
            if existing.as_ref() != active_pub.as_ref() {
                return Err(AuthStoreError::InvalidKey(
                    "JWKS entry for active kid does not match private key".to_string(),
                ));
            }
        } else {
            new_public_keys.insert(private.kid.clone(), active_pub);
        }

        // Keep the JWKS struct consistent with public_keys so that
        // /.well-known/jwks.json always advertises the active key.
        let mut jwks = jwks;
        if !jwks.keys.iter().any(|k| k.kid == private.kid) {
            jwks.keys.push(private.to_public_jwk());
        }

        let total_keys = jwks.keys.len();

        self.swap_state(new_active.clone(), None, Some(new_public_keys), jwks);

        info!(kid = %new_active.kid, total_keys, "Reloaded keys from disk");

        Ok(())
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
        store.reload().await?;
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

    /// Re-read revocations from disk into the in-memory cache.
    ///
    /// Callers must hold the file lock when using this as part of a
    /// read-modify-write cycle.
    async fn reload_from_disk(&self) -> Result<(), AuthStoreError> {
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

    // Hold a single write lock for both the insert and the prune
    // so no reader can observe a partially-updated map.
    #[allow(clippy::significant_drop_tightening)]
    async fn revoke(&self, token_hash: &str, exp: u64) -> Result<(), AuthStoreError> {
        let data_path = self.state_dir.join("revoked.json");
        let _flock = acquire_file_lock(&data_path).await?;
        self.reload_from_disk().await?;
        {
            let mut guard = lock_write(&self.revoked);
            guard.insert(token_hash.to_string(), exp);
            Self::prune_expired_locked(&mut guard);
        }
        self.persist().await?;
        debug!(token_hash = %token_hash, "Token revoked");
        Ok(())
    }

    async fn reload(&self) -> Result<(), AuthStoreError> {
        let data_path = self.state_dir.join("revoked.json");
        let _flock = acquire_file_lock(&data_path).await?;
        self.reload_from_disk().await
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

    /// Re-read token metadata from disk into the in-memory cache.
    ///
    /// Callers must hold the file lock when using this as part of a
    /// read-modify-write cycle.
    async fn reload_from_disk(&self) -> Result<(), AuthStoreError> {
        let path = self.state_dir.join("tokens.json");
        if !path.exists() {
            return Ok(());
        }
        FileKeyProvider::verify_permissions(&path)?;
        let data = tokio::fs::read_to_string(&path).await?;
        let tokens: Vec<TokenMetadata> = serde_json::from_str(&data)?;
        let count = tokens.len();
        let mut new_tokens: HashMap<String, TokenMetadata> = HashMap::with_capacity(count);
        for token in tokens {
            new_tokens.insert(token.jti.clone(), token);
        }
        *lock_write(&self.tokens) = new_tokens;
        debug!(count, "Reloaded token metadata from disk");
        Ok(())
    }
}

#[async_trait]
impl TokenMetadataStore for FileTokenMetadataStore {
    async fn store(&self, meta: TokenMetadata) -> Result<(), AuthStoreError> {
        let jti = meta.jti.clone();
        let data_path = self.state_dir.join("tokens.json");
        let _flock = acquire_file_lock(&data_path).await?;
        self.reload_from_disk().await?;
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
        let data_path = self.state_dir.join("tokens.json");
        let _flock = acquire_file_lock(&data_path).await?;
        self.reload_from_disk().await?;
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

    async fn reload(&self) -> Result<(), AuthStoreError> {
        let data_path = self.state_dir.join("tokens.json");
        let _flock = acquire_file_lock(&data_path).await?;
        self.reload_from_disk().await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::{TokenMetadata, TokenType};
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
    async fn test_key_provider_reload_after_external_rotation() {
        let temp_dir = TempDir::new().unwrap();

        // Simulate two independent instances sharing the same state directory,
        // like the CLI and the running server.
        let server_provider =
            Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());
        let cli_provider = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());

        let original_kid = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            move || p.active_key().kid
        })
        .await
        .unwrap();

        // CLI rotates the key (writes new key material to disk).
        let new_key = cli_provider.rotate().await.unwrap();
        assert_ne!(original_kid, new_key.kid);

        // Server instance still sees the old active key and does NOT
        // have the new kid in its verification set — this is the bug.
        let stale_kid = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            move || p.active_key().kid
        })
        .await
        .unwrap();
        assert_eq!(stale_kid, original_kid, "server should still have the old key before reload");

        let missing = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            let kid = new_key.kid.clone();
            move || p.verification_key(&kid)
        })
        .await
        .unwrap();
        assert!(missing.is_none(), "new kid should be unknown before reload");

        // After reload the server picks up the on-disk changes.
        server_provider.reload().await.unwrap();

        let reloaded_kid = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            move || p.active_key().kid
        })
        .await
        .unwrap();
        assert_eq!(reloaded_kid, new_key.kid, "active key should match after reload");

        let found = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            let kid = new_key.kid.clone();
            move || p.verification_key(&kid)
        })
        .await
        .unwrap();
        assert!(found.is_some(), "new kid should be verifiable after reload");

        // Old key should still be verifiable (kept in JWKS).
        let old_found = tokio::task::spawn_blocking({
            let p = server_provider.clone();
            move || p.verification_key(&original_kid)
        })
        .await
        .unwrap();
        assert!(old_found.is_some(), "old kid should remain verifiable after reload");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_rotations_preserve_all_kids() {
        let temp_dir = TempDir::new().unwrap();
        let provider = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());
        let original_kid = {
            let p = provider.clone();
            tokio::task::spawn_blocking(move || p.active_key().kid).await.unwrap()
        };

        // Spawn two concurrent rotations.
        let p1 = provider.clone();
        let p2 = provider.clone();
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { p1.rotate().await.unwrap() }),
            tokio::spawn(async move { p2.rotate().await.unwrap() }),
        );
        let key1 = r1.unwrap();
        let key2 = r2.unwrap();

        // All three kids (original + both rotations) must be verifiable.
        let p = provider.clone();
        let kids = tokio::task::spawn_blocking(move || p.valid_kids()).await.unwrap();
        assert!(kids.contains(&original_kid), "original kid lost after concurrent rotate");
        assert!(kids.contains(&key1.kid), "key1 kid lost after concurrent rotate");
        assert!(kids.contains(&key2.kid), "key2 kid lost after concurrent rotate");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_reload_missing_private_key_errors() {
        let temp_dir = TempDir::new().unwrap();
        let provider = FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap();

        // Remove the private key file.
        tokio::fs::remove_file(temp_dir.path().join(PRIVATE_JWK_FILENAME)).await.unwrap();

        let result = provider.reload().await;
        assert!(result.is_err(), "reload should fail when auth.jwk is missing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_reload_reinserts_active_kid_into_jwks() {
        let temp_dir = TempDir::new().unwrap();
        let provider = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());

        let active_kid = {
            let p = provider.clone();
            tokio::task::spawn_blocking(move || p.active_key().kid).await.unwrap()
        };

        // Strip the active kid from the on-disk JWKS (simulates hand-editing).
        let jwks_path = temp_dir.path().join(PUBLIC_JWKS_FILENAME);
        tokio::fs::write(&jwks_path, r#"{"keys":[]}"#).await.unwrap();

        provider.reload().await.unwrap();

        // After reload, the active kid should be re-inserted into JWKS.
        let p = provider.clone();
        let jwks = tokio::task::spawn_blocking(move || p.jwks()).await.unwrap();
        assert!(
            jwks.keys.iter().any(|k| k.kid == active_kid),
            "active kid should be re-inserted into JWKS after reload"
        );

        // And verifiable.
        let p = provider.clone();
        let kid_clone = active_kid.clone();
        let vk = tokio::task::spawn_blocking(move || p.verification_key(&kid_clone)).await.unwrap();
        assert!(vk.is_some(), "active kid should be verifiable after reload");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_key_provider_reload_corrupt_jwks() {
        let temp_dir = TempDir::new().unwrap();
        let provider = FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap();

        // Corrupt the JWKS file on disk.
        let jwks_path = temp_dir.path().join(PUBLIC_JWKS_FILENAME);
        tokio::fs::write(&jwks_path, "not valid json").await.unwrap();

        let result = provider.reload().await;
        assert!(result.is_err(), "reload should fail on corrupt jwks.json");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_token_metadata_store_reload() {
        let temp_dir = TempDir::new().unwrap();
        let store_a = FileTokenMetadataStore::new(temp_dir.path()).await.unwrap();
        let store_b = FileTokenMetadataStore::new(temp_dir.path()).await.unwrap();

        let meta = TokenMetadata {
            jti: "reload-jti".to_string(),
            token_hash: "hash".to_string(),
            token_type: TokenType::Api,
            role: Some("admin".to_string()),
            label: Some("test".to_string()),
            created_at: 1000,
            exp: 2000,
            revoked: false,
            created_by: "test".to_string(),
        };

        // store_a writes a token; store_b doesn't know about it yet.
        store_a.store(meta).await.unwrap();
        assert!(!store_b.exists("reload-jti").await);

        // After reload, store_b picks it up.
        store_b.reload().await.unwrap();
        assert!(store_b.exists("reload-jti").await);
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

    // --- Tests for #345: revoke() single lock acquisition ---

    #[tokio::test(flavor = "multi_thread")]
    async fn test_revoke_does_not_deadlock() {
        // Regression: revoke() previously acquired the write lock twice
        // (once for insert, once for prune_expired), which could panic on
        // re-entrant locking or deadlock with concurrent readers.
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());

        // Rapid sequential revocations must not deadlock.
        for i in 0..20 {
            store.revoke(&format!("hash-{i}"), 0).await.unwrap();
        }

        // All 20 hashes must be present.
        for i in 0..20 {
            let s = store.clone();
            let hash = format!("hash-{i}");
            let revoked = tokio::task::spawn_blocking(move || s.is_revoked(&hash)).await.unwrap();
            assert!(revoked, "hash-{i} should be revoked");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_revoke_prunes_expired_in_same_lock() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());

        // Insert a revocation that has already expired (exp = 1).
        store.revoke("expired-hash", 1).await.unwrap();

        // Insert a live revocation — prune should remove the expired one
        // in the same lock acquisition.
        store.revoke("live-hash", 0).await.unwrap();

        let s = store.clone();
        let expired =
            tokio::task::spawn_blocking(move || s.is_revoked("expired-hash")).await.unwrap();
        assert!(!expired, "expired revocation should have been pruned");

        let s = store.clone();
        let live = tokio::task::spawn_blocking(move || s.is_revoked("live-hash")).await.unwrap();
        assert!(live, "live revocation should remain");
    }

    // --- Tests for #331: fsync after write_secure ---

    #[tokio::test(flavor = "multi_thread")]
    async fn test_write_secure_fsync_durability() {
        // Verify write_secure produces durable files: the data is readable
        // immediately after the call returns (sync_all was called).
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("durable.json");
        let content = r#"{"test": "fsync"}"#;

        FileKeyProvider::write_secure(&path, content).await.unwrap();

        // Re-read to confirm content survived the fsync + rename.
        let read_back = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(read_back, content);

        // Verify permissions are 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "write_secure should set 0600 permissions");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_write_secure_overwrites_atomically() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("atomic.json");

        FileKeyProvider::write_secure(&path, "first").await.unwrap();
        FileKeyProvider::write_secure(&path, "second").await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "second");

        // No leftover temp files.
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(entries.is_empty(), "temp files should be cleaned up");
    }

    // --- Tests for #329: flock-based cross-process file locking ---

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_token_stores_no_lost_updates() {
        // Simulate two independent store instances (like CLI + server)
        // writing tokens concurrently. With file locking, no writes
        // should be lost.
        let temp_dir = TempDir::new().unwrap();
        let store_a = Arc::new(FileTokenMetadataStore::new(temp_dir.path()).await.unwrap());
        let store_b = Arc::new(FileTokenMetadataStore::new(temp_dir.path()).await.unwrap());

        let make_meta = |id: &str| TokenMetadata {
            jti: id.to_string(),
            token_hash: format!("hash-{id}"),
            token_type: TokenType::Api,
            role: Some("admin".to_string()),
            label: Some(format!("token-{id}")),
            created_at: 1000,
            exp: 2000,
            revoked: false,
            created_by: "test".to_string(),
        };

        // Store tokens concurrently from both instances.
        let sa = store_a.clone();
        let sb = store_b.clone();
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move {
                for i in 0..5 {
                    sa.store(make_meta(&format!("a-{i}"))).await.unwrap();
                }
            }),
            tokio::spawn(async move {
                for i in 0..5 {
                    sb.store(make_meta(&format!("b-{i}"))).await.unwrap();
                }
            }),
        );
        r1.unwrap();
        r2.unwrap();

        // Read the on-disk state with a fresh store to verify nothing was lost.
        let verifier = FileTokenMetadataStore::new(temp_dir.path()).await.unwrap();
        let all = verifier.list().await.unwrap();
        assert_eq!(all.len(), 10, "all 10 tokens must survive concurrent writes");

        for i in 0..5 {
            assert!(verifier.exists(&format!("a-{i}")).await, "a-{i} missing");
            assert!(verifier.exists(&format!("b-{i}")).await, "b-{i} missing");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_revocations_no_lost_updates() {
        let temp_dir = TempDir::new().unwrap();
        let store_a = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());
        let store_b = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());

        let sa = store_a.clone();
        let sb = store_b.clone();
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move {
                for i in 0..5 {
                    sa.revoke(&format!("a-{i}"), 0).await.unwrap();
                }
            }),
            tokio::spawn(async move {
                for i in 0..5 {
                    sb.revoke(&format!("b-{i}"), 0).await.unwrap();
                }
            }),
        );
        r1.unwrap();
        r2.unwrap();

        // Verify with a fresh store instance.
        let verifier = Arc::new(FileRevocationStore::new(temp_dir.path()).await.unwrap());
        for i in 0..5 {
            let v = verifier.clone();
            let hash = format!("a-{i}");
            assert!(
                tokio::task::spawn_blocking(move || v.is_revoked(&hash)).await.unwrap(),
                "a-{i} missing"
            );
            let v = verifier.clone();
            let hash = format!("b-{i}");
            assert!(
                tokio::task::spawn_blocking(move || v.is_revoked(&hash)).await.unwrap(),
                "b-{i} missing"
            );
        }
    }

    // --- Tests for rotate() cross-process file locking ---

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_rotations_no_lost_keys() {
        // Simulate two independent KeyProvider instances (CLI + server)
        // rotating concurrently.  With flock, all generated keys must
        // appear in the final JWKS.
        let temp_dir = TempDir::new().unwrap();
        let provider_a = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());
        let provider_b = Arc::new(FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap());

        let pa = provider_a.clone();
        let pb = provider_b.clone();
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move {
                for _ in 0..3 {
                    pa.rotate().await.unwrap();
                }
            }),
            tokio::spawn(async move {
                for _ in 0..3 {
                    pb.rotate().await.unwrap();
                }
            }),
        );
        r1.unwrap();
        r2.unwrap();

        // Re-read from disk with a fresh provider to verify.
        let verifier = FileKeyProvider::load_or_init(temp_dir.path()).await.unwrap();
        let jwks = verifier.jwks();
        // 1 initial key + 6 rotations = 7 keys total.
        assert_eq!(
            jwks.keys.len(),
            7,
            "all rotated keys must survive concurrent writes (found {})",
            jwks.keys.len()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_file_lock_is_exclusive() {
        // Verify that two concurrent lock acquisitions are serialized.
        let temp_dir = TempDir::new().unwrap();
        let lock_target = temp_dir.path().join("test.json");

        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let max_concurrent = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let path = lock_target.clone();
            let ctr = counter.clone();
            let max = max_concurrent.clone();
            handles.push(tokio::spawn(async move {
                let _guard = acquire_file_lock(&path).await.unwrap();
                let cur = ctr.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max.fetch_max(cur, std::sync::atomic::Ordering::SeqCst);
                // Hold the lock briefly.
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                ctr.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(
            max_concurrent.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "file lock must be exclusive — at most 1 holder at a time"
        );
    }
}
