// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for `streamkit-plugin-native`.
//!
//! Compiles the `panicking-plugin` test fixture (a cdylib `.so`) so
//! that the `panicking_plugin` integration test can load it at
//! runtime.  The artefact path is written to
//! `$OUT_DIR/panicking_plugin_path`.

// Build scripts need println! to communicate with Cargo
#![allow(clippy::disallowed_macros)]
// Panicking is the correct error strategy in build scripts
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let fixture_dir = manifest_dir.join("../../tests/fixtures/panicking-plugin");

    // Rebuild when the fixture source or its SDK dependency changes.
    println!("cargo::rerun-if-changed={}", fixture_dir.join("src").display());
    println!("cargo::rerun-if-changed={}", fixture_dir.join("Cargo.toml").display());
    let sdk_dir = manifest_dir.join("../../sdks/plugin-sdk/native/src");
    println!("cargo::rerun-if-changed={}", sdk_dir.display());

    // Docker builds exclude test fixtures via .dockerignore (`**/tests/`).
    if !fixture_dir.exists() {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    // Build the fixture crate.
    let status = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(fixture_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(out_dir.join("panicking-plugin-target"))
        .status()
        .expect("failed to run `cargo build` for panicking-plugin fixture");

    assert!(status.success(), "panicking-plugin fixture build failed");

    let so_path =
        out_dir.join("panicking-plugin-target").join("debug").join("libpanicking_plugin.so");

    assert!(so_path.exists(), "Expected .so at {}", so_path.display());

    std::fs::write(out_dir.join("panicking_plugin_path"), so_path.to_str().expect("valid UTF-8"))
        .expect("failed to write panicking_plugin_path");
}
