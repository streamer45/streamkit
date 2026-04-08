// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Object store write node — streams binary data to S3-compatible object storage.
//!
//! Uses [Apache OpenDAL](https://opendal.apache.org/) to support S3, GCS,
//! Azure Blob, MinIO, RustFS, and other compatible backends.
//!
//! Incoming [`Packet::Binary`] packets are buffered up to `chunk_size` and
//! written via OpenDAL's multipart [`Writer`](opendal::Writer), keeping memory
//! bounded regardless of the total upload size.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default buffer/chunk size: 5 MiB (the S3 minimum multipart part size).
const DEFAULT_CHUNK_SIZE: usize = 5 * 1024 * 1024;

const fn default_chunk_size() -> usize {
    DEFAULT_CHUNK_SIZE
}

fn default_region() -> String {
    "us-east-1".to_string()
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the object store write node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectStoreWriteConfig {
    /// S3-compatible endpoint URL.
    ///
    /// Examples:
    /// - AWS S3: `https://s3.amazonaws.com`
    /// - MinIO / RustFS: `http://localhost:9000`
    /// - Cloudflare R2: `https://<account>.r2.cloudflarestorage.com`
    pub endpoint: String,

    /// Bucket name.
    pub bucket: String,

    /// Object key (path within the bucket).
    pub key: String,

    /// AWS region (default: `us-east-1`).
    ///
    /// Most S3-compatible services accept any region string; set this to
    /// match the bucket's actual region for AWS S3.
    #[serde(default = "default_region")]
    pub region: String,

    /// Access key ID.
    ///
    /// If omitted, the node falls back to `access_key_id_env`.
    #[serde(default)]
    pub access_key_id: Option<String>,

    /// Environment variable name containing the access key ID.
    ///
    /// Read at node startup.  Takes precedence over `access_key_id`.
    #[serde(default)]
    pub access_key_id_env: Option<String>,

    /// Secret access key.
    ///
    /// If omitted, the node falls back to `secret_key_env`.
    #[serde(default)]
    pub secret_access_key: Option<String>,

    /// Environment variable name containing the secret access key.
    ///
    /// Read at node startup.  Takes precedence over `secret_access_key`.
    #[serde(default)]
    pub secret_key_env: Option<String>,

    /// Buffer size before flushing to the object store (default: 5 MiB).
    ///
    /// This controls the multipart upload part size.  S3 requires a minimum
    /// part size of 5 MiB (except the last part).
    #[serde(default = "default_chunk_size")]
    #[schemars(range(min = 1))]
    pub chunk_size: usize,

    /// Optional MIME content type for the uploaded object
    /// (e.g. `audio/ogg`, `video/mp4`).
    #[serde(default)]
    pub content_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Credential helpers
// ---------------------------------------------------------------------------

/// Resolve a credential value from an env-var name (takes precedence) or a
/// literal fallback.
fn resolve_credential(
    env_name: Option<&str>,
    literal: Option<&str>,
    label: &str,
) -> Result<String, StreamKitError> {
    if let Some(env) = env_name {
        match std::env::var(env) {
            Ok(val) if !val.is_empty() => {
                tracing::debug!("Resolved {label} from env var {env}");
                return Ok(val);
            },
            Ok(_) => {
                return Err(StreamKitError::Configuration(format!(
                    "Environment variable '{env}' for {label} is empty"
                )));
            },
            Err(_) => {
                return Err(StreamKitError::Configuration(format!(
                    "Environment variable '{env}' for {label} is not set"
                )));
            },
        }
    }
    if let Some(val) = literal {
        if val.is_empty() {
            return Err(StreamKitError::Configuration(format!("{label} is empty")));
        }
        return Ok(val.to_string());
    }
    Err(StreamKitError::Configuration(format!("No {label} provided (set via config or env var)")))
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// Sink node that streams [`Packet::Binary`] data to S3-compatible object
/// storage via OpenDAL's multipart upload.
pub struct ObjectStoreWriteNode {
    config: ObjectStoreWriteConfig,
}

impl ObjectStoreWriteNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: ObjectStoreWriteConfig = if params.is_none() {
                // Default config for pin inspection only (dynamic registration)
                ObjectStoreWriteConfig {
                    endpoint: String::new(),
                    bucket: String::new(),
                    key: String::new(),
                    region: default_region(),
                    access_key_id: None,
                    access_key_id_env: None,
                    secret_access_key: None,
                    secret_key_env: None,
                    chunk_size: default_chunk_size(),
                    content_type: None,
                }
            } else {
                config_helpers::parse_config_required(params)?
            };

            if config.chunk_size == 0 {
                return Err(StreamKitError::Configuration(
                    "chunk_size must be greater than 0".to_string(),
                ));
            }

            Ok(Box::new(Self { config }))
        })
    }
}

// ---------------------------------------------------------------------------
// ProcessorNode implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ProcessorNode for ObjectStoreWriteNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        // Sink — no outputs.
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // ── Resolve credentials ──────────────────────────────────────────
        let access_key = resolve_credential(
            self.config.access_key_id_env.as_deref(),
            self.config.access_key_id.as_deref(),
            "access_key_id",
        )
        .inspect_err(|e| {
            state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
        })?;

        let secret_key = resolve_credential(
            self.config.secret_key_env.as_deref(),
            self.config.secret_access_key.as_deref(),
            "secret_access_key",
        )
        .inspect_err(|e| {
            state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
        })?;

        tracing::info!(
            %node_name,
            endpoint = %self.config.endpoint,
            bucket = %self.config.bucket,
            key = %self.config.key,
            region = %self.config.region,
            chunk_size = self.config.chunk_size,
            "ObjectStoreWriteNode initializing"
        );

        // ── Build OpenDAL operator ───────────────────────────────────────
        let operator = {
            let mut cfg = std::collections::HashMap::new();
            cfg.insert("bucket".to_string(), self.config.bucket.clone());
            cfg.insert("endpoint".to_string(), self.config.endpoint.clone());
            cfg.insert("region".to_string(), self.config.region.clone());
            cfg.insert("access_key_id".to_string(), access_key);
            cfg.insert("secret_access_key".to_string(), secret_key);
            // Disable credential loading from environment/instance metadata —
            // we resolve credentials explicitly above.
            cfg.insert("disable_config_load".to_string(), "true".to_string());

            opendal::Operator::from_iter::<opendal::services::S3>(cfg)
                .map_err(|e| {
                    let msg = format!("Failed to build S3 operator: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
                    StreamKitError::Runtime(msg)
                })?
                .finish()
        };

        tracing::info!(%node_name, "S3 operator created, opening writer");

        // ── Open writer (multipart upload) ───────────────────────────────
        let writer_future = operator.writer_with(&self.config.key).chunk(self.config.chunk_size);

        // Apply content type if configured.
        let mut writer = if let Some(ref ct) = self.config.content_type {
            writer_future.content_type(ct).await
        } else {
            writer_future.await
        }
        .map_err(|e| {
            let msg = format!("Failed to open S3 writer for '{}': {e}", self.config.key);
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(
            %node_name,
            key = %self.config.key,
            "S3 multipart writer opened, entering receive loop"
        );

        state_helpers::emit_running(&context.state_tx, &node_name);

        // ── Receive loop ─────────────────────────────────────────────────
        let mut input_rx = context.take_input("in")?;
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let mut packet_count: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut buffer = Vec::with_capacity(self.config.chunk_size);
        let mut chunks_written: u64 = 0;
        let mut reason = "input_closed".to_string();

        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            if let Packet::Binary { data, .. } = packet {
                stats_tracker.received();
                packet_count += 1;
                total_bytes += data.len() as u64;

                buffer.extend_from_slice(&data);

                // Flush when buffer reaches chunk_size
                while buffer.len() >= self.config.chunk_size {
                    let chunk: Vec<u8> = buffer.drain(..self.config.chunk_size).collect();
                    if let Err(e) = writer.write(chunk).await {
                        stats_tracker.errored();
                        stats_tracker.force_send();
                        let msg = format!("S3 write error: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
                        // Attempt to abort the multipart upload to clean up
                        if let Err(abort_err) = writer.abort().await {
                            tracing::error!(
                                %node_name,
                                error = %abort_err,
                                "Failed to abort S3 multipart upload after write error"
                            );
                        }
                        return Err(StreamKitError::Runtime(msg));
                    }
                    chunks_written += 1;
                    tracing::debug!(
                        %node_name,
                        chunks_written,
                        total_bytes,
                        "Flushed chunk to S3"
                    );
                }

                stats_tracker.sent();
                stats_tracker.maybe_send();
            } else {
                tracing::warn!(
                    %node_name,
                    "Received non-Binary packet, ignoring"
                );
                stats_tracker.discarded();
            }
        }

        // ── Flush remaining buffer ───────────────────────────────────────
        if !buffer.is_empty() {
            tracing::debug!(
                %node_name,
                remaining = buffer.len(),
                "Flushing remaining buffer to S3"
            );
            if let Err(e) = writer.write(buffer).await {
                stats_tracker.errored();
                stats_tracker.force_send();
                let msg = format!("S3 write error (final flush): {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
                if let Err(abort_err) = writer.abort().await {
                    tracing::error!(
                        %node_name,
                        error = %abort_err,
                        "Failed to abort S3 multipart upload after final flush error"
                    );
                }
                return Err(StreamKitError::Runtime(msg));
            }
            chunks_written += 1;
        }

        // ── Close (finalize multipart upload) ────────────────────────────
        tracing::info!(
            %node_name,
            "Closing S3 writer (finalizing multipart upload)"
        );
        if let Err(e) = writer.close().await {
            stats_tracker.errored();
            stats_tracker.force_send();
            let msg = format!("Failed to finalize S3 upload: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            reason = format!("close_failed: {e}");
            state_helpers::emit_stopped(&context.state_tx, &node_name, reason);
            return Err(StreamKitError::Runtime(msg));
        }

        stats_tracker.force_send();
        tracing::info!(
            %node_name,
            packet_count,
            total_bytes,
            chunks_written,
            key = %self.config.key,
            "ObjectStoreWriteNode finished uploading to S3"
        );

        state_helpers::emit_stopped(&context.state_tx, &node_name, reason);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use streamkit_core::node::RoutedPacketMessage;
    use streamkit_core::NodeStatsUpdate;
    use tokio::sync::mpsc;

    /// Verify pin definitions for the object store write node.
    #[test]
    fn test_pin_definitions() {
        let node = ObjectStoreWriteNode {
            config: ObjectStoreWriteConfig {
                endpoint: String::new(),
                bucket: String::new(),
                key: String::new(),
                region: default_region(),
                access_key_id: None,
                access_key_id_env: None,
                secret_access_key: None,
                secret_key_env: None,
                chunk_size: default_chunk_size(),
                content_type: None,
            },
        };

        let inputs = node.input_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "in");
        assert_eq!(inputs[0].accepts_types, vec![PacketType::Binary]);

        let outputs = node.output_pins();
        assert!(outputs.is_empty(), "Sink node should have no output pins");
    }

    /// Verify factory rejects zero chunk_size.
    #[test]
    fn test_factory_rejects_zero_chunk_size() {
        let factory = ObjectStoreWriteNode::factory();
        let params = serde_json::json!({
            "endpoint": "http://localhost:9000",
            "bucket": "test",
            "key": "test.bin",
            "chunk_size": 0,
        });
        let result = factory(Some(&params));
        assert!(result.is_err());
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error for zero chunk_size"),
        };
        assert!(err.contains("chunk_size"), "Error should mention chunk_size: {err}");
    }

    /// Verify credential resolution logic.
    #[test]
    fn test_resolve_credential_literal() {
        let result = resolve_credential(None, Some("my-key"), "test");
        assert_eq!(result.unwrap(), "my-key");
    }

    #[test]
    fn test_resolve_credential_empty_literal() {
        let result = resolve_credential(None, Some(""), "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_credential_missing() {
        let result = resolve_credential(None, None, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_credential_env_precedence() {
        // Use a unique env var name to avoid conflicts with parallel tests
        let env_name = "_SK_TEST_OBJSTORE_CRED_PREC";
        std::env::set_var(env_name, "from-env");
        let result = resolve_credential(Some(env_name), Some("from-literal"), "test");
        assert_eq!(result.unwrap(), "from-env");
        std::env::remove_var(env_name);
    }

    #[test]
    fn test_resolve_credential_env_not_set() {
        let result = resolve_credential(Some("_SK_TEST_OBJSTORE_NONEXISTENT_VAR"), None, "test");
        assert!(result.is_err());
    }

    /// Verify that the node emits the correct state transitions and handles
    /// credential failures gracefully (without needing a real S3 endpoint).
    #[tokio::test]
    async fn test_node_fails_on_missing_credentials() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let (mock_sender, _packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_objstore_write".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs,
            input_types: HashMap::new(),
            control_rx,
            output_sender,
            batch_size: 32,
            state_tx,
            stats_tx: Some(stats_tx),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: None,
            pin_management_rx: None,
            audio_pool: None,
            video_pool: None,
            pipeline_mode: streamkit_core::PipelineMode::Dynamic,
            view_data_tx: None,
        };

        // No credentials provided — should fail during init
        let config = ObjectStoreWriteConfig {
            endpoint: "http://localhost:9000".to_string(),
            bucket: "test-bucket".to_string(),
            key: "test/output.bin".to_string(),
            region: default_region(),
            access_key_id: None,
            access_key_id_env: None,
            secret_access_key: None,
            secret_key_env: None,
            chunk_size: default_chunk_size(),
            content_type: None,
        };
        let node = Box::new(ObjectStoreWriteNode { config });

        // Keep input_tx alive until after the node is checked
        let _keep_alive = input_tx;

        let result = node.run(context).await;
        assert!(result.is_err(), "Node should fail when credentials are missing");

        // Should have emitted Initializing then Failed
        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Initializing));

        let state = state_rx.recv().await.unwrap();
        assert!(matches!(state.state, streamkit_core::NodeState::Failed { .. }));
    }
}
