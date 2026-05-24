// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration tests for `LoadedNativePlugin::load` error paths that
//! require a real `.so` on disk (not just a tempfile or a missing path).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use streamkit_plugin_native::LoadedNativePlugin;

fn so_path_from(marker: &str) -> PathBuf {
    let path_file = PathBuf::from(env!("OUT_DIR")).join(marker);
    let path_str = std::fs::read_to_string(&path_file)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path_file.display()));
    let so_path = PathBuf::from(path_str.trim());
    assert!(so_path.exists(), "Fixture .so not found at {}", so_path.display());
    so_path
}

#[test]
fn load_returns_error_when_plugin_api_symbol_is_missing() {
    let so_path = so_path_from("empty_plugin_path");
    let Err(err) = LoadedNativePlugin::load(&so_path) else {
        panic!("expected error for .so without streamkit_native_plugin_api symbol");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("does not export") && msg.contains("streamkit_native_plugin_api"),
        "error must call out the missing entry symbol explicitly, got: {msg}"
    );
}

#[test]
fn load_returns_error_when_plugin_api_version_is_out_of_range() {
    let so_path = so_path_from("bad_version_plugin_path");
    let Err(err) = LoadedNativePlugin::load(&so_path) else {
        panic!("expected error for plugin reporting an out-of-range API version");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("version mismatch") && msg.contains("v5"),
        "error must surface the plugin's reported version and a mismatch hint, got: {msg}"
    );
}
