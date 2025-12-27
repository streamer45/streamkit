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
    #[cfg(feature = "moq")]
    pub moq_address: Option<String>,
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
            #[cfg(feature = "moq")]
            moq_address: Some("127.0.0.1:4545".to_string()),
            #[cfg(feature = "moq")]
            moq_gateway_url: None,
        }
    }
}

/// Plugin directory configuration.
#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
pub struct PluginConfig {
    pub directory: String,
    /// Controls whether runtime plugin upload/delete is allowed via the public APIs.
    ///
    /// Default is false to avoid accidental exposure when running without an auth layer.
    #[serde(default)]
    pub allow_http_management: bool,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self { directory: ".plugins".to_string(), allow_http_management: false }
    }
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
