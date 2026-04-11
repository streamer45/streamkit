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

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;

/// Expected output metadata for a pipeline test case.
///
/// Lives in a `.toml` sidecar file alongside each test pipeline YAML.
#[derive(Debug, Deserialize)]
pub struct Expected {
    /// File extension for the output (e.g. ".webm", ".mp4").
    pub output_extension: String,

    /// Expected video codec name as reported by ffprobe (e.g. "vp9", "h264", "av1").
    pub codec_name: String,

    /// Expected video width in pixels.
    pub width: u32,

    /// Expected video height in pixels.
    pub height: u32,

    /// Expected container format name from ffprobe (e.g. "matroska,webm", "mov,mp4,m4a,3gp,3g2,mj2").
    pub container_format: String,

    /// Optional: node kind that must be registered in the server for this test
    /// to run. If the node is missing, the test is skipped (returns Ok).
    pub requires_node: Option<String>,

    /// Optional: expected pixel format (e.g. "yuv420p").
    pub pix_fmt: Option<String>,
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
/// Returns the path to the output file on success.
pub fn run_pipeline(
    base_url: &str,
    yaml_contents: &str,
    output_extension: &str,
) -> Result<tempfile::NamedTempFile, String> {
    let url = format!("{base_url}/api/v1/process");

    let form = reqwest::blocking::multipart::Form::new()
        .text("config", yaml_contents.to_string());

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
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

    let bytes = response
        .bytes()
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    tmp.write_all(&bytes)
        .map_err(|e| format!("Failed to write output to temp file: {e}"))?;

    tmp.flush()
        .map_err(|e| format!("Failed to flush temp file: {e}"))?;

    Ok(tmp)
}

/// Validate a pipeline's output file against the expected metadata.
///
/// Runs `ffprobe` against the output file and checks codec, resolution,
/// and container format against the [`Expected`] values.
pub fn validate_output(output_path: &Path, expected: &Expected) -> Result<(), String> {
    let probe = ffprobe::ffprobe(output_path)
        .map_err(|e| format!("ffprobe failed on {}: {e}", output_path.display()))?;

    // Check container format.
    let format_name = &probe.format.format_name;
    if !format_name.contains(&expected.container_format)
        && !expected.container_format.contains(format_name.as_str())
    {
        return Err(format!(
            "Container mismatch: expected format containing '{}', got '{}'",
            expected.container_format, format_name
        ));
    }

    // Find the video stream.
    let video = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or("No video stream found in output")?;

    // Check codec name.
    let codec_name = video
        .codec_name
        .as_deref()
        .unwrap_or("<unknown>");
    if codec_name != expected.codec_name {
        return Err(format!(
            "Codec mismatch: expected '{}', got '{}'",
            expected.codec_name, codec_name
        ));
    }

    // Check resolution.
    let width = video.width.unwrap_or(0) as u32;
    let height = video.height.unwrap_or(0) as u32;
    if width != expected.width || height != expected.height {
        return Err(format!(
            "Resolution mismatch: expected {}x{}, got {}x{}",
            expected.width, expected.height, width, height
        ));
    }

    // Check pixel format (optional).
    if let Some(ref expected_pix_fmt) = expected.pix_fmt {
        let pix_fmt = video.pix_fmt.as_deref().unwrap_or("<unknown>");
        if pix_fmt != expected_pix_fmt.as_str() {
            return Err(format!(
                "Pixel format mismatch: expected '{}', got '{}'",
                expected_pix_fmt, pix_fmt
            ));
        }
    }

    Ok(())
}


