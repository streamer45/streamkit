// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::Level;

use crate::permissions::PermissionsConfig;

const fn default_engine_batch_size() -> usize {
    32
}

/// Deserialize `Option<u64>` with a minimum clamp of 1 for timeout values.
fn deserialize_clamp_timeout<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val: Option<u64> = Option::deserialize(deserializer)?;
    Ok(val.map(|v| v.max(1)))
}

/// Preset tuning profiles for the engine.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EnginePerfProfile {
    /// Low-latency real-time streaming (minimal buffering, more backpressure)
    LowLatency,
    /// Balanced defaults for general streaming and interactive pipelines
    Balanced,
    /// High-throughput / batch processing (more buffering, higher latency)
    HighThroughput,
}

impl EnginePerfProfile {
    const fn node_input_capacity(self) -> usize {
        match self {
            Self::LowLatency => 8,
            Self::Balanced => 32,
            Self::HighThroughput => 128,
        }
    }

    const fn pin_distributor_capacity(self) -> usize {
        match self {
            Self::LowLatency => 4,
            Self::Balanced => 16,
            Self::HighThroughput => 64,
        }
    }
}

/// Engine configuration for packet processing and buffering.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct EngineConfig {
    /// Optional tuning profile that provides sensible buffering defaults.
    ///
    /// Explicit values for `node_input_capacity` and/or `pin_distributor_capacity` take precedence.
    #[serde(default)]
    pub profile: Option<EnginePerfProfile>,
    /// Batch size for processing packets in nodes (default: 32)
    /// Lower values = more responsive to control messages, higher values = better throughput
    #[serde(default = "default_engine_batch_size")]
    pub packet_batch_size: usize,
    /// Buffer size for node input channels (default: 128 packets)
    /// Higher = more buffering/latency, lower = more backpressure/responsiveness
    /// For low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame)
    pub node_input_capacity: Option<usize>,
    /// Buffer size between node output and pin distributor (default: 64 packets)
    /// For low-latency streaming, consider 4-8 packets
    pub pin_distributor_capacity: Option<usize>,
    /// Configuration for oneshot (HTTP batch) pipelines.
    #[serde(default)]
    pub oneshot: OneshotConfig,
    /// Advanced buffer tuning for codec and container nodes.
    /// Only modify if you understand the latency/throughput implications.
    #[serde(default)]
    pub advanced: AdvancedBufferConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            profile: None,
            packet_batch_size: default_engine_batch_size(),
            node_input_capacity: None,
            pin_distributor_capacity: None,
            oneshot: OneshotConfig::default(),
            advanced: AdvancedBufferConfig::default(),
        }
    }
}

impl EngineConfig {
    pub(crate) fn resolved_node_input_capacity(&self) -> Option<usize> {
        self.node_input_capacity
            .or_else(|| self.profile.map(EnginePerfProfile::node_input_capacity))
    }

    pub(crate) fn resolved_pin_distributor_capacity(&self) -> Option<usize> {
        self.pin_distributor_capacity
            .or_else(|| self.profile.map(EnginePerfProfile::pin_distributor_capacity))
    }
}

/// Oneshot pipeline configuration (HTTP batch processing).
///
/// These settings apply to stateless pipelines executed via the `/api/v1/process` endpoint.
/// Oneshot pipelines use larger buffers by default than dynamic sessions because they
/// don't require tight backpressure coordination.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct OneshotConfig {
    /// Batch size for processing packets in oneshot pipelines (default: 32)
    /// Lower values = more responsive, higher values = better throughput
    #[serde(default = "default_engine_batch_size")]
    pub packet_batch_size: usize,

    /// Buffer size for media channels between nodes (default: 256 packets)
    /// Oneshot uses larger buffers than dynamic for batch efficiency.
    pub media_channel_capacity: Option<usize>,

    /// Buffer size for I/O stream channels (default: 16)
    /// Used for HTTP input/output streaming.
    pub io_channel_capacity: Option<usize>,
}

impl Default for OneshotConfig {
    fn default() -> Self {
        Self {
            packet_batch_size: default_engine_batch_size(),
            media_channel_capacity: None, // Uses DEFAULT_ONESHOT_MEDIA_CAPACITY (256)
            io_channel_capacity: None,    // Uses DEFAULT_ONESHOT_IO_CAPACITY (16)
        }
    }
}

/// Advanced internal buffer configuration for power users.
///
/// These settings affect async/blocking handoff channels in codec and container nodes.
/// Most users should not need to modify these values. Only adjust if you understand
/// the latency/throughput tradeoffs and have specific performance requirements.
///
/// All values are in packets (not bytes). The actual memory footprint depends on packet size.
#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
pub struct AdvancedBufferConfig {
    /// Capacity for codec processing channels (opus, flac, mp3) (default: 32)
    /// Used for async/blocking handoff in codec nodes.
    pub codec_channel_capacity: Option<usize>,

    /// Capacity for streaming reader channels (container demuxers) (default: 8)
    /// Smaller than codec channels because container frames may be larger.
    pub stream_channel_capacity: Option<usize>,

    /// Duplex buffer size for ogg demuxer in bytes (default: 65536)
    pub demuxer_buffer_size: Option<usize>,

    /// MoQ transport peer channel capacity (default: 100)
    /// Used for network send/receive coordination in MoQ transport nodes.
    pub moq_peer_channel_capacity: Option<usize>,
}

/// Log level for filtering messages.
#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Info => Self::INFO,
            LogLevel::Warn => Self::WARN,
            LogLevel::Error => Self::ERROR,
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_max_body_size() -> usize {
    // Default to 100MB for multipart uploads (oneshot media, plugins, assets)
    100 * 1024 * 1024
}

const fn default_native_call_timeout_value() -> u64 {
    300
}

fn default_cors_allowed_origins() -> Vec<String> {
    vec![
        // Portless localhost (e.g., reverse proxy on 80/443)
        "http://localhost".to_string(),
        "https://localhost".to_string(),
        "http://localhost:*".to_string(),
        "https://localhost:*".to_string(),
        // Portless 127.0.0.1 (e.g., reverse proxy on 80/443)
        "http://127.0.0.1".to_string(),
        "https://127.0.0.1".to_string(),
        "http://127.0.0.1:*".to_string(),
        "https://127.0.0.1:*".to_string(),
    ]
}

/// CORS configuration for cross-origin requests.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct CorsConfig {
    /// Allowed origins for CORS requests.
    /// Supports wildcards: "http://localhost:*" matches any port on localhost.
    /// Default: localhost and 127.0.0.1 on any port (HTTP and HTTPS).
    /// Set to `["*"]` to allow all origins (not recommended for production).
    #[serde(default = "default_cors_allowed_origins")]
    pub allowed_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self { allowed_origins: default_cors_allowed_origins() }
    }
}

fn default_label_fallback() -> String {
    "other".to_string()
}

/// A bounded metric label sourced from a trusted request header.
///
/// The header value is trimmed and lowercased, then matched against `allowed`;
/// anything not in the allowlist (or a missing header) collapses to `fallback`,
/// so client-supplied headers can never inflate metric cardinality.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct RequestLabelConfig {
    /// Metric label key (e.g. `service`).
    pub name: String,
    /// Trusted request header to read the value from (e.g. `X-StreamKit-Service`).
    ///
    /// Read before auth middleware runs, so point this at a header set by a
    /// trusted upstream (e.g. the gateway, which strips client-supplied copies)
    /// — not at an auth-injected header such as `X-StreamKit-Role`, whose
    /// pre-auth value is client-controlled.
    pub header: String,
    /// Permitted values, matched case-insensitively after trimming.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Value emitted when the header is absent or its value is not in `allowed`.
    #[serde(default = "default_label_fallback")]
    pub fallback: String,
}

/// Configuration for request-scoped metric labeling.
///
/// Empty by default: no request metric gains a configured label unless an
/// operator opts in. Declaring `request_labels` sets the full list (figment
/// does not merge sequences). See the commented example in `samples/skit.toml`.
#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
pub struct MetricsConfig {
    /// Bounded labels attached to request metrics, each sourced from a trusted
    /// request header. Applied to all HTTP request metrics and to oneshot
    /// pipeline metrics.
    #[serde(default)]
    pub request_labels: Vec<RequestLabelConfig>,
}

/// Prometheus sanitizes any character outside `[a-zA-Z0-9_]` in a label key to
/// `_`, so `http.method` and `http_method` collapse to the same series key. We
/// compare sanitized keys to catch collisions that only appear after scrape.
fn sanitize_label_key(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// A metric label name must be a non-empty identifier (dots allowed, per the
/// OpenTelemetry convention used by the built-in keys) so it survives export.
fn is_valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {},
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

impl MetricsConfig {
    /// Normalize then validate — the single chokepoint that readies a metrics
    /// config for use. Callers decide how to treat the error: `load()` rejects
    /// the file, `create_app_state()` warns and disables the labels.
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails after normalization.
    pub fn prepare(&mut self) -> Result<(), String> {
        self.normalize();
        self.validate()
    }

    /// Lowercase and trim every allowlist entry and each `fallback` so the
    /// per-request hot path only has to normalize the incoming header value and
    /// every emitted value shares one normalized space.
    pub fn normalize(&mut self) {
        for label in &mut self.request_labels {
            for allowed in &mut label.allowed {
                *allowed = crate::metrics_labels::normalize(allowed);
            }
            label.fallback = crate::metrics_labels::normalize(&label.fallback);
        }
    }

    /// Reject label configs that would silently corrupt metrics: invalid names,
    /// names colliding (after Prometheus sanitization) with a built-in key or
    /// each other, invalid/empty headers, and empty allowlist or fallback values.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first offending label.
    pub fn validate(&self) -> Result<(), String> {
        let reserved: std::collections::HashSet<String> =
            crate::metrics_labels::RESERVED_LABEL_KEYS
                .iter()
                .map(|k| sanitize_label_key(k))
                .collect();
        let mut seen = std::collections::HashSet::new();
        for label in &self.request_labels {
            if !is_valid_label_name(&label.name) {
                return Err(format!(
                    "metrics request_label name '{}' is not a valid metric label name",
                    label.name
                ));
            }
            let key = sanitize_label_key(&label.name);
            if reserved.contains(&key) {
                return Err(format!(
                    "metrics request_label name '{}' collides with a built-in metric key",
                    label.name
                ));
            }
            if !seen.insert(key) {
                return Err(format!("duplicate metrics request_label name '{}'", label.name));
            }
            if axum::http::HeaderName::try_from(label.header.as_str()).is_err() {
                return Err(format!(
                    "metrics request_label '{}' has an invalid header '{}'",
                    label.name, label.header
                ));
            }
            if label.allowed.iter().any(|v| v.trim().is_empty()) {
                return Err(format!(
                    "metrics request_label '{}' has an empty allowed value",
                    label.name
                ));
            }
            if label.fallback.trim().is_empty() {
                return Err(format!(
                    "metrics request_label '{}' has an empty fallback value",
                    label.name
                ));
            }
        }
        Ok(())
    }
}

/// Telemetry and observability configuration (OpenTelemetry, tokio-console).
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Enable OpenTelemetry tracing (spans) export.
    ///
    /// Metrics export is controlled separately via `otlp_endpoint`.
    #[serde(default)]
    pub tracing_enable: bool,
    pub otlp_endpoint: Option<String>,
    /// OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`).
    pub otlp_traces_endpoint: Option<String>,
    #[serde(default)]
    pub otlp_headers: HashMap<String, String>,
    #[serde(default)]
    pub tokio_console: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enable: true,
            tracing_enable: false,
            otlp_endpoint: None,
            otlp_traces_endpoint: None,
            otlp_headers: HashMap::new(),
            tokio_console: false,
        }
    }
}

/// Log file format options.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Plain text format (faster, lower CPU overhead)
    #[default]
    Text,
    /// JSON format (structured, better for log aggregation but ~2-3x slower)
    Json,
}

/// Logging configuration for console and file output.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct LogConfig {
    #[serde(default)]
    pub console_enable: bool,
    #[serde(default)]
    pub file_enable: bool,
    #[serde(default)]
    pub console_level: LogLevel,
    #[serde(default)]
    pub file_level: LogLevel,
    #[serde(default)]
    pub file_path: String,
    /// Format for file logging: "text" (default, faster) or "json" (structured)
    #[serde(default)]
    pub file_format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            console_enable: true,
            file_enable: true,
            console_level: LogLevel::default(),
            file_level: LogLevel::Info, // Debug level has significant CPU overhead
            file_path: "./skit.log".to_string(),
            file_format: LogFormat::default(),
        }
    }
}

/// HTTP server configuration including TLS and CORS settings.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct ServerConfig {
    pub address: String,
    pub tls: bool,
    pub cert_path: String,
    pub key_path: String,
    pub samples_dir: String,
    /// Maximum request body size in bytes for multipart uploads (default: 100MB)
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
    /// Base path for subpath deployments (e.g., "/s/session_xxx"). Used to inject <base> tag in HTML.
    /// If None, no <base> tag is injected (root deployment).
    pub base_path: Option<String>,
    /// CORS configuration for cross-origin requests
    #[serde(default)]
    pub cors: CorsConfig,
    /// Bounded request-metric labeling configuration.
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[cfg(feature = "moq")]
    pub moq_address: Option<String>,
    /// TLS certificate for the MoQ WebTransport listener.
    /// When set, the MoQ QUIC server uses these certs independently of `[server].tls`.
    /// When unset, falls back to `cert_path`/`key_path` (if `tls = true`) or self-signed.
    #[cfg(feature = "moq")]
    #[serde(default)]
    pub moq_cert_path: Option<String>,
    /// TLS private key for the MoQ WebTransport listener (see `moq_cert_path`).
    #[cfg(feature = "moq")]
    #[serde(default)]
    pub moq_key_path: Option<String>,
    /// MoQ Gateway URL to use in the frontend (can be overridden via SK_SERVER__MOQ_GATEWAY_URL)
    #[cfg(feature = "moq")]
    pub moq_gateway_url: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:4545".to_string(),
            tls: false,
            cert_path: String::new(),
            key_path: String::new(),
            samples_dir: "./samples/pipelines".to_string(),
            max_body_size: default_max_body_size(),
            base_path: None,
            cors: CorsConfig::default(),
            metrics: MetricsConfig::default(),
            #[cfg(feature = "moq")]
            moq_address: None,
            #[cfg(feature = "moq")]
            moq_cert_path: None,
            #[cfg(feature = "moq")]
            moq_key_path: None,
            #[cfg(feature = "moq")]
            moq_gateway_url: None,
        }
    }
}

/// Plugin directory configuration.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct PluginConfig {
    pub directory: String,
    /// Native plugin FFI call timeout in seconds (default: 300, minimum: 1).
    ///
    /// Set to `null` to use only the default backstop timeout on the reply
    /// side; the send-side backpressure guard remains bounded regardless.
    ///
    /// Values below 1 are clamped to 1 to prevent instant timeouts.
    #[serde(
        default = "PluginConfig::default_native_call_timeout_secs",
        deserialize_with = "deserialize_clamp_timeout"
    )]
    pub native_call_timeout_secs: Option<u64>,
    #[serde(flatten, default)]
    pub http_management: PluginHttpConfig,
    #[serde(flatten, default)]
    pub marketplace: PluginMarketplaceConfig,
    /// Minisign public keys (contents of `.pub` files) trusted for marketplace manifests.
    #[serde(default)]
    pub trusted_pubkeys: Vec<String>,
    /// Registry index URLs (e.g., `https://example.com/index.json`).
    #[serde(default)]
    pub registries: Vec<String>,
    /// Optional directory to store downloaded models (defaults to `models` when unset).
    #[serde(default)]
    pub models_dir: Option<String>,
    /// Optional Hugging Face token for gated model downloads.
    #[serde(default)]
    pub huggingface_token: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
pub struct PluginHttpConfig {
    /// Controls whether runtime plugin upload/delete is allowed via the public APIs.
    ///
    /// Default is false to avoid accidental exposure when running without an auth layer.
    #[serde(default)]
    pub allow_http_management: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
pub struct PluginMarketplaceConfig {
    /// Enables the plugin marketplace API and UI (default: false).
    #[serde(default)]
    pub marketplace_enabled: bool,
    /// Allows native plugins to be installed from a marketplace (default: false).
    ///
    /// Native plugins run in-process and are unsafe without full trust.
    #[serde(default)]
    pub allow_native_marketplace: bool,
    #[serde(flatten, default)]
    pub security: PluginMarketplaceSecurityConfig,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceSchemePolicy {
    #[default]
    HttpsOnly,
    AllowHttp,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceHostPolicy {
    #[default]
    PublicOnly,
    AllowPrivate,
}

#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct PluginMarketplaceSecurityConfig {
    /// Allow direct URL model downloads from manifests (default: false).
    #[serde(default)]
    pub allow_model_urls: bool,
    /// Require marketplace URLs to share origin with the registry (default: false).
    #[serde(default = "default_require_registry_origin")]
    pub marketplace_require_registry_origin: bool,
    /// Scheme policy for marketplace URLs (default: https_only).
    #[serde(default)]
    pub marketplace_scheme_policy: MarketplaceSchemePolicy,
    /// Host policy for marketplace URLs (default: public_only).
    #[serde(default)]
    pub marketplace_host_policy: MarketplaceHostPolicy,
    /// Resolve hostnames for marketplace URLs and check resolved IPs (default: false).
    #[serde(default)]
    pub marketplace_resolve_hostnames: bool,
    /// Allowed marketplace origins (e.g., "https://example.com", "https://example.com:*").
    #[serde(default)]
    pub marketplace_url_allowlist: Vec<String>,
}

impl Default for PluginMarketplaceSecurityConfig {
    fn default() -> Self {
        Self {
            allow_model_urls: false,
            marketplace_require_registry_origin: default_require_registry_origin(),
            marketplace_scheme_policy: MarketplaceSchemePolicy::default(),
            marketplace_host_policy: MarketplaceHostPolicy::default(),
            marketplace_resolve_hostnames: false,
            marketplace_url_allowlist: Vec::new(),
        }
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            directory: ".plugins".to_string(),
            native_call_timeout_secs: Some(default_native_call_timeout_value()),
            http_management: PluginHttpConfig::default(),
            marketplace: PluginMarketplaceConfig::default(),
            trusted_pubkeys: Vec::new(),
            registries: Vec::new(),
            models_dir: None,
            huggingface_token: None,
        }
    }
}

impl PluginConfig {
    // Serde default hooks must return the exact field type; the wrapped value
    // distinguishes missing config from explicit null.
    #[allow(clippy::unnecessary_wraps)]
    const fn default_native_call_timeout_secs() -> Option<u64> {
        Some(default_native_call_timeout_value())
    }
}

const fn default_require_registry_origin() -> bool {
    false
}

const fn default_keep_models_loaded() -> bool {
    true
}

/// Configuration for a single plugin to pre-warm at startup.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct PrewarmPluginConfig {
    /// Plugin kind (e.g., "plugin::native::kokoro", "plugin::native::whisper")
    pub kind: String,

    /// Parameters to use when creating the warmup instance
    /// These should match the most common usage pattern
    #[serde(default)]
    pub params: Option<serde_json::Value>,

    /// Optional fallback parameters to try if the primary params fail
    /// Useful for GPU plugins that should fallback to CPU
    #[serde(default)]
    pub fallback_params: Option<serde_json::Value>,
}

/// Configuration for pre-warming plugins at startup.
#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
pub struct PrewarmConfig {
    /// Enable pre-warming (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// List of plugins to pre-warm with their parameters
    #[serde(default)]
    pub plugins: Vec<PrewarmPluginConfig>,
}

/// Resource management configuration for ML models and shared resources.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct ResourceConfig {
    /// Keep loaded resources (models) in memory until explicit unload (default: true).
    /// When false, resources may be evicted based on LRU policy if max_memory_mb is set.
    #[serde(default = "default_keep_models_loaded")]
    pub keep_models_loaded: bool,

    /// Optional memory limit in megabytes for cached resources (models).
    /// When set, least-recently-used resources will be evicted to stay under the limit.
    /// Only applies when keep_models_loaded is false.
    pub max_memory_mb: Option<usize>,

    /// Pre-warming configuration for reducing first-use latency
    #[serde(default)]
    pub prewarm: PrewarmConfig,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self { keep_models_loaded: true, max_memory_mb: None, prewarm: PrewarmConfig::default() }
    }
}

/// URL allowlist rule for fetch() API in script nodes.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct AllowlistRule {
    /// URL pattern with wildcards (e.g., "https://api.example.com/*")
    pub url: String,
    /// Allowed HTTP methods
    pub methods: Vec<String>,
}

/// Type of secret for validation and documentation.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SecretType {
    /// URL (e.g., webhook URLs)
    Url,
    /// Bearer token
    Token,
    /// API key
    ApiKey,
    /// Generic string
    String,
}

const fn default_secret_type() -> SecretType {
    SecretType::String
}

/// Configuration for a single secret loaded from environment.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct SecretConfig {
    /// Environment variable name containing the secret value
    pub env: String,

    /// Type of secret (for validation and formatting)
    #[serde(default = "default_secret_type")]
    #[serde(rename = "type")]
    pub secret_type: SecretType,

    /// Optional allowlist of URL patterns where this secret may be injected into `fetch()` headers.
    ///
    /// Patterns use the same format as `script.global_fetch_allowlist` entries:
    /// - `https://api.openai.com/*`
    /// - `https://api.openai.com/v1/chat/completions`
    ///
    /// Empty = no additional restriction (backwards-compatible).
    #[serde(default)]
    pub allowed_fetch_urls: Vec<String>,

    /// Optional description for documentation
    #[serde(default)]
    pub description: String,
}

const fn default_script_timeout_ms() -> u64 {
    100
}

const fn default_script_memory_limit_mb() -> usize {
    64
}

/// Configuration for the core::script node.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct ScriptConfig {
    /// Default timeout for script execution per packet (in milliseconds)
    #[serde(default = "default_script_timeout_ms")]
    pub default_timeout_ms: u64,

    /// Default memory limit for QuickJS runtime (in megabytes)
    #[serde(default = "default_script_memory_limit_mb")]
    pub default_memory_limit_mb: usize,

    /// Global fetch allowlist (empty = block all fetch() calls)
    /// Applies to all script nodes.
    ///
    /// Security note: there is no per-pipeline allowlist override; this prevents bypass via
    /// user-provided pipelines.
    #[serde(default)]
    pub global_fetch_allowlist: Vec<AllowlistRule>,

    /// Available secrets (name → environment variable mapping)
    /// Empty map = no secrets available to any script node
    /// Secrets are loaded from environment variables at server startup
    /// and can be injected into HTTP headers via pipeline configuration
    #[serde(default)]
    pub secrets: HashMap<String, SecretConfig>,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: default_script_timeout_ms(),
            default_memory_limit_mb: default_script_memory_limit_mb(),
            global_fetch_allowlist: Vec::new(),
            secrets: HashMap::new(),
        }
    }
}

const fn default_compositor_max_canvas_dimension() -> u32 {
    7680
}

const fn default_compositor_max_image_dimension() -> u32 {
    7680
}

const fn default_compositor_max_font_size() -> u32 {
    4096
}

const fn default_compositor_max_text_length() -> usize {
    10_000
}

// Backward-compat aliases: the TOML keys were renamed from
// `default_max_canvas_dimension` → `max_canvas_dimension` (and similarly
// for font size).  The `alias` attribute lets old config files keep working.

/// Server-level defaults for the video compositor node.
///
/// These limits apply to every compositor node created by the engine.
/// Individual nodes cannot exceed these values, even via `UpdateParams`.
///
/// ```toml
/// [compositor]
/// max_canvas_dimension = 7680
/// max_font_size = 4096
/// max_text_length = 10000
/// ```
// All fields are upper-bound limits — the shared `max_` prefix is intentional
// and maps directly to the TOML key names.
#[allow(clippy::struct_field_names)]
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct CompositorServerConfig {
    /// Maximum allowed canvas dimension (width or height) in pixels.
    /// Default: 7680 (8K UHD).
    #[serde(
        default = "default_compositor_max_canvas_dimension",
        alias = "default_max_canvas_dimension"
    )]
    pub max_canvas_dimension: u32,

    /// Maximum allowed font size for text overlays in pixels.
    /// Default: 4096.
    #[serde(default = "default_compositor_max_font_size", alias = "default_max_font_size")]
    pub max_font_size: u32,

    /// Maximum allowed text overlay string length in bytes.
    /// Default: 10000.
    #[serde(default = "default_compositor_max_text_length")]
    pub max_text_length: usize,

    /// Maximum allowed image overlay dimension (width or height) in pixels.
    /// Uploads exceeding this limit are rejected before full decode to prevent
    /// decompression bombs.  Default: 7680.
    #[serde(default = "default_compositor_max_image_dimension")]
    pub max_image_dimension: u32,
}

impl Default for CompositorServerConfig {
    fn default() -> Self {
        Self {
            max_canvas_dimension: default_compositor_max_canvas_dimension(),
            max_font_size: default_compositor_max_font_size(),
            max_text_length: default_compositor_max_text_length(),
            max_image_dimension: default_compositor_max_image_dimension(),
        }
    }
}

/// Authentication mode for the server.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Auto: disabled on loopback, enabled on non-loopback
    #[default]
    Auto,
    /// Always require authentication
    Enabled,
    /// Disable authentication entirely (NOT recommended for production)
    Disabled,
}

fn default_auth_state_dir() -> String {
    ".streamkit/auth".to_string()
}

fn default_auth_cookie_name() -> String {
    "skit_session".to_string()
}

const fn default_api_default_ttl() -> u64 {
    86400 // 24 hours
}

const fn default_api_max_ttl() -> u64 {
    2_592_000 // 30 days
}

const fn default_moq_default_ttl() -> u64 {
    3600 // 1 hour
}

const fn default_moq_max_ttl() -> u64 {
    86400 // 1 day
}

/// Authentication configuration for built-in JWT-based auth.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct AuthConfig {
    /// Authentication mode (auto, enabled, disabled)
    #[serde(default)]
    pub mode: AuthMode,

    /// Directory for auth state (keys, tokens). Default: ".streamkit/auth"
    #[serde(default = "default_auth_state_dir")]
    pub state_dir: String,

    /// Cookie name for browser sessions. Default: "skit_session"
    #[serde(default = "default_auth_cookie_name")]
    pub cookie_name: String,

    /// Default TTL for API tokens in seconds. Default: 86400 (24 hours)
    #[serde(default = "default_api_default_ttl")]
    pub api_default_ttl_secs: u64,

    /// Maximum TTL for API tokens in seconds. Default: 2592000 (30 days)
    #[serde(default = "default_api_max_ttl")]
    pub api_max_ttl_secs: u64,

    /// Default TTL for MoQ tokens in seconds. Default: 3600 (1 hour)
    #[serde(default = "default_moq_default_ttl")]
    pub moq_default_ttl_secs: u64,

    /// Maximum TTL for MoQ tokens in seconds. Default: 86400 (1 day)
    #[serde(default = "default_moq_max_ttl")]
    pub moq_max_ttl_secs: u64,

    /// Gateway paths that allow unauthenticated MoQ WebTransport connections.
    /// Connections to listed path prefixes skip JWT validation; the HTTP API remains protected.
    /// Example: `["/moq"]` makes all `/moq/**` paths public; `["/moq/abc123"]` for a single path.
    /// Empty list (default) = all MoQ connections require auth.
    #[serde(default)]
    pub moq_public_paths: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::default(),
            state_dir: default_auth_state_dir(),
            cookie_name: default_auth_cookie_name(),
            api_default_ttl_secs: default_api_default_ttl(),
            api_max_ttl_secs: default_api_max_ttl(),
            moq_default_ttl_secs: default_moq_default_ttl(),
            moq_max_ttl_secs: default_moq_max_ttl(),
            moq_public_paths: Vec::new(),
        }
    }
}

fn default_allowed_file_paths() -> Vec<String> {
    vec!["samples/**".to_string()]
}

const fn default_allowed_write_paths() -> Vec<String> {
    Vec::new()
}

/// Security configuration for file access and other security-sensitive settings.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct SecurityConfig {
    /// Allowed file paths for file_reader nodes.
    /// Supports glob patterns (e.g., "samples/**", "/data/media/*").
    /// Relative paths are resolved against the server's working directory.
    /// Default: `["samples/**"]` - only allow reading from the samples directory.
    /// Set to `["**"]` to allow all paths (not recommended for production).
    #[serde(default = "default_allowed_file_paths")]
    pub allowed_file_paths: Vec<String>,

    /// Allowed file paths for file_writer nodes.
    ///
    /// Default: empty (deny all writes). This is intentional: arbitrary file writes from
    /// user-provided pipelines are a high-risk capability.
    ///
    /// Patterns follow the same rules as `allowed_file_paths` and are matched against the
    /// resolved absolute target path.
    #[serde(default = "default_allowed_write_paths")]
    pub allowed_write_paths: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allowed_file_paths: default_allowed_file_paths(),
            allowed_write_paths: default_allowed_write_paths(),
        }
    }
}

fn default_mcp_endpoint() -> String {
    "/api/v1/mcp".to_string()
}

/// MCP (Model Context Protocol) server configuration.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct McpConfig {
    /// Enable the embedded MCP endpoint (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Streamable HTTP endpoint path (default: "/api/v1/mcp").
    #[serde(default = "default_mcp_endpoint")]
    pub endpoint: String,
    /// Hostnames accepted by the MCP transport's `Host` header check
    /// (DNS rebinding protection).
    ///
    /// When empty (default), the check is disabled — acceptable when the
    /// endpoint sits behind `auth_guard_middleware` and
    /// `origin_guard_middleware`.  For deployments exposed to untrusted
    /// networks, set this to the public hostname(s) of the server.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self { enabled: false, endpoint: default_mcp_endpoint(), allowed_hosts: Vec::new() }
    }
}

impl McpConfig {
    /// Validate MCP configuration.
    ///
    /// The endpoint MUST live under `/api/` so that `auth_guard_middleware`,
    /// `origin_guard_middleware`, CORS, tracing, and metrics all apply.
    /// It must NOT start with `/api/v1/auth/` because that prefix is
    /// short-circuited by the auth guard.  Only paths matching
    /// `/api/v<digits>/mcp` (with optional trailing subpath) are accepted.
    ///
    /// # Errors
    ///
    /// Returns an error string describing the misconfiguration.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        let ep = &self.endpoint;

        // Must start with /api/
        if !ep.starts_with("/api/") {
            return Err(format!(
                "mcp.endpoint must start with /api/ to ensure auth and origin guards apply. Got: '{ep}'"
            ));
        }

        // Must not sit under the auth prefix (auth_guard_middleware short-circuits it)
        if ep.starts_with("/api/v1/auth/") || ep == "/api/v1/auth" {
            return Err(format!(
                "mcp.endpoint must not start with /api/v1/auth/ (bypasses auth guard). Got: '{ep}'"
            ));
        }

        // Reject unsafe path segments (e.g. ".." which could escape the
        // intended mount point).  This is conservative — it also rejects
        // legitimate paths like "/api/v1/mcp/foo..bar" — but mount paths
        // should never need such patterns.
        if ep.contains("..") {
            return Err(format!("mcp.endpoint must not contain unsafe path segments (..): '{ep}'"));
        }

        // Must match /api/v<digits>/mcp or /api/v<digits>/mcp/...
        let parts: Vec<&str> = ep.trim_start_matches('/').split('/').collect();
        if parts.len() < 3 {
            return Err(format!("mcp.endpoint must be at least /api/v<N>/mcp. Got: '{ep}'"));
        }
        // parts[0] = "api", parts[1] = "v<digits>", parts[2] = "mcp"
        let version_part = parts[1];
        if !version_part.starts_with('v')
            || !version_part[1..].chars().all(|c| c.is_ascii_digit())
            || version_part.len() < 2
        {
            return Err(format!(
                "mcp.endpoint version segment must be v<digits> (e.g. v1). Got: '{version_part}' in '{ep}'"
            ));
        }
        if parts[2] != "mcp" {
            return Err(format!(
                "mcp.endpoint third segment must be 'mcp'. Got: '{}' in '{ep}'",
                parts[2]
            ));
        }

        Ok(())
    }
}

/// Root configuration for the StreamKit server.
#[derive(Deserialize, Serialize, Default, Debug, Clone, JsonSchema)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub log: LogConfig,

    #[serde(default)]
    pub telemetry: TelemetryConfig,

    #[serde(default)]
    pub engine: EngineConfig,

    #[serde(default)]
    pub plugins: PluginConfig,

    #[serde(default)]
    pub resources: ResourceConfig,

    #[serde(default)]
    pub permissions: PermissionsConfig,

    #[serde(default)]
    pub script: ScriptConfig,

    #[serde(default)]
    pub compositor: CompositorServerConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub mcp: McpConfig,

    /// Root directory for sample assets (`samples/audio`, `samples/images`,
    /// `samples/fonts`, and plugin asset directories).  When `None` (the
    /// default), the working directory at server startup is used.
    /// A relative path is resolved against the startup working directory.
    /// The value is snapshotted once at startup and not re-read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_root: Option<std::path::PathBuf>,
}

#[derive(Debug)]
pub struct ConfigLoadResult {
    pub config: Config,
    pub file_missing: Option<String>,
}

/// Loads the application configuration from defaults, a TOML file, and environment variables.
///
/// # Errors
///
/// Returns an error if:
/// - The configuration file exists but contains invalid TOML syntax
/// - Environment variables are set but contain invalid values
/// - Configuration values fail validation (e.g., invalid types or constraints)
pub fn load(config_path: &str) -> Result<ConfigLoadResult, Box<figment::Error>> {
    let mut figment =
        Figment::new().merge(figment::providers::Serialized::defaults(Config::default()));

    let mut file_missing = None;

    // Try to load the config file, but don't fail if it doesn't exist
    if std::path::Path::new(config_path).exists() {
        figment = figment.merge(Toml::file(config_path));
    } else {
        file_missing = Some(config_path.to_string());
    }

    let mut config: Config =
        figment.merge(Env::prefixed("SK_").split("__")).extract().map_err(Box::new)?;

    normalize_permissions_config(&mut config);

    if let Err(e) = config.mcp.validate() {
        return Err(Box::new(figment::Error::from(e)));
    }
    if let Err(e) = config.server.metrics.prepare() {
        return Err(Box::new(figment::Error::from(e)));
    }

    Ok(ConfigLoadResult { config, file_missing })
}

fn normalize_permissions_config(config: &mut Config) {
    for role in config.permissions.roles.values_mut() {
        normalize_allowed_samples(&config.server.samples_dir, role.allowed_samples.as_mut_slice());
    }
}

/// Normalize legacy `allowed_samples` patterns to the canonical format.
///
/// Canonical format: paths relative to `[server].samples_dir`, e.g. `oneshot/*.yml`.
///
/// Legacy formats accepted and normalized:
/// - `samples/pipelines/oneshot/*.yml`
/// - `./samples/pipelines/oneshot/*.yml`
/// - `<server.samples_dir>/oneshot/*.yml` (absolute or relative)
fn normalize_allowed_samples(samples_dir: &str, allowed_samples: &mut [String]) {
    let samples_dir = samples_dir.trim();
    let samples_dir = samples_dir.trim_end_matches(['/', '\\']);

    let samples_dir_no_dot = samples_dir.trim_start_matches("./").trim_start_matches(".\\");

    // Common historical prefixes in configs/docs.
    let mut prefixes = vec![
        "samples/pipelines",
        "./samples/pipelines",
        "samples\\pipelines",
        ".\\samples\\pipelines",
    ];
    if !samples_dir.is_empty() {
        prefixes.push(samples_dir);
    }
    if !samples_dir_no_dot.is_empty() {
        prefixes.push(samples_dir_no_dot);
    }
    prefixes.sort_unstable();
    prefixes.dedup();

    for pattern in allowed_samples.iter_mut() {
        let pattern_trimmed = pattern.trim();

        if pattern_trimmed.is_empty() || pattern_trimmed == "*" {
            *pattern = pattern_trimmed.to_string();
            continue;
        }

        let mut normalized = pattern_trimmed.to_string();

        for prefix in &prefixes {
            for sep in ['/', '\\'] {
                let candidate = format!("{prefix}{sep}");
                if normalized.starts_with(&candidate) {
                    normalized = normalized[candidate.len()..].to_string();
                    break;
                }
            }
        }

        *pattern = normalized;
    }
}

/// Generates the default configuration as a pretty-printed TOML string.
///
/// # Errors
///
/// Returns an error if the default configuration cannot be serialized to TOML.
/// This is extremely unlikely in practice as it would indicate a programming error.
pub fn generate_default() -> Result<String, toml::ser::Error> {
    let default_config = Config::default();
    toml::to_string_pretty(&default_config)
}

#[cfg(test)]
// `unwrap` / `expect` are idiomatic in tests where the panic IS the assertion.
// `result_large_err` fires on the closure passed to `figment::Jail::expect_with`,
// whose `Err` variant size is fixed by the upstream API.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]
mod tests {
    use super::*;
    use crate::permissions::Permissions;

    #[derive(Deserialize, Debug, PartialEq, Eq)]
    struct ClampWrapper {
        #[serde(default, deserialize_with = "deserialize_clamp_timeout")]
        v: Option<u64>,
    }

    #[test]
    fn deserialize_clamp_timeout_preserves_none() {
        let w: ClampWrapper = serde_json::from_str(r#"{"v": null}"#).unwrap();
        assert_eq!(w.v, None);
    }

    #[test]
    fn deserialize_clamp_timeout_clamps_zero_to_one() {
        let w: ClampWrapper = serde_json::from_str(r#"{"v": 0}"#).unwrap();
        assert_eq!(w.v, Some(1));
    }

    #[test]
    fn deserialize_clamp_timeout_preserves_one() {
        let w: ClampWrapper = serde_json::from_str(r#"{"v": 1}"#).unwrap();
        assert_eq!(w.v, Some(1));
    }

    #[test]
    fn deserialize_clamp_timeout_preserves_large_values() {
        let w: ClampWrapper = serde_json::from_str(r#"{"v": 60000}"#).unwrap();
        assert_eq!(w.v, Some(60_000));
    }

    #[test]
    fn engine_perf_profile_low_latency_capacities() {
        assert_eq!(EnginePerfProfile::LowLatency.node_input_capacity(), 8);
        assert_eq!(EnginePerfProfile::LowLatency.pin_distributor_capacity(), 4);
    }

    #[test]
    fn engine_perf_profile_balanced_capacities() {
        assert_eq!(EnginePerfProfile::Balanced.node_input_capacity(), 32);
        assert_eq!(EnginePerfProfile::Balanced.pin_distributor_capacity(), 16);
    }

    #[test]
    fn engine_perf_profile_high_throughput_capacities() {
        assert_eq!(EnginePerfProfile::HighThroughput.node_input_capacity(), 128);
        assert_eq!(EnginePerfProfile::HighThroughput.pin_distributor_capacity(), 64);
    }

    #[test]
    fn resolved_node_input_capacity_explicit_beats_profile() {
        let cfg = EngineConfig {
            profile: Some(EnginePerfProfile::Balanced),
            node_input_capacity: Some(7),
            ..EngineConfig::default()
        };
        assert_eq!(cfg.resolved_node_input_capacity(), Some(7));
    }

    #[test]
    fn resolved_node_input_capacity_falls_back_to_profile() {
        let cfg = EngineConfig {
            profile: Some(EnginePerfProfile::HighThroughput),
            node_input_capacity: None,
            ..EngineConfig::default()
        };
        assert_eq!(cfg.resolved_node_input_capacity(), Some(128));
    }

    #[test]
    fn resolved_node_input_capacity_none_when_unset() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.resolved_node_input_capacity(), None);
    }

    #[test]
    fn resolved_pin_distributor_capacity_explicit_beats_profile() {
        let cfg = EngineConfig {
            profile: Some(EnginePerfProfile::Balanced),
            pin_distributor_capacity: Some(3),
            ..EngineConfig::default()
        };
        assert_eq!(cfg.resolved_pin_distributor_capacity(), Some(3));
    }

    #[test]
    fn resolved_pin_distributor_capacity_falls_back_to_profile() {
        let cfg = EngineConfig {
            profile: Some(EnginePerfProfile::LowLatency),
            pin_distributor_capacity: None,
            ..EngineConfig::default()
        };
        assert_eq!(cfg.resolved_pin_distributor_capacity(), Some(4));
    }

    #[test]
    fn resolved_pin_distributor_capacity_none_when_unset() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.resolved_pin_distributor_capacity(), None);
    }

    #[test]
    fn default_engine_batch_size_is_32() {
        assert_eq!(default_engine_batch_size(), 32);
    }

    #[test]
    fn normalize_allowed_samples_strips_default_samples_dir_prefix() {
        let mut patterns = vec![
            "samples/pipelines/oneshot/a.yml".to_string(),
            "./samples/pipelines/dynamic/b.yml".to_string(),
            "samples/pipelines/dyn/c.yml".to_string(),
            "oneshot/d.yml".to_string(),
        ];
        normalize_allowed_samples("./samples/pipelines", &mut patterns);
        assert_eq!(
            patterns,
            vec![
                "oneshot/a.yml".to_string(),
                "dynamic/b.yml".to_string(),
                "dyn/c.yml".to_string(),
                "oneshot/d.yml".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_allowed_samples_strips_custom_samples_dir() {
        let mut patterns = vec![
            "/data/skit/samples/oneshot/foo.yml".to_string(),
            "/data/skit/samples/bar.yml".to_string(),
        ];
        normalize_allowed_samples("/data/skit/samples", &mut patterns);
        assert_eq!(patterns, vec!["oneshot/foo.yml".to_string(), "bar.yml".to_string()]);
    }

    #[test]
    fn normalize_allowed_samples_leaves_unrelated_absolute_paths_intact() {
        let mut patterns = vec!["/etc/passwd".to_string(), "/some/other/path.yml".to_string()];
        normalize_allowed_samples("./samples/pipelines", &mut patterns);
        assert_eq!(patterns, vec!["/etc/passwd".to_string(), "/some/other/path.yml".to_string()]);
    }

    #[test]
    fn normalize_allowed_samples_handles_wildcard_and_blank_patterns() {
        let mut patterns = vec![
            "*".to_string(),
            "   ".to_string(),
            "samples/pipelines/x.yml".to_string(),
            "oneshot/already.yml".to_string(),
        ];
        normalize_allowed_samples("./samples/pipelines", &mut patterns);
        assert_eq!(
            patterns,
            vec![
                "*".to_string(),
                String::new(),
                "x.yml".to_string(),
                "oneshot/already.yml".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_permissions_config_normalizes_each_role_allowed_samples() {
        let mut config = Config::default();
        config.server.samples_dir = "./samples/pipelines".to_string();
        let perms = Permissions {
            allowed_samples: vec![
                "samples/pipelines/oneshot/foo.yml".to_string(),
                "./samples/pipelines/dynamic/bar.yml".to_string(),
            ],
            ..Permissions::default()
        };
        config.permissions.roles.insert("custom".to_string(), perms);

        normalize_permissions_config(&mut config);

        let custom = config.permissions.roles.get("custom").expect("custom role must be preserved");
        assert_eq!(
            custom.allowed_samples,
            vec!["oneshot/foo.yml".to_string(), "dynamic/bar.yml".to_string()]
        );
    }

    #[test]
    fn normalize_permissions_config_preserves_explicit_default_role() {
        let mut config = Config::default();
        config.permissions.default_role = "myrole".to_string();
        normalize_permissions_config(&mut config);
        assert_eq!(config.permissions.default_role, "myrole");
    }

    #[test]
    fn default_permissions_config_has_admin_user_viewer_roles() {
        let config = Config::default();
        assert!(config.permissions.roles.contains_key("admin"));
        assert!(config.permissions.roles.contains_key("user"));
        assert!(config.permissions.roles.contains_key("viewer"));
    }

    #[test]
    fn default_permissions_default_role_is_admin() {
        let config = Config::default();
        assert_eq!(config.permissions.default_role, "admin");
    }

    #[test]
    fn generate_default_returns_parseable_toml_with_expected_defaults() {
        let serialized = generate_default().expect("default config should serialize to TOML");
        let parsed: Config = toml::from_str(&serialized).expect("default TOML must round-trip");

        assert_eq!(parsed.server.address, "127.0.0.1:4545");
        assert_eq!(parsed.server.samples_dir, "./samples/pipelines");
        assert_eq!(parsed.engine.packet_batch_size, 32);
        assert!(parsed.engine.profile.is_none());
        assert!(parsed.engine.node_input_capacity.is_none());
        assert!(!parsed.mcp.enabled);
    }

    // `figment::Jail` runs each closure in a sandboxed cwd with isolated env
    // vars (restored on drop) — required because `load` reads SK_-prefixed env
    // vars and a `Toml::file` relative to the current directory.

    #[test]
    fn load_minimal_toml_returns_documented_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("skit.toml", "")?;
            let result = load("skit.toml").expect("loading empty TOML must succeed");

            assert!(result.file_missing.is_none(), "file should be reported as present");
            assert_eq!(result.config.server.address, "127.0.0.1:4545");
            assert_eq!(result.config.engine.packet_batch_size, 32);
            assert!(result.config.permissions.roles.contains_key("admin"));
            assert!(result.config.permissions.roles.contains_key("user"));
            assert!(result.config.permissions.roles.contains_key("viewer"));
            Ok(())
        });
    }

    #[test]
    fn load_reports_missing_file_without_error() {
        figment::Jail::expect_with(|_jail| {
            let result = load("does-not-exist.toml").expect("missing file is not an error");
            assert_eq!(result.file_missing.as_deref(), Some("does-not-exist.toml"));
            assert_eq!(result.config.server.address, "127.0.0.1:4545");
            Ok(())
        });
    }

    #[test]
    fn load_env_var_overrides_toml_value() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "skit.toml",
                r#"[server]
address = "127.0.0.1:1234"
"#,
            )?;
            jail.set_env("SK_SERVER__ADDRESS", "127.0.0.1:9999");

            let result = load("skit.toml").expect("load with env override must succeed");
            assert_eq!(result.config.server.address, "127.0.0.1:9999");
            Ok(())
        });
    }

    #[test]
    fn load_toml_values_override_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "skit.toml",
                r#"[engine]
packet_batch_size = 64
profile = "low-latency"

[server]
samples_dir = "/srv/skit/samples"
"#,
            )?;
            let result = load("skit.toml").expect("load with overrides must succeed");
            assert_eq!(result.config.engine.packet_batch_size, 64);
            assert!(matches!(result.config.engine.profile, Some(EnginePerfProfile::LowLatency)));
            assert_eq!(result.config.server.samples_dir, "/srv/skit/samples");
            Ok(())
        });
    }

    #[test]
    fn load_invalid_toml_returns_err() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("skit.toml", "this is = not [ valid TOML\n")?;
            let result = load("skit.toml");
            assert!(result.is_err(), "expected Err for malformed TOML, got: {result:?}");
            Ok(())
        });
    }

    #[test]
    fn load_normalizes_allowed_samples_in_roles() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "skit.toml",
                r#"[server]
samples_dir = "./samples/pipelines"

[permissions.roles.custom]
create_sessions = true
allowed_samples = ["samples/pipelines/oneshot/foo.yml", "dynamic/bar.yml"]
allowed_nodes = []
allowed_plugins = []
"#,
            )?;
            let result = load("skit.toml").expect("load with custom role");
            let custom = result
                .config
                .permissions
                .roles
                .get("custom")
                .expect("custom role survives load + normalize");
            assert_eq!(
                custom.allowed_samples,
                vec!["oneshot/foo.yml".to_string(), "dynamic/bar.yml".to_string()]
            );
            Ok(())
        });
    }

    fn request_label(name: &str) -> RequestLabelConfig {
        RequestLabelConfig {
            name: name.to_string(),
            header: "X-Test".to_string(),
            allowed: vec![],
            fallback: "other".to_string(),
        }
    }

    #[test]
    fn metrics_validate_rejects_reserved_label_name() {
        let metrics = MetricsConfig { request_labels: vec![request_label("status")] };
        assert!(metrics.validate().is_err());
    }

    #[test]
    fn metrics_validate_rejects_duplicate_label_name() {
        let metrics = MetricsConfig {
            request_labels: vec![request_label("service"), request_label("service")],
        };
        assert!(metrics.validate().is_err());
    }

    #[test]
    fn metrics_validate_accepts_default() {
        assert!(MetricsConfig::default().validate().is_ok());
    }

    #[test]
    fn metrics_default_is_empty_opt_in() {
        assert!(MetricsConfig::default().request_labels.is_empty());
    }

    #[test]
    fn metrics_validate_rejects_empty_or_invalid_label_name() {
        assert!(MetricsConfig { request_labels: vec![request_label("")] }.validate().is_err());
        assert!(MetricsConfig { request_labels: vec![request_label("   ")] }.validate().is_err());
        assert!(MetricsConfig { request_labels: vec![request_label("1service")] }
            .validate()
            .is_err());
    }

    #[test]
    fn metrics_validate_rejects_sanitized_reserved_collision() {
        // `http_method` sanitizes to the same Prometheus key as `http.method`.
        let metrics = MetricsConfig { request_labels: vec![request_label("http_method")] };
        assert!(metrics.validate().is_err());
    }

    #[test]
    fn metrics_validate_rejects_empty_allowed_value() {
        let metrics = MetricsConfig {
            request_labels: vec![RequestLabelConfig {
                name: "service".to_string(),
                header: "X-Test".to_string(),
                allowed: vec!["tts".to_string(), " ".to_string()],
                fallback: "other".to_string(),
            }],
        };
        assert!(metrics.validate().is_err());
    }

    #[test]
    fn metrics_validate_rejects_empty_or_invalid_header() {
        let empty = RequestLabelConfig {
            name: "service".to_string(),
            header: String::new(),
            allowed: vec!["tts".to_string()],
            fallback: "other".to_string(),
        };
        assert!(MetricsConfig { request_labels: vec![empty] }.validate().is_err());
        let bad = RequestLabelConfig {
            name: "service".to_string(),
            header: "bad header".to_string(),
            allowed: vec!["tts".to_string()],
            fallback: "other".to_string(),
        };
        assert!(MetricsConfig { request_labels: vec![bad] }.validate().is_err());
    }

    #[test]
    fn metrics_validate_rejects_empty_fallback() {
        let metrics = MetricsConfig {
            request_labels: vec![RequestLabelConfig {
                name: "service".to_string(),
                header: "X-Test".to_string(),
                allowed: vec!["tts".to_string()],
                fallback: "  ".to_string(),
            }],
        };
        assert!(metrics.validate().is_err());
    }

    #[test]
    fn metrics_prepare_normalizes_then_validates() {
        let mut metrics = MetricsConfig {
            request_labels: vec![RequestLabelConfig {
                name: "service".to_string(),
                header: "X-StreamKit-Service".to_string(),
                allowed: vec!["  TTS ".to_string()],
                fallback: "Other".to_string(),
            }],
        };
        assert!(metrics.prepare().is_ok());
        assert_eq!(metrics.request_labels[0].allowed, vec!["tts".to_string()]);
        assert_eq!(metrics.request_labels[0].fallback, "other");
    }

    #[test]
    fn metrics_normalize_lowercases_fallback() {
        let mut metrics = MetricsConfig {
            request_labels: vec![RequestLabelConfig {
                name: "service".to_string(),
                header: "X-Test".to_string(),
                allowed: vec!["tts".to_string()],
                fallback: " Other ".to_string(),
            }],
        };
        metrics.normalize();
        assert_eq!(metrics.request_labels[0].fallback, "other");
    }

    #[test]
    fn metrics_normalize_lowercases_allowlist() {
        let mut metrics = MetricsConfig {
            request_labels: vec![RequestLabelConfig {
                name: "service".to_string(),
                header: "X-StreamKit-Service".to_string(),
                allowed: vec!["  TTS ".to_string(), "Stt".to_string()],
                fallback: "other".to_string(),
            }],
        };
        metrics.normalize();
        assert_eq!(metrics.request_labels[0].allowed, vec!["tts".to_string(), "stt".to_string()]);
    }

    #[test]
    fn load_rejects_reserved_metrics_label_name() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "skit.toml",
                r#"[[server.metrics.request_labels]]
name = "http.route"
header = "X-StreamKit-Service"
allowed = ["tts"]
"#,
            )?;
            assert!(load("skit.toml").is_err(), "reserved label name must fail load");
            Ok(())
        });
    }
}
