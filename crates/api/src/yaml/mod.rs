// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! YAML pipeline format parsing and compilation.
//!
//! This module provides user-friendly YAML formats that compile to the internal Pipeline representation.
//! Supports two formats:
//! - **Steps**: Linear pipeline (`steps: [...]`)
//! - **DAG**: Directed acyclic graph (`nodes: {...}` with `needs: [...]` dependencies)

use super::{ConnectionMode, EngineMode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Represents a single step in a linear pipeline definition.
#[derive(Debug, Deserialize)]
pub struct Step {
    pub kind: String,
    pub params: Option<serde_json::Value>,
}

/// Represents a single node in a user-facing DAG pipeline definition.
#[derive(Debug, Deserialize)]
pub struct UserNode {
    pub kind: String,
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub needs: Needs,
}

/// A single dependency with optional connection mode.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NeedsDependency {
    /// Simple string: just the node name (mode defaults to Reliable)
    Simple(String),
    /// Object with node name and optional mode
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

    /// Returns (node, from_pin) where from_pin is parsed from "node.pin" syntax if present.
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

/// Represents the `needs` field for DAG nodes.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum Needs {
    #[default]
    None,
    Single(NeedsDependency),
    Multiple(Vec<NeedsDependency>),
    /// Map variant: keys are **target input pin names**.
    /// Enables explicit pin targeting, e.g.
    /// ```yaml
    /// needs:
    ///   video: vp9_encoder
    ///   audio: opus_encoder
    /// ```
    ///
    /// **Note:** Because `Needs` uses `#[serde(untagged)]`, a single-entry
    /// map whose key is `"node"` (with an optional `"mode"` key matching a
    /// valid [`ConnectionMode`]) will be parsed as `Single(WithMode)` rather
    /// than `Map`.  Avoid using `node` as a pin name.
    Map(IndexMap<String, NeedsDependency>),
}

/// Top-level `client` section in pipeline YAML.
///
/// Declares what the browser UI should do when rendering this pipeline.
/// Dynamic pipelines use `relay_url`/`gateway_path`/`publish`/`watch`;
/// oneshot pipelines use `input`/`output`.  The two sets are mutually
/// exclusive by mode (enforced by the lint pass, not at parse time).
#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ClientSection {
    /// Direct relay URL for external MoQ relay pattern.
    pub relay_url: Option<String>,
    /// Gateway path for gateway-managed MoQ pattern.
    pub gateway_path: Option<String>,
    /// Browser-side publish configuration (dynamic pipelines).
    pub publish: Option<PublishConfig>,
    /// Browser-side watch configuration (dynamic pipelines).
    /// Supports MoQ (via `broadcast`) and/or MSE (via `mse_path`) output.
    pub watch: Option<WatchConfig>,
    /// Input UX configuration (oneshot pipelines).
    pub input: Option<InputConfig>,
    /// Output rendering configuration (oneshot pipelines).
    pub output: Option<OutputConfig>,
    /// Declarative overlay controls for runtime node tuning (dynamic pipelines).
    #[serde(default)]
    pub controls: Option<Vec<ControlConfig>>,
}

/// Browser-side publish configuration for dynamic pipelines.
///
/// Uses a generic `tracks` array where each entry declares a media source
/// the browser should capture and publish.  Tracks are grouped by their
/// effective broadcast name (track-level override or top-level default)
/// and each group becomes a separate `Publish.Broadcast` instance.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct PublishConfig {
    /// Default broadcast name for all tracks.
    pub broadcast: String,
    /// Media tracks to capture and publish.
    pub tracks: Vec<PublishTrackConfig>,
}

/// A single media track to capture and publish from the browser.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct PublishTrackConfig {
    /// Media kind: "audio" or "video"
    pub kind: TrackKind,
    /// Capture source: "camera", "screen", "microphone"
    pub source: CaptureSource,
    /// Override broadcast name for this track.
    /// When omitted, uses the parent `PublishConfig.broadcast`.
    #[serde(default)]
    pub broadcast: Option<String>,
    /// Desired encode width in pixels (e.g. 1280).
    /// Applied as a capture constraint and used to compute `maxPixels` for
    /// the `@moq/publish` video encoder.
    #[serde(default)]
    pub width: Option<u32>,
    /// Desired encode height in pixels (e.g. 720).
    #[serde(default)]
    pub height: Option<u32>,
    /// Codec identifier.  Supported values: `"vp9"` (video), `"opus"` (audio).
    /// Defaults to `"vp9"` for video tracks when omitted.
    ///
    /// Stored as `String` (rather than an enum) for forward compatibility — new
    /// codecs can be added without a breaking schema change.  The Rust linter
    /// and the TS `mapCodecToWebCodecs` function validate the value at compile
    /// and runtime respectively.
    #[serde(default)]
    pub codec: Option<String>,
    /// Maximum bitrate in kilobits per second (1 kbps = 1000 bps).  For video
    /// tracks, converted to bps and passed as `maxBitrate` to the `@moq/publish`
    /// encoder.  Audio track bitrate is parsed and validated but not yet wired
    /// to the audio encoder.
    #[serde(default)]
    pub max_bitrate: Option<u32>,
}

/// Media kind for a publish track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Audio,
    Video,
}

/// Capture source for a publish track.
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

/// Browser-side watch configuration for dynamic pipelines.
///
/// Supports two output transports:
/// - **MoQ** (WebTransport): set `broadcast` to subscribe via `@moq/watch`.
/// - **MSE** (HTTP chunked): set `mse_path` to fetch from
///   `/mse/{session_id}{mse_path}` and play via `MediaSource`.
///
/// Both can be set simultaneously for dual-transport output.
/// The content type for MSE is read from the HTTP response `Content-Type`
/// header (set by the `transport::http::mse` node), avoiding duplication
/// with the node's `content_type` param.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct WatchConfig {
    /// MoQ broadcast name the browser subscribes to.
    /// Omit for MSE-only pipelines.
    #[serde(default)]
    pub broadcast: Option<String>,
    /// MSE endpoint path suffix (e.g. `/video`).  When set, the browser
    /// fetches chunked WebM from `/mse/{session_id}{mse_path}`.
    #[serde(default)]
    pub mse_path: Option<String>,
    /// Whether the pipeline outputs audio to subscribers.
    #[serde(default)]
    pub audio: bool,
    /// Whether the pipeline outputs video to subscribers.
    #[serde(default)]
    pub video: bool,
}

/// Input UX configuration for oneshot pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct InputConfig {
    /// The kind of input UX to present.
    #[serde(rename = "type")]
    pub input_type: InputType,
    /// MIME filter for file pickers (e.g. `audio/*`).
    pub accept: Option<String>,
    /// Tags for filtering the asset picker (e.g. `["speech"]`).
    pub asset_tags: Option<Vec<String>>,
    /// Placeholder text for text inputs.
    pub placeholder: Option<String>,
    /// Per-field UI hints keyed by `http_input` field name.
    #[ts(type = "Record<string, FieldHint> | null")]
    pub field_hints: Option<IndexMap<String, FieldHint>>,
}

/// The kind of input UX a oneshot pipeline expects.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    /// File upload with optional MIME filter.
    FileUpload,
    /// Free-form text input.
    Text,
    /// Has `http_input` but the body is irrelevant (trigger only).
    Trigger,
    /// No `http_input` node — pipeline generates its own input.
    None,
}

/// Output rendering configuration for oneshot pipelines.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct OutputConfig {
    /// The media kind the pipeline produces.
    #[serde(rename = "type")]
    pub output_type: OutputType,
}

/// The media kind a oneshot pipeline produces.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    /// Transcription segments (JSON stream).
    Transcription,
    /// Generic JSON stream.
    Json,
    /// Audio output.
    Audio,
    /// Video output.
    Video,
}

/// Per-field input type discriminator for `field_hints`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// File upload field.
    File,
    /// Text input field.
    Text,
}

/// Per-field UI hint within `InputConfig.field_hints`.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct FieldHint {
    /// Override the field's input type (default is file upload).
    #[serde(rename = "type")]
    pub field_type: Option<FieldType>,
    /// MIME filter for file picker.
    pub accept: Option<String>,
    /// Placeholder text for text inputs.
    pub placeholder: Option<String>,
}

/// The kind of interactive control widget rendered in the StreamView.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    /// Boolean on/off switch.
    Toggle,
    /// Debounced text input.
    Text,
    /// Numeric slider with min/max/step.
    Number,
    /// Action button that sends a fixed value on click.
    Button,
    /// Dropdown selector with predefined options.
    Select,
}

/// A single option for a `select` control.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct SelectOption {
    /// Human-readable text shown in the dropdown.
    pub label: String,
    /// Value sent to the server when this option is selected.
    #[ts(type = "unknown")]
    pub value: serde_json::Value,
}

/// A single declarative control entry in the `client.controls` array.
///
/// Each control targets a specific node + property and renders as a widget
/// in the StreamView.  On interaction the frontend sends a `TuneNodeAsync`
/// / `UpdateParams` message to the targeted node.
///
/// The `property` field uses dot-notation paths (e.g. `"properties.home_score"`)
/// so the frontend can build the correct nested JSON payload.  A flat path
/// like `"gain_db"` produces `{"gain_db": <value>}`.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ControlConfig {
    /// Human-readable label shown next to the widget.
    pub label: String,
    /// Widget type.
    #[serde(rename = "type")]
    pub control_type: ControlType,
    /// Target node ID in the pipeline graph.
    pub node: String,
    /// Dot-notation property path, e.g. `"properties.home_score"`.
    pub property: String,
    /// Optional grouping label — controls with the same group are rendered
    /// together under a shared heading.
    #[serde(default)]
    pub group: Option<String>,
    /// Initial value for the UI widget.  This is a **UI-only hint** — it
    /// seeds the local component state but is *not* sent to the server on
    /// mount.  Pipeline authors should ensure defaults here match the
    /// node's own initial params to avoid a visual desync before the first
    /// user interaction.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub default: Option<serde_json::Value>,
    // -- Number-only fields --
    /// Minimum value (number controls).
    #[serde(default)]
    pub min: Option<f64>,
    /// Maximum value (number controls).
    #[serde(default)]
    pub max: Option<f64>,
    /// Step increment (number controls).
    #[serde(default)]
    pub step: Option<f64>,
    // -- Button-only field --
    /// Fixed value sent on click (button controls).  Defaults to `true`.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub value: Option<serde_json::Value>,
    // -- Select-only field --
    /// Predefined options for select controls.  Each entry has a `label`
    /// (shown in the dropdown) and a `value` (sent to the server).
    #[serde(default)]
    pub options: Option<Vec<SelectOption>>,
}

/// The top-level structure for a user-facing pipeline definition.
/// `serde(untagged)` allows it to be parsed as either a steps-based
/// pipeline or a nodes-based (DAG) pipeline.
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
        steps: Vec<Step>,
        /// Declarative UI metadata (optional — required only for UI rendering).
        client: Option<ClientSection>,
    },
    Dag {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default)]
        mode: EngineMode,
        nodes: IndexMap<String, UserNode>,
        /// Declarative UI metadata (optional — required only for UI rendering).
        client: Option<ClientSection>,
    },
}

/// Parse a YAML string into a [`UserPipeline`].
///
/// Uses a two-step approach (YAML → `serde_json::Value` → `UserPipeline`)
/// to work around a `serde_saphyr` limitation where deeply nested
/// structures fail to deserialize inside `#[serde(untagged)]` enums.
///
/// # Errors
///
/// Returns an error if the YAML is malformed or doesn't match the
/// `UserPipeline` schema.
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
