// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for `streamkit-plugin-native`.
//!
//! Compiles the `panicking-plugin` and `source-plugin` test fixtures
//! (cdylib `.so`) so that the integration tests can load them at
//! runtime.  Artefact paths are written to
//! `$OUT_DIR/panicking_plugin_path` and `$OUT_DIR/source_plugin_path`.

// Build scripts need println! to communicate with Cargo
#![allow(clippy::disallowed_macros)]
// Panicking is the correct error strategy in build scripts
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn build_fixture(
    manifest_dir: &Path,
    out_dir: &Path,
    fixture_subdir: &str,
    lib_basename: &str,
    path_marker: &str,
) {
    let fixture_dir = manifest_dir.join("../../tests/fixtures").join(fixture_subdir);

    println!("cargo::rerun-if-changed={}", fixture_dir.join("src").display());
    println!("cargo::rerun-if-changed={}", fixture_dir.join("Cargo.toml").display());

    // Docker builds exclude test fixtures via .dockerignore (`**/tests/`).
    if !fixture_dir.exists() {
        return;
    }

    let target_dir = out_dir.join(format!("{fixture_subdir}-target"));

    let status = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(fixture_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run `cargo build` for {fixture_subdir}: {e}"));

    assert!(status.success(), "{fixture_subdir} fixture build failed");

    let so_path = target_dir.join("debug").join(format!("lib{lib_basename}.so"));

    assert!(so_path.exists(), "Expected .so at {}", so_path.display());

    std::fs::write(out_dir.join(path_marker), so_path.to_str().expect("valid UTF-8"))
        .unwrap_or_else(|e| panic!("failed to write {path_marker}: {e}"));
}

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    let sdk_dir = manifest_dir.join("../../sdks/plugin-sdk/native/src");
    println!("cargo::rerun-if-changed={}", sdk_dir.display());

    build_fixture(
        &manifest_dir,
        &out_dir,
        "panicking-plugin",
        "panicking_plugin",
        "panicking_plugin_path",
    );
    build_fixture(&manifest_dir, &out_dir, "source-plugin", "source_plugin", "source_plugin_path");
    build_fixture(&manifest_dir, &out_dir, "empty-plugin", "empty_plugin", "empty_plugin_path");
    build_fixture(
        &manifest_dir,
        &out_dir,
        "bad-version-plugin",
        "bad_version_plugin",
        "bad_version_plugin_path",
    );
}
