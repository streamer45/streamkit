// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use url::Url;

#[derive(Debug, Deserialize, Clone)]
pub struct LoadTestConfig {
    pub server: ServerConfig,
    pub test: TestConfig,
    pub oneshot: OneShotConfig,
    pub dynamic: DynamicConfig,
    pub populate: PopulateConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TestConfig {
    pub duration_secs: u64,
    pub scenario: Scenario,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Scenario {
    #[serde(rename = "oneshot")]
    OneShot,
    Dynamic,
    Mixed,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OneShotConfig {
    pub enabled: bool,
    pub concurrency: usize,
    pub pipeline: String,
    pub input_file: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DynamicConfig {
    pub enabled: bool,
    pub session_count: usize,
    pub tune_interval_ms: u64,
    pub pipelines: Vec<String>,
    /// Optional broadcaster configuration for MoQ load testing.
    /// When set, broadcaster sessions are created first to publish audio
    /// to the MoQ relay before subscriber sessions are created.
    #[serde(default)]
    pub broadcaster: Option<BroadcasterConfig>,
}

/// Configuration for MoQ broadcaster sessions in load testing.
/// Broadcasters publish audio to a MoQ relay so that subscriber
/// pipelines have data to receive.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct BroadcasterConfig {
    /// Path to the broadcaster pipeline YAML file
    pub pipeline: String,
    /// Number of broadcaster sessions to create (default: 1)
    #[serde(default = "default_broadcaster_count")]
    pub count: usize,
}

const fn default_broadcaster_count() -> usize {
    1
}

#[derive(Debug, Deserialize, Clone)]
pub struct PopulateConfig {
    pub load_plugins: bool,
    #[serde(default)]
    pub plugins_native: Vec<String>,
    #[serde(default)]
    pub plugins_wasm: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OutputConfig {
    pub format: OutputFormat,
    pub real_time_updates: bool,
    pub update_interval_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Json,
    Csv,
}

impl LoadTestConfig {
    /// Load configuration from a TOML file
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be read
    /// - The TOML content is invalid
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;

        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML config: {}", path.as_ref().display()))?;

        Ok(config)
    }

    /// Validate config consistency and referenced files.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is inconsistent (e.g. scenario enabled but missing
    /// required fields) or if referenced local files do not exist.
    pub fn validate(&self) -> Result<()> {
        let server_url = self.server.url.trim();
        if server_url.is_empty() {
            bail!("server.url must not be empty");
        }

        let parsed = Url::parse(server_url)
            .with_context(|| format!("server.url is not a valid URL: {server_url}"))?;
        match parsed.scheme() {
            "http" | "https" => {},
            scheme => bail!("server.url must be http(s), got: {scheme}"),
        }

        if self.output.real_time_updates && self.output.update_interval_ms == 0 {
            bail!("output.update_interval_ms must be > 0 when output.real_time_updates = true");
        }

        match self.test.scenario {
            Scenario::OneShot => {
                if !self.oneshot.enabled {
                    bail!("test.scenario=oneshot requires oneshot.enabled=true");
                }
                if self.oneshot.concurrency == 0 {
                    bail!("oneshot.concurrency must be > 0");
                }
                if self.oneshot.pipeline.trim().is_empty() {
                    bail!("oneshot.pipeline must not be empty");
                }
                if self.oneshot.input_file.trim().is_empty() {
                    bail!("oneshot.input_file must not be empty");
                }
                if !Path::new(&self.oneshot.pipeline).exists() {
                    bail!("oneshot.pipeline does not exist: {}", self.oneshot.pipeline);
                }
                if !Path::new(&self.oneshot.input_file).exists() {
                    bail!("oneshot.input_file does not exist: {}", self.oneshot.input_file);
                }
            },
            Scenario::Dynamic => {
                if !self.dynamic.enabled {
                    bail!("test.scenario=dynamic requires dynamic.enabled=true");
                }
                if self.dynamic.session_count == 0 {
                    bail!("dynamic.session_count must be > 0");
                }
                if self.dynamic.tune_interval_ms == 0 {
                    bail!("dynamic.tune_interval_ms must be > 0");
                }
                if self.dynamic.pipelines.is_empty() {
                    bail!("dynamic.pipelines must not be empty");
                }
                for pipeline in &self.dynamic.pipelines {
                    if pipeline.trim().is_empty() {
                        bail!("dynamic.pipelines contains an empty path");
                    }
                    if !Path::new(pipeline).exists() {
                        bail!("dynamic.pipelines entry does not exist: {pipeline}");
                    }
                }
                // Validate broadcaster config if present
                if let Some(ref broadcaster) = self.dynamic.broadcaster {
                    if broadcaster.pipeline.trim().is_empty() {
                        bail!("dynamic.broadcaster.pipeline must not be empty");
                    }
                    if !Path::new(&broadcaster.pipeline).exists() {
                        bail!(
                            "dynamic.broadcaster.pipeline does not exist: {}",
                            broadcaster.pipeline
                        );
                    }
                    if broadcaster.count == 0 {
                        bail!("dynamic.broadcaster.count must be > 0");
                    }
                }
            },
            Scenario::Mixed => {
                if !self.oneshot.enabled || !self.dynamic.enabled {
                    bail!("test.scenario=mixed requires oneshot.enabled=true and dynamic.enabled=true");
                }
                if self.oneshot.concurrency == 0 {
                    bail!("oneshot.concurrency must be > 0");
                }
                if self.dynamic.session_count == 0 {
                    bail!("dynamic.session_count must be > 0");
                }
                if self.dynamic.tune_interval_ms == 0 {
                    bail!("dynamic.tune_interval_ms must be > 0");
                }
                if self.dynamic.pipelines.is_empty() {
                    bail!("dynamic.pipelines must not be empty");
                }

                if self.oneshot.pipeline.trim().is_empty() {
                    bail!("oneshot.pipeline must not be empty");
                }
                if self.oneshot.input_file.trim().is_empty() {
                    bail!("oneshot.input_file must not be empty");
                }
                if !Path::new(&self.oneshot.pipeline).exists() {
                    bail!("oneshot.pipeline does not exist: {}", self.oneshot.pipeline);
                }
                if !Path::new(&self.oneshot.input_file).exists() {
                    bail!("oneshot.input_file does not exist: {}", self.oneshot.input_file);
                }

                for pipeline in &self.dynamic.pipelines {
                    if pipeline.trim().is_empty() {
                        bail!("dynamic.pipelines contains an empty path");
                    }
                    if !Path::new(pipeline).exists() {
                        bail!("dynamic.pipelines entry does not exist: {pipeline}");
                    }
                }
                // Validate broadcaster config if present
                if let Some(ref broadcaster) = self.dynamic.broadcaster {
                    if broadcaster.pipeline.trim().is_empty() {
                        bail!("dynamic.broadcaster.pipeline must not be empty");
                    }
                    if !Path::new(&broadcaster.pipeline).exists() {
                        bail!(
                            "dynamic.broadcaster.pipeline does not exist: {}",
                            broadcaster.pipeline
                        );
                    }
                    if broadcaster.count == 0 {
                        bail!("dynamic.broadcaster.count must be > 0");
                    }
                }
            },
        }

        Ok(())
    }
}
