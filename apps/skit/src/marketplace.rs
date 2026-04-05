// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::HashMap,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::{engine::general_purpose, Engine as _};
use blake2::{digest::consts::U64, Blake2b, Digest};
use bytes::Bytes;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::marketplace_security::{validated_get_bytes, MarketplaceUrlPolicy, OriginKey};

const MINISIGN_ALGO_ED25519: [u8; 2] = *b"Ed";
const MINISIGN_ALGO_ED25519_HASHED: [u8; 2] = *b"ED";
const MINISIGN_PUBLIC_KEY_LEN: usize = 42;
const MINISIGN_SIGNATURE_LEN: usize = 74;
const MAX_INDEX_CACHE_ENTRIES: usize = 32;
const MAX_MANIFEST_CACHE_ENTRIES: usize = 128;

#[derive(Debug, Clone)]
pub struct MinisignPublicKey {
    key_id: [u8; 8],
    public_key: [u8; 32],
}

impl MinisignPublicKey {
    /// Parses a minisign public key string.
    ///
    /// # Errors
    ///
    /// Returns an error if the key is missing, malformed, or not Ed25519.
    pub fn parse(input: &str) -> Result<Self> {
        let line = extract_minisign_payload(input)?;
        let decoded = decode_base64_line(&line).context("Failed to decode minisign public key")?;

        if decoded.len() != MINISIGN_PUBLIC_KEY_LEN {
            return Err(anyhow!(
                "Invalid minisign public key length: expected {MINISIGN_PUBLIC_KEY_LEN} bytes"
            ));
        }
        if decoded[0..2] != MINISIGN_ALGO_ED25519 {
            return Err(anyhow!("Unsupported minisign public key algorithm: expected Ed25519"));
        }

        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&decoded[2..10]);

        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&decoded[10..42]);

        Ok(Self { key_id, public_key })
    }

    pub fn key_id_hex(&self) -> String {
        key_id_hex(self.key_id)
    }

    /// Verifies a minisign signature against the provided message.
    ///
    /// # Errors
    ///
    /// Returns an error if the key ID does not match or the signature fails verification.
    pub fn verify(&self, signature: &MinisignSignature, message: &[u8]) -> Result<()> {
        if self.key_id != signature.key_id {
            return Err(anyhow!("Signature key id does not match trusted key"));
        }

        let verifier = UnparsedPublicKey::new(&ED25519, &self.public_key);
        if signature.prehashed {
            let hash = Blake2b::<U64>::digest(message);
            verifier
                .verify(&hash, &signature.signature)
                .map_err(|_| anyhow!("Minisign signature verification failed"))
        } else {
            verifier
                .verify(message, &signature.signature)
                .map_err(|_| anyhow!("Minisign signature verification failed"))
        }
    }
}

#[derive(Debug, Clone)]
pub struct MinisignSignature {
    prehashed: bool,
    key_id: [u8; 8],
    signature: [u8; 64],
}

impl MinisignSignature {
    /// Parses a minisign signature string.
    ///
    /// # Errors
    ///
    /// Returns an error if the signature is missing, malformed, or not Ed25519.
    pub fn parse(input: &str) -> Result<Self> {
        let line = extract_minisign_payload(input)?;
        let decoded = decode_base64_line(&line).context("Failed to decode minisign signature")?;

        if decoded.len() != MINISIGN_SIGNATURE_LEN {
            return Err(anyhow!(
                "Invalid minisign signature length: expected {MINISIGN_SIGNATURE_LEN} bytes"
            ));
        }
        let prehashed = if decoded[0..2] == MINISIGN_ALGO_ED25519 {
            false
        } else if decoded[0..2] == MINISIGN_ALGO_ED25519_HASHED {
            true
        } else {
            return Err(anyhow!("Unsupported minisign signature algorithm: expected Ed25519"));
        };

        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&decoded[2..10]);

        let mut signature = [0u8; 64];
        signature.copy_from_slice(&decoded[10..74]);

        Ok(Self { prehashed, key_id, signature })
    }

    pub fn key_id_hex(&self) -> String {
        key_id_hex(self.key_id)
    }
}

#[derive(Debug, Clone)]
pub struct MinisignVerifier {
    trusted_keys: Vec<MinisignPublicKey>,
}

impl MinisignVerifier {
    /// Builds a verifier from a list of minisign public key strings.
    ///
    /// # Errors
    ///
    /// Returns an error if any key is malformed or unsupported.
    pub fn from_trusted_pubkeys(keys: &[String]) -> Result<Self> {
        let mut trusted_keys = Vec::new();
        for key in keys {
            let trimmed = key.trim();
            if trimmed.is_empty() {
                continue;
            }
            trusted_keys.push(MinisignPublicKey::parse(trimmed)?);
        }
        Ok(Self { trusted_keys })
    }

    /// Verifies a minisign signature for the provided message.
    ///
    /// # Errors
    ///
    /// Returns an error if no trusted keys are configured or verification fails.
    pub fn verify(&self, message: &[u8], signature_text: &str) -> Result<VerifiedSignature> {
        if self.trusted_keys.is_empty() {
            return Err(anyhow!("No trusted minisign public keys configured"));
        }

        let signature = MinisignSignature::parse(signature_text)?;
        for key in &self.trusted_keys {
            if key.key_id == signature.key_id {
                key.verify(&signature, message)?;
                return Ok(VerifiedSignature { key_id: key.key_id_hex() });
            }
        }

        let key_id = signature.key_id_hex();
        Err(anyhow!("Signature key id {key_id} is not in the trusted key set"))
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedSignature {
    pub key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    #[serde(default = "default_registry_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub plugins: Vec<RegistryPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPlugin {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub latest: Option<String>,
    #[serde(default)]
    pub versions: Vec<RegistryPluginVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPluginVersion {
    pub version: String,
    pub manifest_url: String,
    pub signature_url: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    #[serde(default = "default_manifest_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: Option<String>,
    pub version: String,
    pub node_kind: String,
    pub kind: PluginKind,
    pub description: Option<String>,
    pub license: Option<String>,
    pub license_url: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub entrypoint: String,
    /// Marketplace bundle info.  Required for marketplace-distributed plugins;
    /// absent for local-only plugins that ship alongside their `.so`.
    #[serde(default)]
    pub bundle: Option<PluginBundle>,
    pub compatibility: Option<PluginCompatibility>,
    #[serde(default)]
    pub models: Vec<ModelSpec>,
    /// Asset types registered by this plugin.
    ///
    /// When a plugin declares asset types, the server creates generic CRUD
    /// endpoints under `/api/v1/assets/plugin/{type_id}` and includes them
    /// in the `GET /api/v1/asset-types` discovery response.
    #[serde(default)]
    pub assets: Vec<PluginAssetSpec>,
}

/// An asset type declared by a plugin in its manifest.
///
/// Each spec causes the server to register generic CRUD endpoints and
/// include the type in the asset-type discovery API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAssetSpec {
    /// URL-safe identifier, unique per plugin (e.g. `slint`).
    pub type_id: String,
    /// Human-readable label for the UI (e.g. "Slint Files").
    pub label: String,
    /// Allowed file extensions (e.g. `["slint"]`).
    pub extensions: Vec<String>,
    /// Maximum upload size in bytes (default: 1 MiB).
    #[serde(default = "default_max_asset_size")]
    pub max_size_bytes: usize,
    /// Whether the file content is text (editable) or binary.
    #[serde(default = "default_asset_content_type")]
    pub content_type: AssetContentType,
    /// UI icon hint (e.g. `code`, `music`, `image`, `type`, `file`).
    pub icon_hint: Option<String>,
    /// Which node parameter references this asset on drag-drop
    /// (e.g. `slint_file`).
    pub node_param: Option<String>,
    /// Directory (relative to server CWD) containing bundled system assets.
    /// User uploads go to a sibling `user/` directory derived from this path.
    pub system_dir: Option<String>,
}

/// Whether a plugin asset is text (editable) or binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AssetContentType {
    Text,
    Binary,
}

const fn default_max_asset_size() -> usize {
    1_048_576 // 1 MiB
}

const fn default_asset_content_type() -> AssetContentType {
    AssetContentType::Binary
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    Wasm,
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginBundle {
    pub url: String,
    pub sha256: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCompatibility {
    pub streamkit: Option<String>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub arch: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(flatten)]
    pub source: ModelSource,
    pub expected_size_bytes: Option<u64>,
    pub sha256: Option<String>,
    #[serde(default)]
    pub file_checksums: HashMap<String, String>,
    pub license: Option<String>,
    pub license_url: Option<String>,
    #[serde(default)]
    pub gated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum ModelSource {
    Huggingface { repo_id: String, revision: Option<String>, files: Vec<String> },
    Url { url: String },
}

#[derive(Debug, Clone)]
pub struct RegistryClient {
    http: reqwest::Client,
    cache: Arc<RwLock<RegistryCache>>,
    index_ttl: Duration,
    manifest_ttl: Duration,
}

impl RegistryClient {
    /// Creates a registry client with timeouts and cache TTLs.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be constructed.
    pub fn new(timeout: Duration, index_ttl: Duration, manifest_ttl: Duration) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("Failed to build registry HTTP client")?;
        Ok(Self {
            http,
            cache: Arc::new(RwLock::new(RegistryCache::default())),
            index_ttl,
            manifest_ttl,
        })
    }

    /// Fetches a registry index while validating redirect hops against policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry cannot be fetched, parsed, or violates URL policy.
    pub async fn fetch_index_with_policy(
        &self,
        url: &Url,
        policy: &MarketplaceUrlPolicy,
        registry_origin: &OriginKey,
    ) -> Result<RegistryIndex> {
        let url_str = url.as_str();
        if let Some(cached) = self.get_cached_index(url_str).await {
            return Ok(cached);
        }

        let bytes = validated_get_bytes(
            &self.http,
            policy,
            "registry index",
            url,
            Some(registry_origin),
            None,
        )
        .await?;
        let index: RegistryIndex = serde_json::from_slice(&bytes)
            .with_context(|| format!("Failed to parse registry index from {url_str}"))?;

        self.set_cached_index(url_str, index.clone()).await;
        Ok(index)
    }

    /// Fetches a plugin manifest while validating redirect hops against policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be fetched, parsed, or violates URL policy.
    pub async fn fetch_manifest_raw_with_policy(
        &self,
        url: &Url,
        policy: &MarketplaceUrlPolicy,
        registry_origin: &OriginKey,
    ) -> Result<ManifestRaw> {
        let entry = self.fetch_manifest_entry_with_policy(url, policy, registry_origin).await?;
        Ok(ManifestRaw { bytes: entry.raw, manifest: entry.manifest })
    }

    /// Fetches a text resource while validating redirect hops against policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched, parsed, or violates URL policy.
    pub async fn fetch_text_with_policy(
        &self,
        label: &str,
        url: &Url,
        policy: &MarketplaceUrlPolicy,
        registry_origin: &OriginKey,
    ) -> Result<String> {
        let bytes =
            validated_get_bytes(&self.http, policy, label, url, Some(registry_origin), None)
                .await?;
        std::str::from_utf8(&bytes)
            .with_context(|| format!("Response from {url} is not valid UTF-8"))
            .map(std::string::ToString::to_string)
    }

    async fn fetch_manifest_entry_with_policy(
        &self,
        url: &Url,
        policy: &MarketplaceUrlPolicy,
        registry_origin: &OriginKey,
    ) -> Result<ManifestCacheEntry> {
        let url_str = url.as_str();
        if let Some(cached) = self.get_cached_manifest(url_str).await {
            return Ok(cached);
        }

        let bytes = validated_get_bytes(
            &self.http,
            policy,
            "manifest url",
            url,
            Some(registry_origin),
            None,
        )
        .await?;
        let manifest: PluginManifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("Failed to parse plugin manifest from {url_str}"))?;
        let entry = ManifestCacheEntry { raw: bytes, manifest };

        self.set_cached_manifest(url_str, entry.clone()).await;
        Ok(entry)
    }

    async fn get_cached_index(&self, url: &str) -> Option<RegistryIndex> {
        let cache = self.cache.read().await;
        cache
            .indexes
            .get(url)
            .filter(|entry| entry.is_fresh(self.index_ttl))
            .map(|entry| entry.value.clone())
    }

    async fn set_cached_index(&self, url: &str, index: RegistryIndex) {
        let mut cache = self.cache.write().await;
        cache.indexes.insert(url.to_string(), Cached::new(index));
        cache.prune_indexes(self.index_ttl, MAX_INDEX_CACHE_ENTRIES);
    }

    async fn get_cached_manifest(&self, url: &str) -> Option<ManifestCacheEntry> {
        let cache = self.cache.read().await;
        cache
            .manifests
            .get(url)
            .filter(|entry| entry.is_fresh(self.manifest_ttl))
            .map(|entry| entry.value.clone())
    }

    async fn set_cached_manifest(&self, url: &str, manifest: ManifestCacheEntry) {
        let mut cache = self.cache.write().await;
        cache.manifests.insert(url.to_string(), Cached::new(manifest));
        cache.prune_manifests(self.manifest_ttl, MAX_MANIFEST_CACHE_ENTRIES);
    }
}

#[derive(Debug, Clone)]
pub struct ManifestRaw {
    pub bytes: Bytes,
    pub manifest: PluginManifest,
}

#[derive(Debug, Default)]
struct RegistryCache {
    indexes: HashMap<String, Cached<RegistryIndex>>,
    manifests: HashMap<String, Cached<ManifestCacheEntry>>,
}

impl RegistryCache {
    fn prune_indexes(&mut self, ttl: Duration, max_entries: usize) {
        Self::prune_map(&mut self.indexes, ttl, max_entries);
    }

    fn prune_manifests(&mut self, ttl: Duration, max_entries: usize) {
        Self::prune_map(&mut self.manifests, ttl, max_entries);
    }

    fn prune_map<T>(map: &mut HashMap<String, Cached<T>>, ttl: Duration, max_entries: usize) {
        map.retain(|_, entry| entry.is_fresh(ttl));
        while map.len() > max_entries {
            let oldest_key =
                map.iter().min_by_key(|(_, entry)| entry.fetched_at).map(|(key, _)| key.clone());
            let Some(oldest_key) = oldest_key else {
                break;
            };
            map.remove(&oldest_key);
        }
    }
}

#[derive(Debug, Clone)]
struct Cached<T> {
    value: T,
    fetched_at: Instant,
}

impl<T> Cached<T> {
    fn new(value: T) -> Self {
        Self { value, fetched_at: Instant::now() }
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() < ttl
    }
}

#[derive(Debug, Clone)]
struct ManifestCacheEntry {
    raw: Bytes,
    manifest: PluginManifest,
}

fn extract_minisign_payload(input: &str) -> Result<String> {
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("untrusted comment:") || trimmed.starts_with("trusted comment:") {
            continue;
        }
        return Ok(trimmed.to_string());
    }

    Err(anyhow!("Minisign payload line not found"))
}

fn decode_base64_line(line: &str) -> Result<Vec<u8>> {
    general_purpose::STANDARD
        .decode(line.as_bytes())
        .map_err(|err| anyhow!("Base64 decode failed: {err}"))
}

fn key_id_hex(key_id: [u8; 8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in key_id {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

const fn default_registry_schema_version() -> u32 {
    1
}

const fn default_manifest_schema_version() -> u32 {
    1
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // Test fixtures generated with minisign CLI
    // Key ID: 3E52143322870FFA (displayed as fa0f87223314523e in little-endian hex)
    const TEST_PUBLIC_KEY: &str = "\
untrusted comment: minisign public key 3E52143322870FFA
RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB";

    const TEST_PUBLIC_KEY_BASE64: &str = "RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB";

    // Second key for testing untrusted key scenarios
    // Key ID: 9DE0FE1340FC07FF
    const TEST_PUBLIC_KEY_2: &str = "\
untrusted comment: minisign public key 9DE0FE1340FC07FF
RWT/B/xAE/7gnfgr0vDarJzAmJSsI2ChTNLL0RrBhNOUb7TSpNQbWD7/";

    const TEST_MESSAGE: &[u8] = b"test message";

    // Signature for TEST_MESSAGE using TEST_PUBLIC_KEY (prehashed/ED)
    const TEST_SIGNATURE: &str = "\
untrusted comment: signature from minisign secret key
RUT6D4ciMxRSPupIJ/JuScXnkKUNfvxSkH9aWoJ/qkpqCnCocjUPC782vYGAjrPsGvwQIV/ZEJGz2RG2pK9NE5qXzsEEbJXBzQE=
trusted comment: timestamp:1769605868	file:test_ms.txt	hashed
Se0A1R+LfBuUD27evCFZ0ckKpR6P9j1Meebdk23uLFeqefFoBGjxEOodWnigTwiVxUcfZjksdyLTrPM5Cu/pCQ==";

    // Signature for TEST_MESSAGE using TEST_PUBLIC_KEY_2 (different key ID)
    const TEST_SIGNATURE_WRONG_KEY_ID: &str = "\
untrusted comment: signature from minisign secret key
RUT/B/xAE/7gnXUiMaqN9jw88kaGVmrdIy6QaYT4NKO6Q+0u7WYnxqo/UB84TsWw6KAoF5BhJLKdifcAkGGZa9KXvzci8/FOFA8=
trusted comment: timestamp:1769606000	file:test_ms.txt	hashed
P/pRiW89ReghkdC5ZJQuaVJtNy+NFYFkfNYG4d+X3z5C90iPub8+bD1Smu3euP+OijknBQudPhea/5w3QDp3BA==";

    // Expected key ID hex (bytes fa 0f 87 22 33 14 52 3e in order)
    const TEST_KEY_ID_HEX: &str = "fa0f87223314523e";

    // ==================== Public Key Parsing Tests ====================

    #[test]
    fn parse_valid_public_key() {
        let key = MinisignPublicKey::parse(TEST_PUBLIC_KEY).unwrap();
        assert_eq!(key.key_id_hex(), TEST_KEY_ID_HEX);
    }

    #[test]
    fn parse_public_key_base64_only() {
        // Public key without comment line
        let key = MinisignPublicKey::parse(TEST_PUBLIC_KEY_BASE64).unwrap();
        assert_eq!(key.key_id_hex(), TEST_KEY_ID_HEX);
    }

    #[test]
    fn parse_public_key_with_comments() {
        // Key with extra blank lines and the standard comment
        let key_with_whitespace = format!("\n\n{TEST_PUBLIC_KEY}\n\n");
        let key = MinisignPublicKey::parse(&key_with_whitespace).unwrap();
        assert_eq!(key.key_id_hex(), TEST_KEY_ID_HEX);
    }

    #[test]
    fn parse_public_key_rejects_wrong_length() {
        // Too short - missing bytes
        let short_key = "RWTAAA==";
        let err = MinisignPublicKey::parse(short_key).unwrap_err();
        assert!(err.to_string().contains("Invalid minisign public key length"));
    }

    #[test]
    fn parse_public_key_rejects_wrong_algorithm() {
        // Build a 42-byte key with wrong algorithm bytes (XX instead of Ed)
        // XX (2 bytes) + key_id (8 bytes) + pubkey (32 bytes) = 42 bytes
        let mut wrong_algo_bytes = vec![0u8; 42];
        wrong_algo_bytes[0] = b'X';
        wrong_algo_bytes[1] = b'X';
        let wrong_algo = general_purpose::STANDARD.encode(&wrong_algo_bytes);
        let err = MinisignPublicKey::parse(&wrong_algo).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported minisign public key algorithm"),
            "Expected algorithm error, got: {err}"
        );
    }

    #[test]
    fn parse_public_key_rejects_invalid_base64() {
        let invalid = "not valid base64!!!";
        let err = MinisignPublicKey::parse(invalid).unwrap_err();
        // Error is wrapped with context, check the full chain
        assert!(
            format!("{err:?}").contains("Base64 decode failed"),
            "Expected error to contain 'Base64 decode failed', got: {err:?}"
        );
    }

    #[test]
    fn parse_public_key_rejects_empty() {
        let err = MinisignPublicKey::parse("").unwrap_err();
        assert!(err.to_string().contains("payload line not found"));

        let err = MinisignPublicKey::parse("   \n   \n   ").unwrap_err();
        assert!(err.to_string().contains("payload line not found"));
    }

    #[test]
    fn parse_public_key_rejects_only_comments() {
        let only_comment = "untrusted comment: some key\n";
        let err = MinisignPublicKey::parse(only_comment).unwrap_err();
        assert!(err.to_string().contains("payload line not found"));
    }

    // ==================== Signature Parsing Tests ====================

    #[test]
    fn parse_valid_signature_hashed() {
        let sig = MinisignSignature::parse(TEST_SIGNATURE).unwrap();
        assert!(sig.prehashed);
        assert_eq!(sig.key_id_hex(), TEST_KEY_ID_HEX);
    }

    #[test]
    fn parse_valid_signature_unhashed() {
        // Manually construct an unhashed signature (Ed instead of ED)
        // Decode a valid signature, change algorithm bytes, re-encode
        let mut modified = general_purpose::STANDARD
            .decode("RUT6D4ciMxRSPupIJ/JuScXnkKUNfvxSkH9aWoJ/qkpqCnCocjUPC782vYGAjrPsGvwQIV/ZEJGz2RG2pK9NE5qXzsEEbJXBzQE=")
            .unwrap();
        // Change ED (0x45, 0x44) to Ed (0x45, 0x64)
        modified[1] = 0x64; // 'd' instead of 'D'
        let unhashed_base64 = general_purpose::STANDARD.encode(&modified);

        let sig = MinisignSignature::parse(&unhashed_base64).unwrap();
        assert!(!sig.prehashed);
        assert_eq!(sig.key_id_hex(), TEST_KEY_ID_HEX);
    }

    #[test]
    fn parse_signature_rejects_wrong_length() {
        // Too short
        let short_sig = "RWTAAA==";
        let err = MinisignSignature::parse(short_sig).unwrap_err();
        assert!(err.to_string().contains("Invalid minisign signature length"));
    }

    #[test]
    fn parse_signature_rejects_wrong_algorithm() {
        // Build a 74-byte signature with wrong algorithm bytes (XX instead of Ed/ED)
        // XX (2 bytes) + key_id (8 bytes) + signature (64 bytes) = 74 bytes
        let mut wrong_algo_bytes = vec![0u8; 74];
        wrong_algo_bytes[0] = b'X';
        wrong_algo_bytes[1] = b'X';
        let wrong_algo = general_purpose::STANDARD.encode(&wrong_algo_bytes);
        let err = MinisignSignature::parse(&wrong_algo).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported minisign signature algorithm"),
            "Expected algorithm error, got: {err}"
        );
    }

    #[test]
    fn parse_signature_rejects_invalid_base64() {
        let invalid = "!!!invalid base64!!!";
        let err = MinisignSignature::parse(invalid).unwrap_err();
        // Error is wrapped with context, check the full chain
        assert!(
            format!("{err:?}").contains("Base64 decode failed"),
            "Expected error to contain 'Base64 decode failed', got: {err:?}"
        );
    }

    #[test]
    fn parse_signature_rejects_empty() {
        let err = MinisignSignature::parse("").unwrap_err();
        assert!(err.to_string().contains("payload line not found"));
    }

    // ==================== Verifier Construction Tests ====================

    #[test]
    fn verifier_from_single_key() {
        let verifier =
            MinisignVerifier::from_trusted_pubkeys(&[TEST_PUBLIC_KEY.to_string()]).unwrap();
        assert_eq!(verifier.trusted_keys.len(), 1);
    }

    #[test]
    fn verifier_from_multiple_keys() {
        let verifier = MinisignVerifier::from_trusted_pubkeys(&[
            TEST_PUBLIC_KEY.to_string(),
            TEST_PUBLIC_KEY_2.to_string(),
        ])
        .unwrap();
        assert_eq!(verifier.trusted_keys.len(), 2);
    }

    #[test]
    fn verifier_skips_empty_strings() {
        let verifier = MinisignVerifier::from_trusted_pubkeys(&[
            String::new(),
            TEST_PUBLIC_KEY.to_string(),
            "   ".to_string(),
            "\n\n".to_string(),
        ])
        .unwrap();
        assert_eq!(verifier.trusted_keys.len(), 1);
    }

    #[test]
    fn verifier_rejects_malformed_key() {
        let err =
            MinisignVerifier::from_trusted_pubkeys(&["not a valid key".to_string()]).unwrap_err();
        // Error is wrapped with context, check the full chain
        assert!(
            format!("{err:?}").contains("Base64 decode failed"),
            "Expected error to contain 'Base64 decode failed', got: {err:?}"
        );
    }

    #[test]
    fn verifier_empty_keys_allowed() {
        // Empty key list is allowed at construction time (error at verify time)
        let verifier = MinisignVerifier::from_trusted_pubkeys(&[]).unwrap();
        assert_eq!(verifier.trusted_keys.len(), 0);
    }

    // ==================== Verification Tests ====================

    #[test]
    fn verify_valid_signature() {
        let verifier =
            MinisignVerifier::from_trusted_pubkeys(&[TEST_PUBLIC_KEY.to_string()]).unwrap();
        let result = verifier.verify(TEST_MESSAGE, TEST_SIGNATURE).unwrap();
        assert_eq!(result.key_id, TEST_KEY_ID_HEX);
    }

    #[test]
    fn verify_with_multiple_trusted_keys() {
        // Verifier with multiple keys should find the right one
        let verifier = MinisignVerifier::from_trusted_pubkeys(&[
            TEST_PUBLIC_KEY_2.to_string(),
            TEST_PUBLIC_KEY.to_string(),
        ])
        .unwrap();
        let result = verifier.verify(TEST_MESSAGE, TEST_SIGNATURE).unwrap();
        assert_eq!(result.key_id, TEST_KEY_ID_HEX);
    }

    #[test]
    fn verify_fails_wrong_message() {
        let verifier =
            MinisignVerifier::from_trusted_pubkeys(&[TEST_PUBLIC_KEY.to_string()]).unwrap();
        let err = verifier.verify(b"wrong message", TEST_SIGNATURE).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn verify_fails_untrusted_key_id() {
        // Use signature from key 2 but only trust key 1
        let verifier =
            MinisignVerifier::from_trusted_pubkeys(&[TEST_PUBLIC_KEY.to_string()]).unwrap();
        let err = verifier.verify(TEST_MESSAGE, TEST_SIGNATURE_WRONG_KEY_ID).unwrap_err();
        assert!(err.to_string().contains("not in the trusted key set"));
    }

    #[test]
    fn verify_fails_no_trusted_keys() {
        let verifier = MinisignVerifier::from_trusted_pubkeys(&[]).unwrap();
        let err = verifier.verify(TEST_MESSAGE, TEST_SIGNATURE).unwrap_err();
        assert!(err.to_string().contains("No trusted minisign public keys configured"));
    }

    #[test]
    fn verify_fails_corrupted_signature() {
        let verifier =
            MinisignVerifier::from_trusted_pubkeys(&[TEST_PUBLIC_KEY.to_string()]).unwrap();

        // Corrupt the signature by modifying a byte in the base64 payload
        let corrupted = TEST_SIGNATURE.replace(
            "RUT6D4ciMxRSPupIJ/JuScXnkKUNfvxSkH9aWoJ/qkpqCnCocjUPC782vYGAjrPsGvwQIV/ZEJGz2RG2pK9NE5qXzsEEbJXBzQE=",
            "RUT6D4ciMxRSPupIJ/JuScXnkKUNfvxSkH9aWoJ/qkpqCnCocjUPC782vYGAjrPsGvwQIV/ZEJGz2RG2pK9NE5qXzsEEbJXBzQA=",
        );

        let err = verifier.verify(TEST_MESSAGE, &corrupted).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    // ==================== Helper Function Tests ====================

    #[test]
    fn key_id_hex_formats_correctly() {
        let key_id: [u8; 8] = [0xfa, 0x0f, 0x87, 0x22, 0x33, 0x14, 0x52, 0x3e];
        assert_eq!(key_id_hex(key_id), "fa0f87223314523e");
    }

    #[test]
    fn key_id_hex_handles_leading_zeros() {
        let key_id: [u8; 8] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(key_id_hex(key_id), "0001020304050607");
    }

    #[test]
    fn extract_minisign_payload_skips_comments() {
        let input = "\
untrusted comment: some comment
RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB
trusted comment: another comment
abcd";
        let payload = extract_minisign_payload(input).unwrap();
        assert_eq!(payload, "RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB");
    }

    #[test]
    fn extract_minisign_payload_handles_whitespace() {
        let input = "   \n  \n  RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB  \n";
        let payload = extract_minisign_payload(input).unwrap();
        assert_eq!(payload, "RWT6D4ciMxRSPupBP+64kBYHS38aPGWasxvKW6sKjalBw93Ao3tQojyB");
    }

    #[test]
    fn decode_base64_line_valid() {
        let decoded = decode_base64_line("SGVsbG8=").unwrap();
        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn decode_base64_line_invalid() {
        let err = decode_base64_line("!!!").unwrap_err();
        assert!(err.to_string().contains("Base64 decode failed"));
    }

    // ==================== PluginManifest Deserialization Tests ====================

    /// Local plugin manifests (e.g. `plugins/native/slint/plugin.yml`) omit
    /// the `bundle` section because they aren't distributed via marketplace.
    /// Ensure deserialization succeeds with `bundle` absent.
    #[test]
    fn deserialize_manifest_without_bundle() {
        let yaml = r#"
schema_version: 1
id: slint
version: "0.1.0"
node_kind: "plugin::native::slint"
kind: native
entrypoint: libslint_plugin.so
assets:
  - type_id: slint
    label: "Slint Files"
    extensions: [slint]
    max_size_bytes: 1048576
    content_type: text
    icon_hint: code
    node_param: slint_file
    system_dir: samples/slint/system
"#;
        let manifest: PluginManifest = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(manifest.id, "slint");
        assert!(manifest.bundle.is_none());
        assert_eq!(manifest.assets.len(), 1);
        assert_eq!(manifest.assets[0].type_id, "slint");
        assert_eq!(manifest.assets[0].content_type, AssetContentType::Text);
    }

    /// Marketplace manifests include `bundle`; deserialization should populate it.
    #[test]
    fn deserialize_manifest_with_bundle() {
        let yaml = r#"
schema_version: 1
id: test-plugin
version: "1.0.0"
node_kind: "plugin::native::test"
kind: native
entrypoint: libtest.so
bundle:
  url: "https://example.com/bundle.tar.zst"
  sha256: "abc123"
"#;
        let manifest: PluginManifest = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        let bundle = manifest.bundle.unwrap();
        assert_eq!(bundle.url, "https://example.com/bundle.tar.zst");
        assert_eq!(bundle.sha256, "abc123");
        assert!(manifest.assets.is_empty());
    }
}
