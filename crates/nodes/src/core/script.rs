// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use rquickjs::function::{Func, Opt};
use rquickjs::IntoJs;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use streamkit_core::control::NodeControlMessage;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::telemetry::{TelemetryEmitter, TelemetryEvent};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};
use tokio::sync::{mpsc, Semaphore};

/// Maps a server-configured secret to an HTTP header for fetch() calls
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeaderMapping {
    /// Secret name (must exist in server config's [script.secrets])
    pub secret: String,

    /// HTTP header name (e.g., "Authorization", "X-API-Key")
    pub header: String,

    /// Optional template for formatting the header value
    /// Use {} as placeholder for the secret value
    /// Examples: "Bearer {}", "token {}", "ApiKey {}"
    /// Default: "{}" (raw value)
    #[serde(default = "default_header_template")]
    pub template: String,
}

fn default_header_template() -> String {
    "{}".to_string()
}

/// Configuration for the script node
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ScriptConfig {
    /// JavaScript code (must define a process(packet) function)
    pub script: String,

    /// Optional path to a JavaScript file to load as the script.
    ///
    /// If set, the file contents are loaded at node creation time.
    /// For security, the StreamKit server validates this path against `security.allowed_file_paths`.
    #[serde(default)]
    pub script_path: Option<String>,

    /// Per-packet timeout in milliseconds (default: 100ms)
    pub timeout_ms: u64,

    /// QuickJS memory limit in MB (default: 64MB)
    pub memory_limit_mb: usize,

    /// Header mappings for fetch() calls
    /// Maps secret names to HTTP headers with optional templates
    #[serde(default)]
    pub headers: Vec<HeaderMapping>,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            script: String::new(),
            script_path: None,
            timeout_ms: 100,
            memory_limit_mb: 64,
            headers: Vec::new(),
        }
    }
}

/// URL allowlist rule for fetch() API
/// This structure is used in global server configuration only
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AllowlistRule {
    /// URL pattern with wildcards (e.g., "https://api.example.com/*")
    pub url: String,

    /// Allowed HTTP methods
    pub methods: Vec<String>,
}

/// Global script configuration passed from server config
#[derive(Debug, Clone)]
pub struct GlobalScriptConfig {
    /// Global fetch allowlist that applies to all script nodes
    pub global_fetch_allowlist: Vec<AllowlistRule>,

    /// Available secrets (name → secret)
    /// Loaded from environment variables at server startup.
    pub secrets: std::collections::HashMap<String, ScriptSecret>,
}

/// A server-configured secret value with optional scoping rules.
///
/// Secrets are never exposed directly to JavaScript. They can only be injected into
/// outbound `fetch()` headers via pipeline configuration.
#[derive(Debug, Clone)]
pub struct ScriptSecret {
    pub value: String,
    /// Optional allowlist of URL patterns this secret may be injected into.
    ///
    /// Patterns use the same format as `script.global_fetch_allowlist` entries, e.g.:
    /// - `https://api.openai.com/*`
    /// - `https://api.openai.com/v1/chat/completions`
    ///
    /// Empty = no additional restriction (backwards-compatible).
    pub allowed_fetch_urls: Vec<String>,
}

/// State for tracking active telemetry spans.
/// Uses host-side timing to avoid JavaScript clock issues.
#[derive(Debug, Clone)]
struct SpanState {
    /// Event type for this span (e.g., "llm.request")
    event_type: String,
    /// Correlation ID for grouping related events
    correlation_id: Option<String>,
    /// Turn ID for voice agent conversation grouping
    turn_id: Option<String>,
    /// Initial data passed to startSpan
    initial_data: JsonValue,
    /// Host-side start time for accurate duration calculation
    start_time: Instant,
}

/// Shared state for telemetry spans across JavaScript calls
type SpanRegistry = Arc<Mutex<HashMap<String, SpanState>>>;

/// A node that executes user-provided JavaScript for API integration, webhooks,
/// text transformation, and dynamic routing.
///
/// ## Use Cases
/// - API integration (fetch external data)
/// - Webhook notifications (send transcriptions to Slack/Discord)
/// - Text transformation (format, filter, route)
/// - Conditional processing (drop/transform based on content)
/// - Metadata-based routing (add routing flags)
///
/// ## Anti-Patterns
/// - Audio processing (use native plugins instead)
/// - Blocking operations (keep scripts fast <100ms)
///
/// ## Security
/// - Memory limit (64MB default)
/// - Execution timeout (100ms default)
/// - fetch() URL allowlist (server config only, empty = block all)
/// - No filesystem access
/// - No per-pipeline allowlist override (prevents bypass via user-uploaded pipelines)
#[derive(Debug)]
pub struct ScriptNode {
    config: ScriptConfig,
    global_config: Option<GlobalScriptConfig>,
}

impl ScriptNode {
    const DEFAULT_FETCH_MAX_IN_FLIGHT: usize = 16;
    const MAX_SCRIPT_BYTES: usize = 256 * 1024;

    fn shared_http_client() -> Result<&'static reqwest::Client, String> {
        static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    // Security: don't follow redirects (avoid allowlist bypass + secret leaks).
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(5))
                    .build()
            })
            .as_ref()
            .map_err(|e| format!("Failed to initialize HTTP client: {e}"))
    }

    fn global_fetch_semaphore() -> std::sync::Arc<Semaphore> {
        static SEM: OnceLock<std::sync::Arc<Semaphore>> = OnceLock::new();
        SEM.get_or_init(|| {
            let max_in_flight = std::env::var("SK_SCRIPT_FETCH_MAX_INFLIGHT")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(Self::DEFAULT_FETCH_MAX_IN_FLIGHT)
                .max(1);
            std::sync::Arc::new(Semaphore::new(max_in_flight))
        })
        .clone()
    }

    fn parse_allowlist_pattern(pattern: &str) -> Option<(String, String, String)> {
        let (scheme, rest) = pattern.split_once("://")?;
        if scheme.trim().is_empty() {
            return None;
        }

        let scheme = scheme.trim().to_ascii_lowercase();
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }

        let (host_pattern, path_pattern) = rest.split_once('/').map_or_else(
            || (rest.to_string(), "/*".to_string()),
            |(host, path)| (host.to_string(), format!("/{path}")),
        );

        if host_pattern.trim().is_empty() {
            return None;
        }

        Some((scheme, host_pattern, path_pattern))
    }

    fn is_url_allowed_by_patterns(url: &str, patterns: &[String]) -> bool {
        let parsed_url = url::Url::parse(url).ok();
        let Some(parsed_url) = parsed_url else {
            return false;
        };

        let scheme = parsed_url.scheme().to_ascii_lowercase();
        let host = parsed_url.host_str().unwrap_or_default();
        if host.is_empty() {
            return false;
        }

        let host_port =
            parsed_url.port().map_or_else(|| host.to_string(), |p| format!("{host}:{p}"));
        let path = parsed_url.path();

        patterns.iter().any(|pattern| {
            let Some((rule_scheme, rule_host_pattern, rule_path_pattern)) =
                Self::parse_allowlist_pattern(pattern)
            else {
                tracing::warn!(
                    target: "streamkit::script",
                    pattern = %pattern,
                    "Ignoring invalid secret URL allowlist pattern (expected format like 'https://example.com/*')"
                );
                return false;
            };

            if rule_scheme != scheme {
                return false;
            }

            let host_candidate =
                if rule_host_pattern.contains(':') { host_port.as_str() } else { host };
            if !wildmatch::WildMatch::new(&rule_host_pattern).matches(host_candidate) {
                return false;
            }

            wildmatch::WildMatch::new(&rule_path_pattern).matches(path)
        })
    }

    fn is_secret_allowed_for_url(secret: &ScriptSecret, url: &str) -> bool {
        if secret.allowed_fetch_urls.is_empty() {
            return true;
        }
        Self::is_url_allowed_by_patterns(url, &secret.allowed_fetch_urls)
    }

    /// Creates a new script node from configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Script is empty
    /// - QuickJS runtime initialization fails
    /// - Script has syntax errors
    /// - Script doesn't define a `process(packet)` function
    pub fn new(
        params: Option<&serde_json::Value>,
        global_config: Option<GlobalScriptConfig>,
    ) -> Result<Self, StreamKitError> {
        // For dynamic nodes, allow None to create a default instance for pin inspection
        let mut config: ScriptConfig = if params.is_none() {
            // Dummy config for pin inspection only (used by NodeRegistry::definitions())
            ScriptConfig {
                script: "function process(p) { return p; }".to_string(),
                ..Default::default()
            }
        } else {
            config_helpers::parse_config_required(params)?
        };

        // Validate script is not empty (skip for dummy config)
        if params.is_some() {
            let has_inline = !config.script.trim().is_empty();
            let has_path = config.script_path.as_ref().is_some_and(|p| !p.trim().is_empty());

            if has_inline && has_path {
                return Err(StreamKitError::Configuration(
                    "Script config must set only one of 'script' or 'script_path'".to_string(),
                ));
            }

            if !has_inline && !has_path {
                return Err(StreamKitError::Configuration(
                    "Script cannot be empty (set 'script' or 'script_path')".to_string(),
                ));
            }

            if has_path {
                let path = config
                    .script_path
                    .as_ref()
                    .ok_or_else(|| {
                        StreamKitError::Configuration("script_path missing".to_string())
                    })?
                    .trim();

                let bytes = std::fs::read(path).map_err(|e| {
                    StreamKitError::Configuration(format!(
                        "Failed to read script file '{path}': {e}",
                    ))
                })?;

                if bytes.len() > Self::MAX_SCRIPT_BYTES {
                    return Err(StreamKitError::Configuration(format!(
                        "Script file '{}' is too large ({} bytes > {} bytes)",
                        path,
                        bytes.len(),
                        Self::MAX_SCRIPT_BYTES
                    )));
                }

                let loaded = String::from_utf8(bytes).map_err(|e| {
                    StreamKitError::Configuration(format!(
                        "Script file '{path}' is not valid UTF-8: {e}",
                    ))
                })?;

                config.script = loaded;
            }
        }

        // Validate header mappings reference available secrets
        if !config.headers.is_empty() {
            if let Some(ref global) = global_config {
                for mapping in &config.headers {
                    if !global.secrets.contains_key(&mapping.secret) {
                        let available: Vec<&String> = global.secrets.keys().collect();
                        return Err(StreamKitError::Configuration(format!(
                            "Header mapping references unknown secret '{}'. Available secrets: {:?}",
                            mapping.secret, available
                        )));
                    }
                }
            } else {
                return Err(StreamKitError::Configuration(
                    "Header mappings configured but no secrets available from server config"
                        .to_string(),
                ));
            }
        }

        // Basic validation - we'll do full validation in run() when we create the runtime
        // For now, just check script is non-empty and headers reference valid secrets
        Ok(Self { config, global_config })
    }

    /// Factory function for dynamic node registration
    /// Accepts optional global script configuration from server config
    pub fn factory(global_config: Option<GlobalScriptConfig>) -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(move |params| Ok(Box::new(Self::new(params, global_config.clone())?)))
    }

    /// Validates the script: checks syntax and verifies process() function exists
    async fn validate_script(
        &self,
        context: &rquickjs::AsyncContext,
    ) -> Result<(), StreamKitError> {
        context
            .with(|ctx| {
                // Execute to load functions and check syntax
                ctx.eval::<(), _>(self.config.script.as_str())?;

                // Verify process() function exists
                let globals = ctx.globals();
                let _process_fn: rquickjs::Function = globals.get("process").map_err(|_| {
                    rquickjs::Error::new_from_js(
                        "script",
                        "Script must define a 'process(packet)' function",
                    )
                })?;

                Ok::<(), rquickjs::Error>(())
            })
            .await
            .map_err(|e| StreamKitError::Configuration(format!("Script validation failed: {e}")))
    }

    /// Converts a Rust Packet to a JavaScript value
    ///
    /// Uses smart marshalling:
    /// - Text/Transcription: Full conversion
    /// - Audio/Binary: Metadata only (avoids expensive copying)
    /// - Custom: Full conversion (JSON payload + optional metadata)
    fn json_value_to_js<'js>(
        ctx: &rquickjs::Ctx<'js>,
        value: &JsonValue,
    ) -> Result<rquickjs::Value<'js>, StreamKitError> {
        match value {
            JsonValue::Null => Ok(rquickjs::Value::new_null(ctx.clone())),
            JsonValue::Bool(b) => Ok((*b).into_js(ctx).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to convert JSON bool to JS: {e}"))
            })?),
            JsonValue::Number(n) => {
                let num = n.as_f64().ok_or_else(|| {
                    StreamKitError::Runtime("Failed to convert JSON number to f64".to_string())
                })?;
                Ok(num.into_js(ctx).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to convert JSON number to JS: {e}"))
                })?)
            },
            JsonValue::String(s) => Ok(s.as_str().into_js(ctx).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to convert JSON string to JS: {e}"))
            })?),
            JsonValue::Array(arr) => {
                let js_arr = rquickjs::Array::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create JS array: {e}"))
                })?;
                for (i, item) in arr.iter().enumerate() {
                    js_arr.set(i, Self::json_value_to_js(ctx, item)?).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set array element: {e}"))
                    })?;
                }
                Ok(js_arr.into())
            },
            JsonValue::Object(map) => {
                let js_obj = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create JS object: {e}"))
                })?;
                for (k, v) in map {
                    js_obj.set(k.as_str(), Self::json_value_to_js(ctx, v)?).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set object field: {e}"))
                    })?;
                }
                Ok(js_obj.into())
            },
        }
    }

    fn packet_to_js<'js>(
        packet: &Packet,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<rquickjs::Value<'js>, StreamKitError> {
        let obj = rquickjs::Object::new(ctx.clone())
            .map_err(|e| StreamKitError::Runtime(format!("Failed to create JS object: {e}")))?;

        match packet {
            Packet::Text(text) => {
                obj.set("type", "Text")
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type: {e}")))?;
                obj.set("data", text.as_ref())
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set data: {e}")))?;
            },

            Packet::Audio(frame) => {
                // Audio: metadata only (no samples)
                obj.set("type", "Audio")
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type: {e}")))?;

                let metadata = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create metadata: {e}"))
                })?;
                metadata.set("sample_rate", frame.sample_rate).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to set sample_rate: {e}"))
                })?;
                metadata
                    .set("channels", frame.channels)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set channels: {e}")))?;
                let frames_count = frame.samples.len() / frame.channels as usize;
                metadata
                    .set("frames", frames_count)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set frames: {e}")))?;

                let frames_count_u64 = u64::try_from(frames_count).unwrap_or(u64::MAX);
                let duration_ms =
                    frames_count_u64.saturating_mul(1000) / u64::from(frame.sample_rate);
                metadata.set("duration_ms", duration_ms).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to set duration_ms: {e}"))
                })?;

                obj.set("metadata", metadata)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set metadata: {e}")))?;
            },

            Packet::Transcription(transcription) => {
                obj.set("type", "Transcription")
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type: {e}")))?;

                let data = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create data object: {e}"))
                })?;

                data.set("text", transcription.text.as_str())
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set text: {e}")))?;

                if let Some(ref lang) = transcription.language {
                    data.set("language", lang.as_str()).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set language: {e}"))
                    })?;
                }

                // Convert segments array
                let segments = rquickjs::Array::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create segments array: {e}"))
                })?;

                for (i, segment) in transcription.segments.iter().enumerate() {
                    let seg_obj = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to create segment object: {e}"))
                    })?;

                    seg_obj.set("text", segment.text.as_str()).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set segment text: {e}"))
                    })?;
                    seg_obj.set("start_time_ms", segment.start_time_ms).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set start_time_ms: {e}"))
                    })?;
                    seg_obj.set("end_time_ms", segment.end_time_ms).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set end_time_ms: {e}"))
                    })?;

                    if let Some(confidence) = segment.confidence {
                        seg_obj.set("confidence", confidence).map_err(|e| {
                            StreamKitError::Runtime(format!("Failed to set confidence: {e}"))
                        })?;
                    }

                    segments.set(i, seg_obj).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set segment in array: {e}"))
                    })?;
                }

                data.set("segments", segments)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set segments: {e}")))?;

                obj.set("data", data)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set data: {e}")))?;
            },

            Packet::Custom(custom) => {
                obj.set("type", "Custom")
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type: {e}")))?;

                obj.set("type_id", custom.type_id.as_str())
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type_id: {e}")))?;

                let encoding = match custom.encoding {
                    streamkit_core::types::CustomEncoding::Json => "json",
                };
                obj.set("encoding", encoding)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set encoding: {e}")))?;

                obj.set("data", Self::json_value_to_js(ctx, &custom.data)?)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set data: {e}")))?;

                if let Some(meta) = &custom.metadata {
                    let js_meta = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to create metadata: {e}"))
                    })?;

                    if let Some(ts) = meta.timestamp_us {
                        js_meta.set("timestamp_us", ts).map_err(|e| {
                            StreamKitError::Runtime(format!("Failed to set timestamp_us: {e}"))
                        })?;
                    }
                    if let Some(dur) = meta.duration_us {
                        js_meta.set("duration_us", dur).map_err(|e| {
                            StreamKitError::Runtime(format!("Failed to set duration_us: {e}"))
                        })?;
                    }
                    if let Some(seq) = meta.sequence {
                        js_meta.set("sequence", seq).map_err(|e| {
                            StreamKitError::Runtime(format!("Failed to set sequence: {e}"))
                        })?;
                    }

                    obj.set("metadata", js_meta).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set metadata: {e}"))
                    })?;
                }
            },

            Packet::Binary { data, content_type, .. } => {
                // Binary: metadata only (no data copying for MVP)
                obj.set("type", "Binary")
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set type: {e}")))?;

                let metadata = rquickjs::Object::new(ctx.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create metadata: {e}"))
                })?;

                if let Some(ref ct) = content_type {
                    metadata.set("content_type", ct.as_ref()).map_err(|e| {
                        StreamKitError::Runtime(format!("Failed to set content_type: {e}"))
                    })?;
                }

                metadata
                    .set("size", data.len())
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set size: {e}")))?;

                obj.set("metadata", metadata)
                    .map_err(|e| StreamKitError::Runtime(format!("Failed to set metadata: {e}")))?;
            },
        }

        Ok(obj.into())
    }

    /// Converts a JavaScript value to a Rust Packet
    ///
    /// Returns:
    /// - Some(Packet) - Continue processing with this packet
    /// - None - Drop the packet
    #[allow(clippy::needless_pass_by_value, clippy::unused_self)]
    fn js_to_packet(
        &self,
        value: rquickjs::Value<'_>,
        original_packet: &Packet,
    ) -> Result<Option<streamkit_core::types::Packet>, StreamKitError> {
        // null or undefined = drop packet
        if value.is_null() || value.is_undefined() {
            tracing::debug!("JavaScript returned null/undefined, dropping packet");
            return Ok(None);
        }

        // Must be an object
        let obj = value.as_object().ok_or_else(|| {
            tracing::error!("JavaScript result is not an object, type: {:?}", value.type_name());
            StreamKitError::Runtime(format!(
                "Script must return an object or null, got: {}",
                value.type_name()
            ))
        })?;

        // Get packet type
        let packet_type: String = obj.get("type").map_err(|e| {
            tracing::error!("Failed to get 'type' field from JavaScript object: {}", e);
            StreamKitError::Runtime(format!("Return value must have a 'type' field: {e}"))
        })?;

        tracing::debug!("JavaScript returned packet type: {}", packet_type);

        // For MVP: only Text packets can be created from JavaScript
        if packet_type.as_str() == "Text" {
            let data: String = obj.get("data").map_err(|e| {
                StreamKitError::Runtime(format!("Text packet must have 'data' field: {e}"))
            })?;
            Ok(Some(Packet::Text(data.into())))
        } else {
            // Other packet types: pass through the original packet unchanged
            // Note: Any metadata modifications in JavaScript are lost (acceptable for MVP)
            tracing::debug!(
                "JavaScript returned {} packet - passing through original (metadata changes lost)",
                packet_type
            );
            Ok(Some(original_packet.clone()))
        }
    }

    /// Executes the user script with the given packet
    ///
    /// Supports both synchronous and asynchronous functions:
    ///
    /// ```javascript
    /// // Synchronous (returns value directly)
    /// function process(packet) {
    ///   return {type: 'Text', data: 'result'};
    /// }
    ///
    /// // Asynchronous (returns Promise, automatically resolved)
    /// async function process(packet) {
    ///   const response = await someAsyncOperation();
    ///   return {type: 'Text', data: response};
    /// }
    /// ```
    ///
    /// Note: fetch() is implemented as a blocking call, so it works with or without await.
    /// However, using `async/await` is now supported and recommended for clarity.
    fn execute_script<'js>(
        js_packet: rquickjs::Value<'js>,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<rquickjs::Value<'js>, rquickjs::Error> {
        let globals = ctx.globals();
        let process_fn: rquickjs::Function = globals.get("process")?;
        let result: rquickjs::Value = process_fn.call((js_packet,))?;

        // Check if result is a Promise (from async function)
        result.clone().as_promise().map_or_else(
            || {
                // Sync function, return value directly
                tracing::trace!("Function returned value directly (synchronous)");
                Ok(result)
            },
            |promise| {
                tracing::debug!("Function returned Promise, executing job queue to resolve");

                // finish() executes pending jobs until promise settles
                // This works because our fetch() is blocking - all work completes in one tick
                promise.finish::<rquickjs::Value>().map_err(|e| {
                    tracing::error!("Promise resolution failed: {:?}", e);
                    e
                })
            },
        )
    }

    /// Processes a single packet through the script
    async fn process_packet(
        &self,
        context: &rquickjs::AsyncContext,
        packet: Packet,
        timeout: Duration,
        stats: &mut NodeStatsTracker,
    ) -> Option<streamkit_core::types::Packet> {
        // Clone for pass-through on error
        let packet_clone = packet.clone();

        // Execute script with timeout - all JS work happens synchronously inside with()
        tracing::trace!("Processing packet: {:?}", std::mem::discriminant(&packet));

        let process_future = context.with(|ctx| {
            // Convert packet to JS
            let js_packet = Self::packet_to_js(&packet, &ctx).map_err(|_e| {
                rquickjs::Error::new_from_js(
                    "marshalling",
                    "Failed to convert Rust packet to JavaScript",
                )
            })?;

            // Execute script
            let result = Self::execute_script(js_packet, &ctx)?;

            // Convert result back to Packet
            let output = self.js_to_packet(result, &packet).map_err(|e| {
                tracing::error!("js_to_packet conversion failed: {}", e);
                rquickjs::Error::new_from_js(
                    "unmarshalling",
                    "Failed to convert JavaScript result to packet (see logs for details)",
                )
            })?;

            tracing::trace!("Script executed successfully");
            Ok::<Option<streamkit_core::types::Packet>, rquickjs::Error>(output)
        });

        match tokio::time::timeout(timeout, process_future).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                tracing::error!("Script error: {}", e);
                stats.errored();
                Some(packet_clone) // Pass through on error
            },
            Err(_) => {
                tracing::error!("Script timeout ({}ms)", self.config.timeout_ms);
                stats.errored();
                Some(packet_clone) // Pass through on timeout
            },
        }
    }

    /// Checks if a URL and method are allowed by the allowlist
    fn is_url_allowed(url: &str, method: &str, allowlist: &[AllowlistRule]) -> bool {
        if allowlist.is_empty() {
            return false; // Fail-safe: empty allowlist = block all
        }

        let parsed_url = url::Url::parse(url).ok();
        let Some(parsed_url) = parsed_url else {
            tracing::warn!(target: "streamkit::script", url = %url, "Fetch blocked: invalid URL");
            return false;
        };

        let scheme = parsed_url.scheme().to_ascii_lowercase();
        let host = parsed_url.host_str().unwrap_or_default();
        if host.is_empty() {
            tracing::warn!(target: "streamkit::script", url = %url, "Fetch blocked: missing host");
            return false;
        }

        let host_port =
            parsed_url.port().map_or_else(|| host.to_string(), |p| format!("{host}:{p}"));
        let path = parsed_url.path();

        allowlist.iter().any(|rule| {
            // Check method
            if !rule.methods.iter().any(|m| m.eq_ignore_ascii_case(method)) {
                return false;
            }

            let Some((rule_scheme, rule_host_pattern, rule_path_pattern)) =
                Self::parse_allowlist_pattern(&rule.url)
            else {
                tracing::warn!(
                    target: "streamkit::script",
                    rule = %rule.url,
                    "Ignoring invalid fetch allowlist rule (expected format like 'https://example.com/*')"
                );
                return false;
            };

            if rule_scheme != scheme {
                return false;
            }

            // If the rule host includes an explicit port pattern, match against host:port.
            // Otherwise, match against host only.
            let host_candidate =
                if rule_host_pattern.contains(':') { host_port.as_str() } else { host };
            if !wildmatch::WildMatch::new(&rule_host_pattern).matches(host_candidate) {
                return false;
            }

            wildmatch::WildMatch::new(&rule_path_pattern).matches(path)
        })
    }

    /// Initializes Web APIs (console, fetch)
    async fn initialize_web_apis(
        &self,
        context: &rquickjs::AsyncContext,
    ) -> Result<(), StreamKitError> {
        // Register console.log/error/warn
        context
            .with(|ctx| {
                let console = rquickjs::Object::new(ctx.clone())?;

                console.set(
                    "log",
                    Func::from(|msg: String| {
                        tracing::info!(target: "streamkit::script", "{}", msg);
                    }),
                )?;

                console.set(
                    "error",
                    Func::from(|msg: String| {
                        tracing::error!(target: "streamkit::script", "{}", msg);
                    }),
                )?;

                console.set(
                    "warn",
                    Func::from(|msg: String| {
                        tracing::warn!(target: "streamkit::script", "{}", msg);
                    }),
                )?;

                ctx.globals().set("console", console)?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Console API init failed: {e}")))?;

        // Register fetch API
        self.register_fetch(context).await?;

        Ok(())
    }

    /// Registers the fetch() API with POST support, secret injection, and URL allowlist validation
    ///
    /// Supports:
    /// - GET/POST/PUT/PATCH/DELETE methods
    /// - JSON request bodies (must be pre-stringified with JSON.stringify)
    /// - HTTP headers from secrets (configured in pipeline, injected by Rust)
    /// - Custom headers from JavaScript (in addition to secret-based ones)
    ///
    /// # Usage from JavaScript
    /// ```javascript
    /// // GET request
    /// const response = await fetch('https://api.example.com/data');
    ///
    /// // POST request with JSON body
    /// const response = await fetch('https://api.example.com/data', {
    ///   method: 'POST',
    ///   body: JSON.stringify({ key: 'value' })
    /// });
    ///
    /// // With custom headers
    /// const response = await fetch('https://api.example.com/data', {
    ///   method: 'POST',
    ///   headers: { 'X-Custom-Header': 'value' },
    ///   body: JSON.stringify({ key: 'value' })
    /// });
    /// ```
    async fn register_fetch(&self, context: &rquickjs::AsyncContext) -> Result<(), StreamKitError> {
        // Use only global allowlist from server configuration
        // Empty allowlist = block all fetch() calls (secure by default)
        let allowlist = self
            .global_config
            .as_ref()
            .map(|gc| gc.global_fetch_allowlist.clone())
            .unwrap_or_default();

        // Get available secrets
        let secrets = self.global_config.as_ref().map(|gc| gc.secrets.clone()).unwrap_or_default();

        // Get header mappings
        let header_mappings = self.config.headers.clone();
        let fetch_semaphore = Self::global_fetch_semaphore();

        context
            .with(|ctx| {
                let allowlist_clone = allowlist.clone();
                let secrets_clone = secrets.clone();
                let headers_clone = header_mappings.clone();
                let fetch_semaphore = fetch_semaphore.clone();

                let fetch_fn = Func::from(move |url: String, options: Opt<rquickjs::Object>| {
                    let allowlist = allowlist_clone.clone();
                    let secrets = secrets_clone.clone();
                    let header_configs = headers_clone.clone();
                    let fetch_semaphore = fetch_semaphore.clone();

                    // Convert Opt to Option for easier handling
                    let options = options.0;

                    // Parse options: { method?, headers?, body? }
                    let method = options
                        .as_ref()
                        .and_then(|o| o.get::<_, String>("method").ok())
                        .unwrap_or_else(|| "GET".to_string())
                        .to_uppercase();

                    // Check allowlist
                    if !Self::is_url_allowed(&url, &method, &allowlist) {
                        if allowlist.is_empty() {
                            tracing::warn!(target: "streamkit::script", "Fetch blocked: Global allowlist is empty. URL: {}", url);
                            return Err::<String, rquickjs::Error>(rquickjs::Error::new_from_js(
                                "fetch",
                                "Blocked: Global allowlist is empty",
                            ));
                        }
                        tracing::warn!(target: "streamkit::script", "Fetch blocked: URL not in global allowlist. URL: {}", url);
                        return Err::<String, rquickjs::Error>(rquickjs::Error::new_from_js(
                            "fetch",
                            "Blocked: URL not in global allowlist",
                        ));
                    }

                    tracing::debug!(target: "streamkit::script", "Fetch allowed: {} {}", method, url);

                    // Execute async reqwest in a blocking context.
                    //
                    // PERFORMANCE NOTE: block_in_place can stall the current Tokio worker thread.
                    // If many concurrent fetch() calls happen across script nodes, this can exhaust
                    // the thread pool and cause latency spikes for unrelated pipeline tasks.
                    // For scripts with frequent/concurrent fetches, consider rate limiting or
                    // batching requests. If this becomes a bottleneck, we could migrate to
                    // spawn_blocking for true isolation.
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            let _permit = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                fetch_semaphore.acquire_owned(),
                            )
                            .await
                            .map_err(|_| "Fetch blocked: too many concurrent fetch() calls")?
                            .map_err(|_| "Fetch blocked: fetch limiter unavailable")?;

                            let client = Self::shared_http_client()?;

                            // Build request
                            let method_enum = method
                                .parse()
                                .map_err(|e| format!("Invalid HTTP method: {e}"))?;
                            let mut request = client.request(method_enum, &url);

                            // Add configured headers with secrets (injected by Rust)
                            for mapping in &header_configs {
                                if let Some(secret) = secrets.get(&mapping.secret) {
                                    if !Self::is_secret_allowed_for_url(secret, &url) {
                                        tracing::warn!(
                                            target: "streamkit::script",
                                            secret = %mapping.secret,
                                            url = %url,
                                            "Secret injection blocked: URL not allowed for secret"
                                        );
                                        continue;
                                    }

                                    let header_value = mapping.template.replace("{}", &secret.value);
                                    request = request.header(&mapping.header, header_value);
                                } else {
                                    tracing::warn!(
                                        target: "streamkit::script",
                                        "Secret '{}' not found in server config, header '{}' not added",
                                        mapping.secret,
                                        mapping.header
                                    );
                                }
                            }

                            // Add custom headers from JavaScript (if provided)
                            // These are ADDITIONAL headers, not replacements for secret-based ones
                            if let Some(ref opts) = options {
                                if let Ok(js_headers) = opts.get::<_, rquickjs::Object>("headers") {
                                    for (key, value) in js_headers.props::<String, String>().flatten() {
                                        request = request.header(&key, value);
                                    }
                                }
                            }

                            // Add body for POST/PUT/PATCH
                            // Body must be passed as a JSON string from JavaScript
                            // Example: fetch(url, { method: 'POST', body: JSON.stringify({data: 'value'}) })
                            if matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
                                if let Some(ref opts) = options {
                                    if let Ok(body_str) = opts.get::<_, String>("body") {
                                        request = request
                                            .header("Content-Type", "application/json")
                                            .body(body_str);
                                    }
                                }
                            }

                            // Execute with 5s timeout
                            let response = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                request.send(),
                            )
                            .await
                            .map_err(|_| "Request timeout (5s)")?
                            .map_err(|e| format!("Request failed: {e}"))?;

                            // Read response body with timeout
                            let text = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                response.text(),
                            )
                            .await
                            .map_err(|_| "Response body read timeout (5s)")?
                            .map_err(|e| format!("Failed to read response body: {e}"))?;

                            Ok::<String, String>(text)
                        })
                    });

                    match result {
                        Ok(text) => Ok(text),
                        Err(e) => {
                            tracing::error!(target: "streamkit::script", "Fetch error: {}", e);
                            Err(rquickjs::Error::new_from_js("fetch", "Request failed"))
                        },
                    }
                });

                ctx.globals().set("fetch", fetch_fn)?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Fetch API init failed: {e}")))?;

        Ok(())
    }

    /// Registers the telemetry API for JavaScript.
    ///
    /// Provides three methods:
    /// - `telemetry.emit(event_type, data)` - Emit a simple telemetry event
    /// - `telemetry.startSpan(event_type, data?)` - Start a span and return a span_id
    /// - `telemetry.endSpan(span_id, data?)` - End a span and emit with duration
    ///
    /// All timing is computed host-side to avoid JavaScript clock issues.
    ///
    /// # Usage from JavaScript
    /// ```javascript
    /// // Simple event
    /// telemetry.emit("custom.event", { key: "value" });
    ///
    /// // Span for latency tracking
    /// const spanId = telemetry.startSpan("llm.request", {
    ///     model: "gpt-4",
    ///     turn_id: turnId  // Optional: for voice agent correlation
    /// });
    /// const response = await fetch(...);
    /// telemetry.endSpan(spanId, {
    ///     output_chars: response.length,
    ///     status: "success"
    /// });
    /// // Emits "llm.request" with latency_ms computed by host
    /// ```
    async fn register_telemetry(
        &self,
        context: &rquickjs::AsyncContext,
        telemetry_tx: Option<mpsc::Sender<TelemetryEvent>>,
        node_id: String,
        session_id: Option<String>,
    ) -> Result<(), StreamKitError> {
        let span_registry: SpanRegistry = Arc::new(Mutex::new(HashMap::new()));
        let shared_emitter: Option<Arc<TelemetryEmitter>> = telemetry_tx.as_ref().map(|tx| {
            Arc::new(TelemetryEmitter::new(node_id.clone(), session_id.clone(), Some(tx.clone())))
        });

        context
            .with(|ctx| {
                let telemetry_obj = rquickjs::Object::new(ctx.clone())?;

                // telemetry.emit(event_type, data?)
                let emitter_emit = shared_emitter.clone();
                telemetry_obj.set(
                    "emit",
                    Func::from(
                        move |event_type: String, data: Opt<rquickjs::Value>| -> bool {
                            let Some(ref emitter) = emitter_emit else {
                                tracing::debug!(target: "streamkit::script", "Telemetry emit ignored: no telemetry channel");
                                return false;
                            };

                            // Convert JS value to JSON
                            let json_data = data.0.map_or_else(
                                || serde_json::json!({}),
                                |v| js_value_to_json(&v).unwrap_or_else(|| serde_json::json!({})),
                            );

                            emitter.emit(&event_type, json_data)
                        },
                    ),
                )?;

                // telemetry.startSpan(event_type, data?) -> string (span_id)
                let registry_start = span_registry.clone();
                let emitter_start = shared_emitter.clone();
                telemetry_obj.set(
                    "startSpan",
                    Func::from(
                        move |event_type: String, data: Opt<rquickjs::Value>| -> String {
                            let span_id = uuid::Uuid::new_v4().to_string();

                            // Convert JS value to JSON
                            let mut json_data = data.0.map_or_else(
                                || serde_json::json!({}),
                                |v| js_value_to_json(&v).unwrap_or_else(|| serde_json::json!({})),
                            );

                            // Ensure we always store object payloads so we can attach metadata.
                            if !json_data.is_object() {
                                json_data = serde_json::json!({ "value": json_data });
                            }

                            // Extract correlation_id and turn_id from data if present
                            let mut correlation_id = json_data
                                .get("correlation_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            let turn_id = json_data
                                .get("turn_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);

                            // Default correlation_id to span_id to enable grouping.
                            if correlation_id.is_none() {
                                correlation_id = Some(span_id.clone());
                            }

                            // Attach span metadata to payload (for UI debugging).
                            if let Some(obj) = json_data.as_object_mut() {
                                obj.insert("span_id".to_string(), serde_json::json!(span_id));
                                if let Some(ref cid) = correlation_id {
                                    obj.entry("correlation_id".to_string())
                                        .or_insert_with(|| serde_json::json!(cid));
                                }
                            }

                            // Emit an immediate start event (visibility during long awaits).
                            if let Some(ref emitter) = emitter_start {
                                let start_event_type = format!("{event_type}.start");
                                if let (Some(ref cid), Some(ref tid)) = (&correlation_id, &turn_id) {
                                    emitter.emit_correlated(&start_event_type, cid, tid, json_data.clone());
                                } else if let Some(ref cid) = correlation_id {
                                    emitter.emit_with_correlation(&start_event_type, cid, json_data.clone());
                                } else if let Some(ref tid) = turn_id {
                                    emitter.emit_with_turn(&start_event_type, tid, json_data.clone());
                                } else {
                                    emitter.emit(&start_event_type, json_data.clone());
                                }
                            }

                            let span_state = SpanState {
                                event_type,
                                correlation_id,
                                turn_id,
                                initial_data: json_data,
                                start_time: Instant::now(),
                            };

                            // Use blocking lock since we're in sync JS context
                            // This is a simple HashMap operation, should be fast
                            let mut registry = registry_start
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            registry.insert(span_id.clone(), span_state);

                            span_id
                        },
                    ),
                )?;

                // telemetry.endSpan(span_id, data?) -> bool
                let registry_end = span_registry.clone();
                let emitter_end = shared_emitter.clone();
                telemetry_obj.set(
                    "endSpan",
                    Func::from(move |span_id: String, data: Opt<rquickjs::Value>| -> bool {
                        // Remove span from registry
                        let span_state = {
                            let mut registry = registry_end
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            registry.remove(&span_id)
                        };

                        if span_state.is_none() {
                            tracing::warn!(
                                target: "streamkit::script",
                                "endSpan: span_id not found: {}",
                                span_id
                            );
                        }

                        let Some(span) = span_state else {
                            return false;
                        };

                        let Some(ref emitter) = emitter_end else {
                            // Telemetry may be disabled for this session; still remove the span to avoid leaks.
                            tracing::debug!(target: "streamkit::script", "Telemetry endSpan ignored: no telemetry channel");
                            return false;
                        };

                        // Calculate duration using host-side timing
                        #[allow(clippy::cast_possible_truncation)] // Duration in ms will fit in u64
                        let duration_ms = span.start_time.elapsed().as_millis() as u64;

                        // Merge initial data with end data
                        let mut end_data = data.0.map_or_else(
                            || serde_json::json!({}),
                            |v| js_value_to_json(&v).unwrap_or_else(|| serde_json::json!({})),
                        );
                        if !end_data.is_object() {
                            end_data = serde_json::json!({ "value": end_data });
                        }

                        let mut merged_data = span.initial_data.clone();
                        if let (Some(base), Some(overlay)) =
                            (merged_data.as_object_mut(), end_data.as_object())
                        {
                            for (k, v) in overlay {
                                base.insert(k.clone(), v.clone());
                            }
                        }

                        // Add latency_ms computed by host
                        if let Some(obj) = merged_data.as_object_mut() {
                            obj.insert("latency_ms".to_string(), serde_json::json!(duration_ms));
                            obj.insert("span_id".to_string(), serde_json::json!(span_id));
                        }

                        // Emit the span event
                        // Use correlation if available
                        if let (Some(ref cid), Some(ref tid)) =
                            (&span.correlation_id, &span.turn_id)
                        {
                            emitter.emit_correlated(&span.event_type, cid, tid, merged_data)
                        } else if let Some(ref cid) = span.correlation_id {
                            emitter.emit_with_correlation(&span.event_type, cid, merged_data)
                        } else if let Some(ref tid) = span.turn_id {
                            emitter.emit_with_turn(&span.event_type, tid, merged_data)
                        } else {
                            emitter.emit(&span.event_type, merged_data)
                        }
                    }),
                )?;

                ctx.globals().set("telemetry", telemetry_obj)?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Telemetry API init failed: {e}")))?;

        Ok(())
    }
}

/// Helper function to convert a rquickjs Value to serde_json::Value
fn js_value_to_json(value: &rquickjs::Value<'_>) -> Option<JsonValue> {
    if value.is_null() || value.is_undefined() {
        return Some(JsonValue::Null);
    }
    if let Some(b) = value.as_bool() {
        return Some(JsonValue::Bool(b));
    }
    if let Some(n) = value.as_int() {
        return Some(JsonValue::Number(n.into()));
    }
    if let Some(n) = value.as_float() {
        return serde_json::Number::from_f64(n).map(JsonValue::Number);
    }
    if let Some(s) = value.as_string() {
        return s.to_string().ok().map(JsonValue::String);
    }
    if let Some(arr) = value.as_array() {
        let items: Option<Vec<JsonValue>> = arr
            .iter::<rquickjs::Value>()
            .map(|r| r.ok().and_then(|v| js_value_to_json(&v)))
            .collect();
        return items.map(JsonValue::Array);
    }
    if let Some(obj) = value.as_object() {
        let mut map = serde_json::Map::new();
        for (key, val) in obj.props::<String, rquickjs::Value>().flatten() {
            if let Some(json_val) = js_value_to_json(&val) {
                map.insert(key, json_val);
            }
        }
        return Some(JsonValue::Object(map));
    }
    None
}

#[async_trait]
impl ProcessorNode for ScriptNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::One,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let state_tx = context.state_tx.clone();

        let result: Result<(), StreamKitError> = async {
            // Create QuickJS runtime with memory limit
            let runtime = rquickjs::AsyncRuntime::new().map_err(|e| {
                StreamKitError::Configuration(format!("Failed to create runtime: {e}"))
            })?;

            runtime.set_memory_limit(self.config.memory_limit_mb * 1024 * 1024).await;

            // Create QuickJS context for this execution
            let js_context = rquickjs::AsyncContext::full(&runtime).await.map_err(|e| {
                StreamKitError::Configuration(format!("Failed to create context: {e}"))
            })?;

            // Initialize Web APIs (console, fetch)
            self.initialize_web_apis(&js_context).await?;

            // Initialize Telemetry API (emit, startSpan, endSpan)
            self.register_telemetry(
                &js_context,
                context.telemetry_tx.clone(),
                node_name.clone(),
                context.session_id.clone(),
            )
            .await?;

            // Validate and load the script (syntax check + process() exists).
            //
            // Important: this evaluation also loads the script into the running context, so we
            // must not eval it a second time (top-level `let/const` declarations would fail).
            self.validate_script(&js_context).await?;

            let mut input_rx = context.take_input("in")?;
            let mut stats = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
            let timeout = Duration::from_millis(self.config.timeout_ms);

            state_helpers::emit_running(&context.state_tx, &node_name);

            loop {
                // Check control messages
                match context.control_rx.try_recv() {
                    Ok(NodeControlMessage::Shutdown) => break,
                    Ok(NodeControlMessage::UpdateParams(_)) => {
                        tracing::warn!(
                            "UpdateParams not supported for script nodes (requires node recreation)"
                        );
                    },
                    _ => {},
                }

                // Receive packet
                let Some(packet) = context.recv_with_cancellation(&mut input_rx).await else {
                    break;
                };
                stats.received();
                tracing::debug!(
                    "Received packet for processing: {:?}",
                    std::mem::discriminant(&packet)
                );

                // Process packet
                let output = self.process_packet(&js_context, packet, timeout, &mut stats).await;

                // Send output (if not dropped)
                if let Some(out_packet) = output {
                    if context.output_sender.send("out", out_packet).await.is_err() {
                        break;
                    }
                    stats.sent();
                } else {
                    tracing::debug!("Script dropped packet");
                    stats.discarded();
                }

                stats.maybe_send();
            }

            stats.force_send();
            state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
            Ok(())
        }
        .await;

        if let Err(err) = &result {
            state_helpers::emit_failed(&state_tx, &node_name, err.to_string());
        }

        result
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::manual_string_new)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::sync::Arc;
    use streamkit_core::types::{
        AudioFrame, CustomEncoding, CustomPacketData, PacketMetadata, TranscriptionData,
        TranscriptionSegment,
    };

    const TEST_VAD_EVENT_TYPE_ID: &str = "plugin::native::vad/vad-event@1";

    fn create_test_config(script: &str) -> ScriptConfig {
        ScriptConfig {
            script: script.to_string(),
            script_path: None,
            timeout_ms: 1000,
            memory_limit_mb: 64,
            headers: Vec::new(),
        }
    }

    #[test]
    fn test_empty_script_rejected() {
        let config =
            serde_json::to_value(ScriptConfig { script: "".to_string(), ..Default::default() })
                .unwrap();

        let result = ScriptNode::new(Some(&config), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Script cannot be empty"));
    }

    #[test]
    fn test_whitespace_only_script_rejected() {
        let config = serde_json::to_value(ScriptConfig {
            script: "   \n\t  ".to_string(),
            ..Default::default()
        })
        .unwrap();

        let result = ScriptNode::new(Some(&config), None);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_script_missing_process_function() {
        let config = create_test_config("const x = 42;");
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        let result = node.validate_script(&context).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("process(packet)"));
    }

    #[tokio::test]
    async fn test_script_with_syntax_error() {
        let config = create_test_config("function process(packet) { return packet }}}");
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        let result = node.validate_script(&context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_valid_script_accepted() {
        let config = create_test_config("function process(packet) { return packet; }");
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        let result = node.validate_script(&context).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_text_packet_passthrough() {
        let config = create_test_config("function process(packet) { return packet; }");
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>("function process(packet) { return packet; }")?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let packet = Packet::Text("Hello World".into());
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet.clone(), Duration::from_secs(1), &mut stats).await;

        assert!(result.is_some());
        match result.unwrap() {
            Packet::Text(text) => assert_eq!(text.as_ref(), "Hello World"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[tokio::test]
    async fn test_text_packet_transformation() {
        let config = create_test_config(
            "function process(packet) {
                if (packet.type === 'Text') {
                    return { type: 'Text', data: packet.data.toUpperCase() };
                }
                return packet;
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        if (packet.type === 'Text') {
                            return { type: 'Text', data: packet.data.toUpperCase() };
                        }
                        return packet;
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let packet = Packet::Text("hello world".into());
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet, Duration::from_secs(1), &mut stats).await;

        assert!(result.is_some());
        match result.unwrap() {
            Packet::Text(text) => assert_eq!(text.as_ref(), "HELLO WORLD"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[tokio::test]
    async fn test_packet_dropping() {
        let config = create_test_config(
            "function process(packet) {
                if (packet.data === 'drop') return null;
                return packet;
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        if (packet.data === 'drop') return null;
                        return packet;
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let packet = Packet::Text("drop".into());
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet, Duration::from_secs(1), &mut stats).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_audio_packet_metadata_only() {
        let config = create_test_config(
            "function process(packet) {
                console.log('Audio packet:', packet.type, packet.metadata);
                return packet;
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        console.log('Audio packet:', packet.type, packet.metadata);
                        return packet;
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let audio_frame = AudioFrame::new(48000, 2, vec![0.0; 960]);
        let packet = Packet::Audio(audio_frame.clone());
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet, Duration::from_secs(1), &mut stats).await;

        // Audio packets pass through unchanged (metadata accessible in JS)
        assert!(result.is_some());
        match result.unwrap() {
            Packet::Audio(frame) => {
                assert_eq!(frame.sample_rate, 48000);
                assert_eq!(frame.channels, 2);
            },
            _ => panic!("Expected Audio packet"),
        }
    }

    #[tokio::test]
    async fn test_transcription_packet_marshalling() {
        let config = create_test_config(
            "function process(packet) {
                if (packet.type === 'Transcription') {
                    console.log('Text:', packet.data.text);
                    console.log('Language:', packet.data.language);
                    console.log('Segments:', packet.data.segments.length);
                }
                return packet;
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        if (packet.type === 'Transcription') {
                            console.log('Text:', packet.data.text);
                            console.log('Language:', packet.data.language);
                            console.log('Segments:', packet.data.segments.length);
                        }
                        return packet;
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let transcription = TranscriptionData {
            text: "Hello world".to_string(),
            language: Some("en".to_string()),
            segments: vec![TranscriptionSegment {
                text: "Hello world".to_string(),
                start_time_ms: 0,
                end_time_ms: 1000,
                confidence: Some(0.95),
            }],
            metadata: None,
        };
        let packet = Packet::Transcription(Arc::new(transcription.clone()));
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet, Duration::from_secs(1), &mut stats).await;

        assert!(result.is_some());
        match result.unwrap() {
            Packet::Transcription(data) => {
                let data = data.as_ref();
                assert_eq!(data.text, "Hello world");
                assert_eq!(data.language.as_deref(), Some("en"));
                assert_eq!(data.segments.len(), 1);
            },
            _ => panic!("Expected Transcription packet"),
        }
    }

    #[tokio::test]
    async fn test_vad_event_marshalling() {
        let config = create_test_config(
            "function process(packet) {
                if (packet.type === 'Custom' && packet.type_id === 'plugin::native::vad/vad-event@1') {
                    console.log('Event:', packet.data.event_type, 'at', packet.data.timestamp_ms);
                }
                return packet;
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        if (packet.type === 'Custom' && packet.type_id === 'plugin::native::vad/vad-event@1') {
                            console.log('Event:', packet.data.event_type, 'at', packet.data.timestamp_ms);
                        }
                        return packet;
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let packet = Packet::Custom(Arc::new(CustomPacketData {
            type_id: TEST_VAD_EVENT_TYPE_ID.to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({
                "event_type": "speech_start",
                "timestamp_ms": 5000,
                "duration_ms": null
            }),
            metadata: None,
        }));
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet, Duration::from_secs(1), &mut stats).await;

        assert!(result.is_some());
        match result.unwrap() {
            Packet::Custom(custom) => {
                assert_eq!(custom.type_id, TEST_VAD_EVENT_TYPE_ID);
                assert_eq!(custom.encoding, CustomEncoding::Json);
                assert_eq!(
                    custom.data,
                    serde_json::json!({
                        "event_type": "speech_start",
                        "timestamp_ms": 5000,
                        "duration_ms": null
                    })
                );
            },
            _ => panic!("Expected Custom packet"),
        }
    }

    #[tokio::test]
    async fn test_script_error_passes_through() {
        let config = create_test_config(
            "function process(packet) {
                throw new Error('Intentional error');
            }",
        );
        let node = ScriptNode { config, global_config: None };

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        context
            .with(|ctx| {
                ctx.eval::<(), _>(
                    "function process(packet) {
                        throw new Error('Intentional error');
                    }",
                )?;
                Ok::<(), rquickjs::Error>(())
            })
            .await
            .unwrap();

        let packet = Packet::Text("test".into());
        let mut stats = NodeStatsTracker::new("test".to_string(), None);

        let result =
            node.process_packet(&context, packet.clone(), Duration::from_secs(1), &mut stats).await;

        // Error should result in pass-through
        assert!(result.is_some());
        match result.unwrap() {
            Packet::Text(text) => assert_eq!(text.as_ref(), "test"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[test]
    fn test_url_allowlist_empty_blocks_all() {
        let allowlist = vec![];
        assert!(!ScriptNode::is_url_allowed("https://example.com", "GET", &allowlist));
    }

    #[test]
    fn test_url_allowlist_exact_match() {
        let allowlist = vec![AllowlistRule {
            url: "https://api.example.com/data".to_string(),
            methods: vec!["GET".to_string()],
        }];

        assert!(ScriptNode::is_url_allowed("https://api.example.com/data", "GET", &allowlist));
        assert!(!ScriptNode::is_url_allowed("https://api.example.com/other", "GET", &allowlist));
    }

    #[test]
    fn test_url_allowlist_wildcard() {
        let allowlist = vec![AllowlistRule {
            url: "https://api.example.com/*".to_string(),
            methods: vec!["GET".to_string(), "POST".to_string()],
        }];

        assert!(ScriptNode::is_url_allowed("https://api.example.com/data", "GET", &allowlist));
        assert!(ScriptNode::is_url_allowed("https://api.example.com/users", "POST", &allowlist));
        assert!(!ScriptNode::is_url_allowed("https://api.example.com/data", "DELETE", &allowlist));
        assert!(!ScriptNode::is_url_allowed("https://other.example.com/data", "GET", &allowlist));
    }

    #[test]
    fn test_url_allowlist_method_case_insensitive() {
        let allowlist = vec![AllowlistRule {
            url: "https://api.example.com/*".to_string(),
            methods: vec!["GET".to_string()],
        }];

        assert!(ScriptNode::is_url_allowed("https://api.example.com/data", "get", &allowlist));
        assert!(ScriptNode::is_url_allowed("https://api.example.com/data", "Get", &allowlist));
    }

    #[test]
    fn test_secret_allowed_fetch_urls_empty_allows_any() {
        let secret = ScriptSecret { value: "x".to_string(), allowed_fetch_urls: vec![] };
        assert!(ScriptNode::is_secret_allowed_for_url(&secret, "https://example.com/path"));
    }

    #[test]
    fn test_secret_allowed_fetch_urls_scopes_destinations() {
        let secret = ScriptSecret {
            value: "x".to_string(),
            allowed_fetch_urls: vec!["https://api.openai.com/*".to_string()],
        };
        assert!(ScriptNode::is_secret_allowed_for_url(
            &secret,
            "https://api.openai.com/v1/chat/completions"
        ));
        assert!(!ScriptNode::is_secret_allowed_for_url(
            &secret,
            "https://uselessfacts.jsph.pl/api/v2/facts/random"
        ));
    }

    #[tokio::test]
    async fn test_async_function_support() {
        let script = r"
            async function process(packet) {
                // Simulate async operation by returning a Promise
                return new Promise((resolve) => {
                    resolve({
                        type: 'Text',
                        data: 'Async result: ' + packet.data
                    });
                });
            }
        ";

        let node = ScriptNode::new(
            Some(&serde_saphyr::from_str(&format!("script: |{script}")).unwrap()),
            None,
        )
        .unwrap();

        let runtime = rquickjs::AsyncRuntime::new().unwrap();
        let context = rquickjs::AsyncContext::full(&runtime).await.unwrap();

        node.validate_script(&context).await.unwrap();
        node.initialize_web_apis(&context).await.unwrap();

        // Test with a text packet
        let packet = Packet::Text("test input".into());

        let result = context
            .with(|ctx| {
                let js_packet = ScriptNode::packet_to_js(&packet, &ctx)?;
                let result = ScriptNode::execute_script(js_packet, &ctx)
                    .map_err(|e| StreamKitError::Runtime(e.to_string()))?;
                node.js_to_packet(result, &packet)
            })
            .await
            .unwrap();

        match result {
            Some(Packet::Text(text)) => {
                assert_eq!(text.as_ref(), "Async result: test input");
            },
            _ => panic!("Expected Text packet"),
        }
    }

    // Integration tests using full node lifecycle
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
    };
    use bytes::Bytes;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_script_node_full_lifecycle() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create script that transforms text
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                if (packet.type === 'Text') {
                  return { type: 'Text', data: packet.data.toUpperCase() };
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        // Verify state transitions
        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send test packets
        input_tx.send(Packet::Text("hello".into())).await.unwrap();
        input_tx.send(Packet::Text("world".into())).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify output
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 2, "Should have 2 output packets");

        match &output_packets[0] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "HELLO"),
            _ => panic!("Expected Text packet"),
        }
        match &output_packets[1] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "WORLD"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_packet_dropping() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that drops packets containing "drop"
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                if (packet.type === 'Text' && packet.data.includes('drop')) {
                  console.log('Dropping packet:', packet.data);
                  return null;
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send mix of packets
        input_tx.send(Packet::Text("keep this".into())).await.unwrap();
        input_tx.send(Packet::Text("drop this".into())).await.unwrap();
        input_tx.send(Packet::Text("keep that".into())).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify only 2 packets passed through
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 2, "Should have 2 packets (1 dropped)");

        match &output_packets[0] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "keep this"),
            _ => panic!("Expected Text packet"),
        }
        match &output_packets[1] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "keep that"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_error_passthrough() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that throws error
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                throw new Error('Intentional test error');
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let test_packet = Packet::Text("test".into());
        input_tx.send(test_packet.clone()).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify packet passed through unchanged
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);
        match &output_packets[0] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "test"),
            _ => panic!("Expected Text packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_multiple_packet_types() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that handles different packet types
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                console.log('Processing:', packet.type);

                if (packet.type === 'Text') {
                  return { type: 'Text', data: '[TEXT] ' + packet.data };
                }

                // Pass through other types
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send different packet types
        input_tx.send(Packet::Text("hello".into())).await.unwrap();

        let binary_packet = Packet::Binary {
            data: Bytes::from(vec![1, 2, 3]),
            content_type: Some(Cow::Borrowed("application/octet-stream")),
            metadata: None,
        };
        input_tx.send(binary_packet).await.unwrap();

        let transcription = TranscriptionData {
            text: "transcribed text".to_string(),
            language: Some("en".to_string()),
            segments: vec![],
            metadata: None,
        };
        input_tx.send(Packet::Transcription(Arc::new(transcription))).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify all packets processed
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 3);

        // Text modified
        match &output_packets[0] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "[TEXT] hello"),
            _ => panic!("Expected Text packet"),
        }

        // Binary passed through
        match &output_packets[1] {
            Packet::Binary { data, .. } => assert_eq!(data.as_ref(), &[1, 2, 3]),
            _ => panic!("Expected Binary packet"),
        }

        // Transcription passed through
        match &output_packets[2] {
            Packet::Transcription(t) => assert_eq!(t.as_ref().text, "transcribed text"),
            _ => panic!("Expected Transcription packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_transcription_processing() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that accesses transcription data
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                if (packet.type === 'Transcription') {
                  console.log('Transcription:', packet.data.text);
                  console.log('Language:', packet.data.language);
                  console.log('Segments:', packet.data.segments.length);
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let transcription = TranscriptionData {
            text: "Hello world".to_string(),
            language: Some("en".to_string()),
            segments: vec![
                TranscriptionSegment {
                    text: "Hello".to_string(),
                    start_time_ms: 0,
                    end_time_ms: 500,
                    confidence: Some(0.95),
                },
                TranscriptionSegment {
                    text: "world".to_string(),
                    start_time_ms: 500,
                    end_time_ms: 1000,
                    confidence: Some(0.98),
                },
            ],
            metadata: None,
        };
        input_tx.send(Packet::Transcription(Arc::new(transcription.clone()))).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        match &output_packets[0] {
            Packet::Transcription(t) => {
                let t = t.as_ref();
                assert_eq!(t.text, "Hello world");
                assert_eq!(t.segments.len(), 2);
                assert_eq!(t.language.as_deref(), Some("en"));
            },
            _ => panic!("Expected Transcription packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_audio_metadata() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that checks audio metadata
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                if (packet.type === 'Audio') {
                  const meta = packet.metadata;
                  console.log('Sample rate:', meta.sample_rate);
                  console.log('Channels:', meta.channels);
                  console.log('Frames:', meta.frames);
                  console.log('Duration:', meta.duration_ms, 'ms');
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        let audio_frame = AudioFrame::new(48000, 2, vec![0.0; 960]);
        input_tx.send(Packet::Audio(audio_frame)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        match &output_packets[0] {
            Packet::Audio(frame) => {
                assert_eq!(frame.sample_rate, 48000);
                assert_eq!(frame.channels, 2);
                assert_eq!(frame.samples.len(), 960);
            },
            _ => panic!("Expected Audio packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_vad_event_handling() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script that processes custom VAD events
        let config = serde_saphyr::from_str(
            r"
            script: |
              function process(packet) {
                if (packet.type === 'Custom' && packet.type_id === 'plugin::native::vad/vad-event@1') {
                  console.log('VAD Event:', packet.data.event_type);
                  console.log('Timestamp:', packet.data.timestamp_ms);
                  if (packet.data.duration_ms) console.log('Duration:', packet.data.duration_ms);
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send speech start event
        input_tx
            .send(Packet::Custom(Arc::new(CustomPacketData {
                type_id: TEST_VAD_EVENT_TYPE_ID.to_string(),
                encoding: CustomEncoding::Json,
                data: serde_json::json!({
                    "event_type": "speech_start",
                    "timestamp_ms": 1000,
                    "duration_ms": null
                }),
                metadata: Some(PacketMetadata {
                    timestamp_us: Some(1_000_000),
                    duration_us: None,
                    sequence: None,
                }),
            })))
            .await
            .unwrap();

        // Send speech end event
        input_tx
            .send(Packet::Custom(Arc::new(CustomPacketData {
                type_id: TEST_VAD_EVENT_TYPE_ID.to_string(),
                encoding: CustomEncoding::Json,
                data: serde_json::json!({
                    "event_type": "speech_end",
                    "timestamp_ms": 3000,
                    "duration_ms": 2000
                }),
                metadata: Some(PacketMetadata {
                    timestamp_us: Some(3_000_000),
                    duration_us: None,
                    sequence: None,
                }),
            })))
            .await
            .unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 2);

        match &output_packets[0] {
            Packet::Custom(custom) => {
                assert_eq!(custom.type_id, TEST_VAD_EVENT_TYPE_ID);
                assert_eq!(
                    custom.data,
                    serde_json::json!({
                        "event_type": "speech_start",
                        "timestamp_ms": 1000,
                        "duration_ms": null
                    })
                );
            },
            _ => panic!("Expected Custom packet"),
        }

        match &output_packets[1] {
            Packet::Custom(custom) => {
                assert_eq!(custom.type_id, TEST_VAD_EVENT_TYPE_ID);
                assert_eq!(
                    custom.data,
                    serde_json::json!({
                        "event_type": "speech_end",
                        "timestamp_ms": 3000,
                        "duration_ms": 2000
                    })
                );
            },
            _ => panic!("Expected Custom packet"),
        }
    }

    #[tokio::test]
    async fn test_script_node_async_function() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Script with async function
        let config = serde_saphyr::from_str(
            r"
            script: |
              async function process(packet) {
                if (packet.type === 'Text') {
                  // Simulate async work with Promise
                  return new Promise((resolve) => {
                    resolve({
                      type: 'Text',
                      data: 'Async: ' + packet.data
                    });
                  });
                }
                return packet;
              }
            ",
        )
        .unwrap();

        let node = ScriptNode::new(Some(&config), None).unwrap();
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        input_tx.send(Packet::Text("test".into())).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        match &output_packets[0] {
            Packet::Text(text) => assert_eq!(text.as_ref(), "Async: test"),
            _ => panic!("Expected Text packet"),
        }
    }
}
