// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Validation harness for the official oneshot sample pipelines.
//!
//! Discovers every `samples/pipelines/oneshot/*.yml`, looks up its expectations
//! in `oneshot-samples.toml`, runs it against a live `skit` server, and
//! validates the output (with `ffprobe` for media, or a JSON parse for
//! transcription/VAD outputs). Reuses the helpers in `pipeline_validation`.
//!
//! Skip controls per sample:
//! - `requires_node` — skipped when the node kind is not registered (e.g.
//!   marketplace plugins, HW codecs). Under `PIPELINE_REQUIRE_NODES=1` a missing
//!   node is a failure unless `optional_node = true`.
//! - `requires_env` — skipped when any listed env var is unset (e.g. S3 creds).
//! - `slow = true` — skipped unless `PIPELINE_INCLUDE_SLOW=1` (heavyweight
//!   showcase pipelines with realtime pacers or slow software AV1).
//!
//! Every oneshot YAML must have a manifest entry; a new sample without one fails
//! the harness so it gets triaged.
//!
//! # Usage
//!
//! ```bash
//! PIPELINE_TEST_URL=http://127.0.0.1:4545 \
//!   cargo test --manifest-path tests/pipeline-validation/Cargo.toml --test oneshot
//!
//! # Include the slow showcase samples:
//! PIPELINE_INCLUDE_SLOW=1 just test-oneshot-samples http://127.0.0.1:4545
//! ```

#![allow(clippy::disallowed_macros)] // Test binary — no logging crate available.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pipeline_validation::{
    available_nodes, base_url, run_pipeline_parts, validate_json_output, validate_output,
    OneshotEntry, OutputKind,
};

/// Repo root, resolved from this crate's manifest dir (`tests/pipeline-validation`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The oneshot sample expectations manifest, loaded once.
fn manifest() -> &'static HashMap<String, OneshotEntry> {
    static MANIFEST: OnceLock<HashMap<String, OneshotEntry>> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("oneshot-samples.toml");
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("Failed to read oneshot manifest {}: {e}", path.display())
        });
        toml::from_str(&contents).unwrap_or_else(|e| {
            panic!("Failed to parse oneshot manifest {}: {e}", path.display())
        })
    })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

/// Validate one official oneshot sample, identified by its YAML file stem.
fn validate_oneshot(path: &Path, yaml: String) -> datatest_stable::Result<()> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("oneshot sample must have a file stem")?;

    let entry = manifest().get(stem).ok_or_else(|| {
        format!(
            "No manifest entry for oneshot sample '{stem}'. \
             Add a `[{stem}]` table to tests/pipeline-validation/oneshot-samples.toml \
             with the expected output (and any requires_node / requires_env / slow flags)."
        )
    })?;

    // Skip when required env vars (e.g. S3 credentials) are absent.
    if let Some(missing) = entry
        .requires_env
        .iter()
        .find(|var| std::env::var(var).is_err())
    {
        eprintln!("SKIP: '{stem}' requires env var '{missing}' which is not set");
        return Ok(());
    }

    // Skip heavyweight showcase samples unless explicitly opted in.
    if entry.slow && !env_flag("PIPELINE_INCLUDE_SLOW") {
        eprintln!("SKIP: '{stem}' is slow; set PIPELINE_INCLUDE_SLOW=1 to run it");
        return Ok(());
    }

    // Skip (or fail) when any required node kind is not registered.
    if let Some(missing) = entry
        .required_nodes()
        .find(|node| !available_nodes().contains(*node))
    {
        if !entry.optional_node && env_flag("PIPELINE_REQUIRE_NODES") {
            return Err(format!(
                "FAIL: '{stem}' requires node '{missing}' which is not available \
                 (PIPELINE_REQUIRE_NODES=1 — skipping is not allowed)"
            )
            .into());
        }
        eprintln!("SKIP: '{stem}' requires node '{missing}' which is not available");
        return Ok(());
    }

    let parts = entry.parts(&repo_root())?;
    let output = run_pipeline_parts(base_url(), &yaml, &entry.output_extension, &parts)
        .map_err(|e| format!("Pipeline '{stem}' failed: {e}"))?;

    match entry.output_kind {
        OutputKind::Media => validate_output(output.path(), &entry.media)
            .map_err(|e| format!("Validation failed for '{stem}': {e}"))?,
        OutputKind::Json => validate_json_output(output.path(), &entry.json_contains)
            .map_err(|e| format!("Validation failed for '{stem}': {e}"))?,
    }

    Ok(())
}

datatest_stable::harness! {
    { test = validate_oneshot, root = "../../samples/pipelines/oneshot", pattern = r"\.yml$" },
}
