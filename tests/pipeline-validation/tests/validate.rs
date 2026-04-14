// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Headless pipeline validation test harness.
//!
//! Discovers all `pipeline.yml` files under `samples/pipelines/test/<name>/`,
//! loads the sibling `expected.toml`, runs the pipeline against a live `skit`
//! server, and validates the output with `ffprobe`.
//!
//! # Usage
//!
//! ```bash
//! # Start skit, then run tests:
//! PIPELINE_TEST_URL=http://127.0.0.1:4545 \
//!   cargo test --manifest-path tests/pipeline-validation/Cargo.toml
//!
//! # Or via justfile:
//! just test-pipelines http://127.0.0.1:4545
//! ```

#![allow(clippy::disallowed_macros)] // Test binary — no logging crate available.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use pipeline_validation::{
    get_available_nodes, get_base_url, run_pipeline, validate_output, Expected,
};

/// Lazily resolved base URL for the skit server.
fn base_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(get_base_url)
}

/// Lazily resolved set of available node kinds on the server.
///
/// When `PIPELINE_REQUIRE_NODES=1` is set, failure to query the schema
/// endpoint panics instead of returning an empty set — this prevents
/// a broken server from silently skipping all HW-codec tests.
fn available_nodes() -> &'static HashSet<String> {
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
            eprintln!("  HW codec tests will be skipped.");
            eprintln!("  Is skit running at {}?", base_url());
            HashSet::new()
        })
    })
}

/// The main test function called by `datatest-stable` for each `pipeline.yml`.
///
/// For each test directory it:
/// 1. Loads the sibling `expected.toml`
/// 2. Checks if the required node kind is available (skips if not)
/// 3. POSTs the pipeline to the server
/// 4. Saves the response to a temp file
/// 5. Validates the output with `ffprobe`
fn validate_pipeline(path: &Path, yaml: String) -> datatest_stable::Result<()> {
    // Load sibling expectations file from the same directory.
    let test_dir = path.parent().expect("pipeline.yml must be inside a test directory");
    let expected_path = test_dir.join("expected.toml");
    let expected_toml = std::fs::read_to_string(&expected_path).map_err(|e| {
        format!(
            "Missing expectations file '{}': {e}\n\
             Each test pipeline YAML needs a companion .toml with expected output metadata.",
            expected_path.display()
        )
    })?;

    let expected: Expected = toml::from_str(&expected_toml).map_err(|e| {
        format!(
            "Invalid expectations file '{}': {e}",
            expected_path.display()
        )
    })?;

    // Check if the required node is available.
    //
    // When `PIPELINE_REQUIRE_NODES=1` is set (typically in CI jobs that
    // explicitly compile with the required features), a missing node is
    // treated as a test failure instead of a silent skip.  This prevents
    // registration regressions from producing false-green CI runs.
    if let Some(ref required) = expected.requires_node {
        if !available_nodes().contains(required.as_str()) {
            if std::env::var("PIPELINE_REQUIRE_NODES").as_deref() == Ok("1") {
                return Err(format!(
                    "FAIL: '{}' requires node '{}' which is not available on this server \
                     (PIPELINE_REQUIRE_NODES=1 — skipping is not allowed)",
                    path.display(),
                    required
                )
                .into());
            }
            eprintln!(
                "SKIP: '{}' requires node '{}' which is not available on this server",
                path.display(),
                required
            );
            return Ok(());
        }
    }

    let test_name = test_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Resolve the input file path (if any) relative to the test directory.
    let input_file = expected.input_file.as_ref().map(|rel| test_dir.join(rel));
    let input_ref = input_file.as_deref();

    // Run the pipeline.
    let output = run_pipeline(base_url(), &yaml, &expected.output_extension, input_ref)
        .map_err(|e| format!("Pipeline '{test_name}' failed: {e}"))?;

    // Validate with ffprobe.
    validate_output(output.path(), &expected)
        .map_err(|e| format!("Validation failed for '{test_name}': {e}"))?;

    Ok(())
}

datatest_stable::harness! {
    { test = validate_pipeline, root = "../../samples/pipelines/test", pattern = r"pipeline\.yml$" },
}
