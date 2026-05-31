// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Headless pipeline validation helpers for StreamKit.
//!
//! This crate provides utilities for running oneshot pipelines against a live
//! `skit` server, capturing the output, and validating it with `ffprobe`.
//! No browser required.
//!
//! # Architecture
//!
//! Each test case is defined by a pair of files in `samples/pipelines/test/`:
//!
//! - `<name>.yml` — the pipeline YAML to POST to `/api/v1/process`
//! - `<name>.toml` — expected output metadata (codec, resolution, container)
//!
//! The test harness discovers all `.yml` files, loads the companion `.toml`,
//! runs the pipeline, and validates the output with `ffprobe`.

#![allow(clippy::disallowed_macros)] // Test-support crate — no logging crate available.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// `ffprobe`-checkable expectations about a pipeline's output media.
///
/// Shared by [`Expected`] (test fixtures) and [`OneshotEntry`] (official
/// samples) via `#[serde(flatten)]`, so a new ffprobe check is added in one
/// place and honoured by both harnesses.
#[derive(Debug, Default, Deserialize)]
pub struct MediaExpectations {
    /// Expected container format name from ffprobe (e.g. "matroska,webm",
    /// "mov,mp4,m4a,3gp,3g2,mj2"). When absent, the container check is skipped.
    pub container_format: Option<String>,

    /// Expected video codec name as reported by ffprobe (e.g. "vp9", "h264", "av1").
    pub codec_name: Option<String>,

    /// Expected video width in pixels.
    pub width: Option<u32>,

    /// Expected video height in pixels.
    pub height: Option<u32>,

    /// Optional: expected pixel format (e.g. "yuv420p").
    pub pix_fmt: Option<String>,

    /// Expected audio codec name as reported by ffprobe (e.g. "opus", "mp3", "flac").
    pub audio_codec: Option<String>,

    /// Expected audio sample rate in Hz (e.g. 48000).
    pub sample_rate: Option<u32>,

    /// Expected number of audio channels (e.g. 2).
    pub channels: Option<u32>,
}

/// Expected output metadata for a pipeline test case.
///
/// Lives in a `.toml` sidecar file alongside each test pipeline YAML.
#[derive(Debug, Deserialize)]
pub struct Expected {
    /// File extension for the output (e.g. ".webm", ".mp4", ".ogg").
    pub output_extension: String,

    /// Optional: node kind that must be registered in the server for this test
    /// to run. If the node is missing, the test is skipped (returns Ok).
    pub requires_node: Option<String>,

    /// Relative path (from the test directory) to an input file to upload.
    /// If set, the pipeline will be run with a file upload instead of no input.
    pub input_file: Option<String>,

    #[serde(flatten)]
    pub media: MediaExpectations,
}

/// Get the base URL for the skit server from environment.
///
/// Checks `PIPELINE_TEST_URL` first, then `E2E_BASE_URL`, and defaults to
/// `http://127.0.0.1:4545`.
pub fn get_base_url() -> String {
    std::env::var("PIPELINE_TEST_URL")
        .or_else(|_| std::env::var("E2E_BASE_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:4545".to_string())
}

/// Lazily resolved base URL for the skit server (see [`get_base_url`]).
pub fn base_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(get_base_url)
}

/// Lazily resolved set of node kinds registered on the server.
///
/// When `PIPELINE_REQUIRE_NODES=1` is set, failure to query the schema endpoint
/// panics instead of returning an empty set — this prevents a broken server
/// from silently skipping all node-gated tests.
pub fn available_nodes() -> &'static HashSet<String> {
    static NODES: OnceLock<HashSet<String>> = OnceLock::new();
    NODES.get_or_init(|| {
        get_available_nodes(base_url()).unwrap_or_else(|err| {
            if std::env::var("PIPELINE_REQUIRE_NODES").as_deref() == Ok("1") {
                panic!(
                    "FATAL: Could not query available nodes: {err}\n  \
                     PIPELINE_REQUIRE_NODES=1 requires a reachable server at {}",
                    base_url()
                );
            }
            eprintln!("WARNING: Could not query available nodes: {err}");
            eprintln!("  Node-gated tests will be skipped.");
            eprintln!("  Is skit running at {}?", base_url());
            HashSet::new()
        })
    })
}

/// Query the server's node schema endpoint to discover which node kinds are
/// registered. Returns a set of node kind strings.
///
/// This is used to skip tests for HW codecs that aren't compiled into the
/// running server binary (e.g. `video::nv::av1_encoder`).
pub fn get_available_nodes(base_url: &str) -> Result<HashSet<String>, String> {
    let url = format!("{base_url}/api/v1/schema/nodes");
    let response = reqwest::blocking::get(&url)
        .map_err(|e| format!("Failed to query node schema at {url}: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Node schema request failed with status {}",
            response.status()
        ));
    }

    #[derive(Deserialize)]
    struct NodeInfo {
        kind: String,
    }

    let nodes: Vec<NodeInfo> = response
        .json()
        .map_err(|e| format!("Failed to parse node schema JSON: {e}"))?;

    Ok(nodes.into_iter().map(|n| n.kind).collect())
}

/// Run a oneshot pipeline against the skit server.
///
/// Posts the pipeline YAML as a multipart form to `/api/v1/process` and
/// saves the streamed response body to a temporary file.
///
/// If `input_file` is `Some`, the file is attached as the `media` part of the
/// multipart form (for pipelines that expect `client.input.type: file_upload`).
///
/// Returns the path to the output file on success.
pub fn run_pipeline(
    base_url: &str,
    yaml_contents: &str,
    output_extension: &str,
    input_file: Option<&Path>,
) -> Result<tempfile::NamedTempFile, String> {
    let parts: Vec<PartSpec> = input_file
        .map(|p| {
            vec![PartSpec::File {
                field: "media".to_string(),
                path: p.to_path_buf(),
            }]
        })
        .unwrap_or_default();
    run_pipeline_parts(base_url, yaml_contents, output_extension, &parts)
}

/// A single multipart form field to attach to a `/api/v1/process` request.
///
/// `streamkit::http_input` maps a single `file_upload`/`text` client input to
/// the `media` field; pipelines declaring named `fields` expect one part per
/// field name (each either an uploaded file or a literal text value).
pub enum PartSpec {
    /// A file upload attached under `field` with the file's basename.
    File { field: String, path: PathBuf },
    /// A literal text value sent as the `field` form field.
    Text { field: String, value: String },
}

/// Run a oneshot pipeline with an arbitrary set of multipart form parts.
///
/// Generalises [`run_pipeline`] to support pipelines with multiple named
/// upload fields (e.g. dual-input mixers) and text inputs (e.g. TTS).
pub fn run_pipeline_parts(
    base_url: &str,
    yaml_contents: &str,
    output_extension: &str,
    parts: &[PartSpec],
) -> Result<tempfile::NamedTempFile, String> {
    let url = format!("{base_url}/api/v1/process");

    let mut form = reqwest::blocking::multipart::Form::new()
        .text("config", yaml_contents.to_string());

    for part in parts {
        match part {
            PartSpec::File { field, path } => {
                let file_bytes = std::fs::read(path)
                    .map_err(|e| format!("Failed to read input file {}: {e}", path.display()))?;
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("input")
                    .to_string();
                let file_part = reqwest::blocking::multipart::Part::bytes(file_bytes)
                    .file_name(file_name);
                form = form.part(field.clone(), file_part);
            },
            PartSpec::Text { field, value } => {
                form = form.text(field.clone(), value.clone());
            },
        }
    }

    let timeout_secs = std::env::var("PIPELINE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let response = client
        .post(&url)
        .multipart(form)
        .send()
        .map_err(|e| format!("Pipeline request to {url} failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unreadable>".to_string());
        return Err(format!("Pipeline returned {status}: {body}"));
    }

    // Stream response to a temp file.
    let mut tmp = tempfile::Builder::new()
        .suffix(output_extension)
        .tempfile()
        .map_err(|e| format!("Failed to create temp file: {e}"))?;

    let bytes = response.bytes().map_err(|e| {
        let mut msg = format!("Failed to read response body: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            msg.push_str(&format!("\n  caused by: {cause}"));
            source = std::error::Error::source(cause);
        }
        msg
    })?;

    if bytes.is_empty() {
        return Err(
            "Pipeline returned HTTP 200 but the response body is empty. \
             This usually means the encoder failed to produce output \
             (e.g. the GPU does not support the required codec via this API)."
                .to_string(),
        );
    }

    tmp.write_all(&bytes)
        .map_err(|e| format!("Failed to write output to temp file: {e}"))?;

    tmp.flush()
        .map_err(|e| format!("Failed to flush temp file: {e}"))?;

    Ok(tmp)
}

/// Validate a pipeline's output file against the expected metadata.
///
/// Runs `ffprobe` against the output file and checks codec, resolution,
/// container format, and audio properties against the [`Expected`] values.
pub fn validate_output(output_path: &Path, expected: &MediaExpectations) -> Result<(), String> {
    let file_size = std::fs::metadata(output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    let probe = ffprobe::ffprobe(output_path).map_err(|e| {
        format!(
            "ffprobe failed on {} ({file_size} bytes): {e}",
            output_path.display()
        )
    })?;

    let format_name = &probe.format.format_name;
    if let Some(ref expected_format) = expected.container_format {
        if !format_name.contains(expected_format)
            && !expected_format.contains(format_name.as_str())
        {
            return Err(format!(
                "Container mismatch: expected format containing '{expected_format}', got '{format_name}'"
            ));
        }
    }
    if let Some(ref expected_codec) = expected.codec_name {
        let video = probe
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"))
            .ok_or("No video stream found in output (expected video codec)")?;

        let codec_name = video.codec_name.as_deref().unwrap_or("<unknown>");
        if codec_name != expected_codec.as_str() {
            return Err(format!(
                "Video codec mismatch: expected '{}', got '{}'",
                expected_codec, codec_name
            ));
        }

        if let Some(expected_w) = expected.width {
            let width = video.width.unwrap_or(0) as u32;
            if width != expected_w {
                return Err(format!(
                    "Video width mismatch: expected {expected_w}, got {width}"
                ));
            }
        }

        if let Some(expected_h) = expected.height {
            let height = video.height.unwrap_or(0) as u32;
            if height != expected_h {
                return Err(format!(
                    "Video height mismatch: expected {expected_h}, got {height}"
                ));
            }
        }

        if let Some(ref expected_pix_fmt) = expected.pix_fmt {
            let pix_fmt = video.pix_fmt.as_deref().unwrap_or("<unknown>");
            if pix_fmt != expected_pix_fmt.as_str() {
                return Err(format!(
                    "Pixel format mismatch: expected '{}', got '{}'",
                    expected_pix_fmt, pix_fmt
                ));
            }
        }
    }
    if let Some(ref expected_audio_codec) = expected.audio_codec {
        let audio = probe
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("audio"))
            .ok_or("No audio stream found in output (expected audio codec)")?;

        let codec_name = audio.codec_name.as_deref().unwrap_or("<unknown>");
        if codec_name != expected_audio_codec.as_str() {
            return Err(format!(
                "Audio codec mismatch: expected '{}', got '{}'",
                expected_audio_codec, codec_name
            ));
        }

        if let Some(expected_sr) = expected.sample_rate {
            // ffprobe reports sample_rate as a string in the stream.
            let actual_sr = audio
                .sample_rate
                .as_deref()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            if actual_sr != expected_sr {
                return Err(format!(
                    "Sample rate mismatch: expected {expected_sr}, got {actual_sr}"
                ));
            }
        }

        if let Some(expected_ch) = expected.channels {
            let actual_ch = audio.channels.unwrap_or(0) as u32;
            if actual_ch != expected_ch {
                return Err(format!(
                    "Channel count mismatch: expected {expected_ch}, got {actual_ch}"
                ));
            }
        }
    }

    // Ensure at least one stream type was validated.
    if expected.codec_name.is_none() && expected.audio_codec.is_none() {
        return Err(
            "Expected TOML must specify at least one of 'codec_name' (video) or 'audio_codec' (audio)"
                .to_string(),
        );
    }

    Ok(())
}

/// How a oneshot sample's output should be validated.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// A media container validated with `ffprobe` (the default).
    #[default]
    Media,
    /// A JSON / NDJSON body (e.g. transcription or VAD events).
    Json,
}

/// A multipart input declared by a oneshot sample's manifest entry.
///
/// Exactly one of `file` (a repo-relative path uploaded as a file) or `text`
/// (a literal value) must be set.
#[derive(Debug, Deserialize)]
pub struct InputSpec {
    /// Multipart field name (`media` for a single input, or a declared field).
    pub field: String,
    /// Repo-relative path to a file to upload under `field`.
    #[serde(default)]
    pub file: Option<String>,
    /// Literal text value to send under `field`.
    #[serde(default)]
    pub text: Option<String>,
}

/// Expected results and run configuration for one official oneshot sample.
///
/// Loaded from the `[<sample-stem>]` table in `oneshot-samples.toml`. Reuses
/// [`MediaExpectations`] and adds skip controls (`requires_node`,
/// `optional_node`, `requires_env`, `slow`), JSON validation, and multipart
/// inputs.
#[derive(Debug, Deserialize)]
pub struct OneshotEntry {
    /// Output file extension (e.g. `webm`, `mp4`, `ogg`, `json`).
    pub output_extension: String,

    /// How to validate the output body.
    #[serde(default)]
    pub output_kind: OutputKind,

    #[serde(flatten)]
    pub media: MediaExpectations,

    /// Substrings that must appear in a JSON output (used when `output_kind = json`).
    #[serde(default)]
    pub json_contains: Vec<String>,

    /// Node kind that must be registered for this sample to run.
    pub requires_node: Option<String>,
    /// Additional node kinds that must also be registered, for samples that
    /// chain several plugins or codecs. Combined with `requires_node`.
    #[serde(default)]
    pub requires_nodes: Vec<String>,
    /// When `true`, a missing required node is always a skip — even under
    /// `PIPELINE_REQUIRE_NODES=1`. Used for nodes that no CI job compiles
    /// (marketplace plugins, VA-API) so the GPU job does not fail on them.
    #[serde(default)]
    pub optional_node: bool,
    /// Environment variables that must be set for this sample to run
    /// (e.g. S3 credentials). Missing any of them skips the sample.
    #[serde(default)]
    pub requires_env: Vec<String>,
    /// When `true`, the sample only runs if `PIPELINE_INCLUDE_SLOW=1` is set.
    /// Used for heavyweight showcase pipelines (realtime pacers, slow SW AV1).
    #[serde(default)]
    pub slow: bool,

    /// Multipart inputs to attach to the request.
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
}

impl OneshotEntry {
    /// Resolve this entry's declared inputs into concrete multipart parts,
    /// resolving `file` paths relative to `repo_root`.
    pub fn parts(&self, repo_root: &Path) -> Result<Vec<PartSpec>, String> {
        self.inputs
            .iter()
            .map(|input| match (&input.file, &input.text) {
                (Some(file), None) => Ok(PartSpec::File {
                    field: input.field.clone(),
                    path: repo_root.join(file),
                }),
                (None, Some(text)) => Ok(PartSpec::Text {
                    field: input.field.clone(),
                    value: text.clone(),
                }),
                _ => Err(format!(
                    "Input field '{}' must set exactly one of 'file' or 'text'",
                    input.field
                )),
            })
            .collect()
    }

    /// All node kinds that must be registered for this sample to run.
    pub fn required_nodes(&self) -> impl Iterator<Item = &str> {
        self.requires_node
            .as_deref()
            .into_iter()
            .chain(self.requires_nodes.iter().map(String::as_str))
    }
}

/// Validate a JSON / NDJSON response body.
///
/// Accepts either a single JSON document or newline-delimited JSON (one record
/// per line, as emitted by the transcription/VAD samples). Every record must
/// parse, and every string in `json_contains` must appear somewhere in the body.
pub fn validate_json_output(output_path: &Path, json_contains: &[String]) -> Result<(), String> {
    let body = std::fs::read_to_string(output_path)
        .map_err(|e| format!("Failed to read JSON output {}: {e}", output_path.display()))?;

    if body.trim().is_empty() {
        return Err("JSON output is empty".to_string());
    }

    // A single JSON document; otherwise treat the body as newline-delimited
    // JSON and require every record to parse.
    if serde_json::from_str::<serde_json::Value>(body.trim()).is_err() {
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|e| format!("Output line is not valid JSON: {e}\n  line: {line}"))?;
        }
    }

    for needle in json_contains {
        if !body.contains(needle.as_str()) {
            return Err(format!("JSON output missing expected substring '{needle}'"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_flattens_media_fields() {
        let expected: Expected = toml::from_str(
            r#"
            output_extension = ".webm"
            requires_node = "video::vp9::encoder"
            container_format = "matroska,webm"
            codec_name = "vp9"
            width = 1280
            height = 720
            sample_rate = 48000
            channels = 2
            "#,
        )
        .expect("Expected should deserialize flattened media fields");

        assert_eq!(expected.output_extension, ".webm");
        assert_eq!(expected.requires_node.as_deref(), Some("video::vp9::encoder"));
        assert_eq!(expected.media.container_format.as_deref(), Some("matroska,webm"));
        assert_eq!(expected.media.codec_name.as_deref(), Some("vp9"));
        assert_eq!(expected.media.width, Some(1280));
        assert_eq!(expected.media.height, Some(720));
        assert_eq!(expected.media.sample_rate, Some(48000));
        assert_eq!(expected.media.channels, Some(2));
    }

    #[test]
    fn oneshot_entry_flattens_media_fields() {
        let entry: OneshotEntry = toml::from_str(
            r#"
            output_extension = ".mp4"
            container_format = "mov,mp4,m4a,3gp,3g2,mj2"
            codec_name = "av1"
            width = 1920
            height = 1080
            slow = true
            requires_node = "video::nv::av1_encoder"
            "#,
        )
        .expect("OneshotEntry should deserialize flattened media fields");

        assert_eq!(entry.media.codec_name.as_deref(), Some("av1"));
        assert_eq!(entry.media.width, Some(1920));
        assert_eq!(entry.media.height, Some(1080));
        assert!(entry.slow);
        assert_eq!(entry.requires_node.as_deref(), Some("video::nv::av1_encoder"));
    }
}

