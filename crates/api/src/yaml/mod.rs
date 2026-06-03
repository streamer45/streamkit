// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! YAML pipeline format parsing and compilation.

use super::{ConnectionMode, EngineMode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Deserialize)]
pub struct Step {
    pub kind: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UserNode {
    pub kind: String,
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub needs: Needs,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NeedsDependency {
    Simple(String),
    WithMode {
        node: String,
        #[serde(default)]
        mode: ConnectionMode,
    },
}

impl NeedsDependency {
    fn node(&self) -> &str {
        match self {
            Self::Simple(s) => s,
            Self::WithMode { node, .. } => node,
        }
    }

    fn node_and_pin(&self) -> (&str, Option<&str>) {
        let label = self.node();
        if let Some((node, pin)) = label.split_once('.') {
            (node, Some(pin))
        } else {
            (label, None)
        }
    }

    fn mode(&self) -> ConnectionMode {
        match self {
            Self::Simple(_) => ConnectionMode::default(),
            Self::WithMode { mode, .. } => *mode,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum Needs {
    #[default]
    None,
    Single(NeedsDependency),
    Multiple(Vec<NeedsDependency>),
    /// Keys are target input pin names.  Avoid `"node"` as a pin name —
    /// `#[serde(untagged)]` will parse it as `Single(WithMode)` instead.
    Map(IndexMap<String, NeedsDependency>),
}

/// Dynamic pipelines use `publish`/`watch`; oneshot uses `input`/`output`.
/// Mutual exclusivity enforced by the lint pass, not at parse time.
#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ClientSection {
    pub relay_url: Option<String>,
    pub gateway_path: Option<String>,
    pub publish: Option<PublishConfig>,
    pub watch: Option<WatchConfig>,
    pub input: Option<InputConfig>,
    pub output: Option<OutputConfig>,
    #[serde(default)]
    pub controls: Option<Vec<ControlConfig>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct PublishConfig {
    pub broadcast: String,
    pub tracks: Vec<PublishTrackConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct PublishTrackConfig {
    pub kind: TrackKind,
    pub source: CaptureSource,
    #[serde(default)]
    pub broadcast: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// String (not enum) for forward compatibility — new codecs don't require schema changes.
    #[serde(default)]
    pub codec: Option<String>,
    /// Kilobits per second.
    #[serde(default)]
    pub max_bitrate: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Camera,
    Screen,
    Microphone,
}

impl std::fmt::Display for TrackKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrackKind::Audio => write!(f, "audio"),
            TrackKind::Video => write!(f, "video"),
        }
    }
}

impl std::fmt::Display for CaptureSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureSource::Camera => write!(f, "camera"),
            CaptureSource::Screen => write!(f, "screen"),
            CaptureSource::Microphone => write!(f, "microphone"),
        }
    }
}

/// MoQ (`broadcast`) and/or MSE (`mse_path`); both can be set simultaneously.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct WatchConfig {
    #[serde(default)]
    pub broadcast: Option<String>,
    /// Path suffix; browser fetches from `/mse/{session_id}{mse_path}`.
    #[serde(default)]
    pub mse_path: Option<String>,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub video: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct InputConfig {
    #[serde(rename = "type")]
    pub input_type: InputType,
    pub accept: Option<String>,
    pub asset_tags: Option<Vec<String>>,
    pub placeholder: Option<String>,
    #[ts(type = "Record<string, FieldHint> | null")]
    pub field_hints: Option<IndexMap<String, FieldHint>>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    FileUpload,
    Text,
    /// Body is irrelevant; presence of `http_input` is the trigger.
    Trigger,
    /// No `http_input` node.
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct OutputConfig {
    #[serde(rename = "type")]
    pub output_type: OutputType,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Transcription,
    Json,
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    File,
    Text,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct FieldHint {
    #[serde(rename = "type")]
    pub field_type: Option<FieldType>,
    pub accept: Option<String>,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    Toggle,
    Text,
    Number,
    Button,
    Select,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct SelectOption {
    pub label: String,
    #[ts(type = "unknown")]
    pub value: serde_json::Value,
}

/// `property` uses dot-notation (e.g. `"properties.home_score"`);
/// the frontend builds the nested JSON payload from it.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ControlConfig {
    pub label: String,
    #[serde(rename = "type")]
    pub control_type: ControlType,
    pub node: String,
    pub property: String,
    #[serde(default)]
    pub group: Option<String>,
    /// UI-only hint; not sent to server on mount.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    #[ts(type = "unknown")]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub options: Option<Vec<SelectOption>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum UserPipeline {
    Steps {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        #[serde(default)]
        attributes: Option<std::collections::BTreeMap<String, String>>,
        steps: Vec<Step>,
        client: Option<ClientSection>,
    },
    Dag {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        #[serde(default)]
        attributes: Option<std::collections::BTreeMap<String, String>>,
        nodes: IndexMap<String, UserNode>,
        client: Option<ClientSection>,
    },
}

/// Two-step parse (YAML → JSON → `UserPipeline`) works around a
/// `serde_saphyr` limitation with deeply nested `#[serde(untagged)]` enums.
pub fn parse_yaml(yaml: &str) -> Result<UserPipeline, String> {
    let json_value: serde_json::Value =
        serde_saphyr::from_str(yaml).map_err(|e| format!("Invalid YAML: {e}"))?;

    // Pre-validate `client` if present so that enum/type errors produce
    // actionable messages instead of collapsing into the generic
    // "did not match any variant" error from the untagged `UserPipeline`.
    if let Some(client_val) = json_value.get("client") {
        let _: ClientSection = serde_json::from_value(client_val.clone())
            .map_err(|e| format!("Invalid client section: {e}"))?;
    }

    serde_json::from_value(json_value).map_err(|e| format!("Invalid pipeline: {e}"))
}

mod client_lint;
mod compiler;

#[cfg(test)]
mod tests;

pub use client_lint::{
    lint_client_against_nodes, lint_client_section, ClientLintWarning, NodeInfo,
};
pub use compiler::compile;
