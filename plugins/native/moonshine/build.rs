// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for the Moonshine native plugin.
//!
//! Compiles the Moonshine C++ core from source as a static library, eliminating
//! the need for users to pre-install libmoonshine. At runtime, the plugin still
//! needs libonnxruntime.so (typically bundled alongside the plugin .so).
//!
//! Environment variables:
//!   MOONSHINE_SRC_DIR  - Path to a local moonshine source checkout (skips download)
//!   ORT_LIB_DIR        - Path to directory containing libonnxruntime.so (skips search)

// Allow: println! in build.rs is the standard way to communicate with Cargo, not logging.
// Allow: expect/unwrap are standard in build scripts — panicking IS the error handling.
#![allow(clippy::disallowed_macros, clippy::expect_used, clippy::unwrap_used)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Moonshine C API version to build from source.
const MOONSHINE_VERSION: &str = "0.0.49";

/// ONNX Runtime version compatible with Moonshine v0.0.49.
const ORT_VERSION: &str = "1.23.2";

fn main() {
    println!("cargo:rerun-if-env-changed=MOONSHINE_SRC_DIR");
    println!("cargo:rerun-if-env-changed=ORT_LIB_DIR");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    // Step 1: Get moonshine source (download or use local checkout)
    let moonshine_src = get_moonshine_source(&out_dir);
    let core_dir = moonshine_src.join("core");

    // Step 2: Find or download ONNX Runtime shared library
    let ort_lib_dir = find_or_download_onnxruntime(&out_dir);

    // Step 3: Compile moonshine C++ core into a static archive
    println!("cargo:warning=Compiling Moonshine C++ core (17 files, may take a few minutes)...");
    build_moonshine_static(&core_dir);

    // Step 4: Link against onnxruntime dynamically (for ORT symbols used by moonshine)
    println!("cargo:rustc-link-search=native={}", ort_lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=onnxruntime");

    // $ORIGIN rpath so the plugin finds libonnxruntime.so next to itself at runtime
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
}

// ---------------------------------------------------------------------------
// Moonshine source acquisition
// ---------------------------------------------------------------------------

/// Returns the path to the moonshine source root directory.
///
/// If `MOONSHINE_SRC_DIR` is set, uses that path directly. Otherwise downloads
/// the source tarball from GitHub.
fn get_moonshine_source(out_dir: &Path) -> PathBuf {
    if let Ok(src_dir) = env::var("MOONSHINE_SRC_DIR") {
        let path = PathBuf::from(src_dir);
        assert!(
            path.join("core/moonshine-c-api.h").exists(),
            "MOONSHINE_SRC_DIR does not contain core/moonshine-c-api.h"
        );
        return path;
    }

    let extract_dir = out_dir.join(format!("moonshine-{MOONSHINE_VERSION}"));
    if extract_dir.join("core/moonshine-c-api.h").exists() {
        return extract_dir;
    }

    let tarball_url = format!(
        "https://github.com/moonshine-ai/moonshine/archive/refs/tags/v{MOONSHINE_VERSION}.tar.gz"
    );
    let tarball_path = out_dir.join(format!("moonshine-v{MOONSHINE_VERSION}.tar.gz"));

    println!(
        "cargo:warning=Downloading Moonshine v{MOONSHINE_VERSION} source (first build only)..."
    );
    run_command(
        Command::new("curl").args(["--fail", "-L", "-o"]).arg(&tarball_path).arg(&tarball_url),
    );

    println!("cargo:warning=Extracting Moonshine source...");
    run_command(Command::new("tar").arg("xf").arg(&tarball_path).arg("-C").arg(out_dir));

    assert!(
        extract_dir.join("core/moonshine-c-api.h").exists(),
        "Extracted moonshine source missing core/moonshine-c-api.h at {}",
        extract_dir.display()
    );

    extract_dir
}

// ---------------------------------------------------------------------------
// ONNX Runtime discovery / download
// ---------------------------------------------------------------------------

/// Required ORT major version (derived from `ORT_VERSION`).
const ORT_MAJOR: u32 = 1;
const ORT_MINOR: u32 = 23;

/// Finds a compatible onnxruntime installation or downloads one.
///
/// Search order:
///   1. `ORT_LIB_DIR` environment variable (must contain compatible version)
///   2. `/usr/local/lib` (only if version matches)
///   3. `/usr/lib/x86_64-linux-gnu` (only if version matches)
///   4. `/usr/lib` (only if version matches)
///   5. Download from GitHub releases into OUT_DIR
#[allow(clippy::similar_names)] // path vs patch are semantically distinct
fn find_or_download_onnxruntime(out_dir: &Path) -> PathBuf {
    if let Ok(dir) = env::var("ORT_LIB_DIR") {
        let path = PathBuf::from(&dir);
        assert!(has_onnxruntime(&path), "ORT_LIB_DIR={dir} does not contain libonnxruntime.so*");
        match ort_version_from_dir(&path) {
            Some((major, minor, _)) if major == ORT_MAJOR && minor == ORT_MINOR => {
                println!("cargo:warning=Using ORT_LIB_DIR={dir} (v{major}.{minor})");
                return path;
            },
            Some((major, minor, patch)) => {
                panic!(
                    "ORT_LIB_DIR={dir} contains ORT {major}.{minor}.{patch} \
                     but moonshine requires {ORT_MAJOR}.{ORT_MINOR}.x"
                );
            },
            None => {
                // Can't determine version — trust the user
                println!("cargo:warning=Using ORT_LIB_DIR={dir} (version unknown)");
                return path;
            },
        }
    }

    let search_paths = ["/usr/local/lib", "/usr/lib/x86_64-linux-gnu", "/usr/lib"];
    for dir in &search_paths {
        let path = PathBuf::from(dir);
        if !has_onnxruntime(&path) {
            continue;
        }
        match ort_version_from_dir(&path) {
            Some((major, minor, _)) if major == ORT_MAJOR && minor == ORT_MINOR => {
                println!("cargo:warning=Found compatible onnxruntime {major}.{minor} at {dir}");
                return path;
            },
            Some((major, minor, _)) => {
                println!(
                    "cargo:warning=Skipping onnxruntime {major}.{minor} at {dir} \
                     (need {ORT_MAJOR}.{ORT_MINOR})"
                );
            },
            None => {
                println!(
                    "cargo:warning=Skipping onnxruntime at {dir} (could not determine version)"
                );
            },
        }
    }

    // No compatible version on system — download it
    download_onnxruntime(out_dir)
}

/// Downloads the ONNX Runtime shared library from GitHub releases.
#[allow(clippy::similar_names)] // ort_extract_dir vs out_dir are semantically distinct
fn download_onnxruntime(out_dir: &Path) -> PathBuf {
    let ort_dir_name = format!("onnxruntime-linux-x64-{ORT_VERSION}");
    let ort_extract_dir = out_dir.join(&ort_dir_name);
    let lib_dir = ort_extract_dir.join("lib");

    if has_onnxruntime(&lib_dir) {
        println!("cargo:warning=Using cached onnxruntime at {}", lib_dir.display());
        return lib_dir;
    }

    let tarball_url = format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{ort_dir_name}.tgz"
    );
    let tarball_path = out_dir.join(format!("onnxruntime-{ORT_VERSION}.tgz"));

    println!("cargo:warning=Downloading ONNX Runtime v{ORT_VERSION} (first build only)...");
    run_command(
        Command::new("curl").args(["--fail", "-L", "-o"]).arg(&tarball_path).arg(&tarball_url),
    );

    println!("cargo:warning=Extracting ONNX Runtime...");
    run_command(Command::new("tar").arg("xf").arg(&tarball_path).arg("-C").arg(out_dir));

    assert!(
        has_onnxruntime(&lib_dir),
        "Downloaded onnxruntime missing libonnxruntime.so in {}",
        lib_dir.display()
    );

    lib_dir
}

/// Checks if a directory contains an onnxruntime shared library.
fn has_onnxruntime(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "libonnxruntime.so" || name.starts_with("libonnxruntime.so.") {
                return true;
            }
        }
    }
    false
}

/// Attempts to extract the ORT version from versioned symlinks in a directory.
///
/// Looks for files named `libonnxruntime.so.X.Y.Z` and parses out the version.
/// Returns `None` if no versioned file is found.
fn ort_version_from_dir(dir: &Path) -> Option<(u32, u32, u32)> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Match libonnxruntime.so.X.Y.Z (the most specific versioned name)
        if let Some(ver_str) = name.strip_prefix("libonnxruntime.so.") {
            let parts: Vec<&str> = ver_str.split('.').collect();
            if parts.len() == 3 {
                if let (Ok(major), Ok(minor), Ok(patch)) =
                    (parts[0].parse::<u32>(), parts[1].parse::<u32>(), parts[2].parse::<u32>())
                {
                    return Some((major, minor, patch));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// C++ compilation
// ---------------------------------------------------------------------------

/// Compiles the Moonshine C++ core into a static library using the `cc` crate.
fn build_moonshine_static(core_dir: &Path) {
    cc::Build::new()
        .cpp(true)
        .flag("-std=c++20")
        .pic(true)
        .warnings(false) // suppress warnings from third-party code
        .opt_level_str("2")
        // ---- Main moonshine source files ----
        .file(core_dir.join("moonshine-c-api.cpp"))
        .file(core_dir.join("cosine-distance.cpp"))
        .file(core_dir.join("moonshine-model.cpp"))
        .file(core_dir.join("moonshine-streaming-model.cpp"))
        .file(core_dir.join("voice-activity-detector.cpp"))
        .file(core_dir.join("silero-vad.cpp"))
        .file(core_dir.join("resampler.cpp"))
        .file(core_dir.join("transcriber.cpp"))
        .file(core_dir.join("gemma-embedding-model.cpp"))
        .file(core_dir.join("intent-recognizer.cpp"))
        .file(core_dir.join("speaker-embedding-model.cpp"))
        .file(core_dir.join("speaker-embedding-model-data.cpp"))
        .file(core_dir.join("online-clusterer.cpp"))
        // ---- ort-utils sub-library ----
        .file(core_dir.join("ort-utils/ort-utils.cpp"))
        .file(core_dir.join("ort-utils/moonshine-ort-allocator.cpp"))
        .file(core_dir.join("ort-utils/moonshine-tensor-view.cpp"))
        .file(core_dir.join("ort-utils/moonshine-tensor.cpp"))
        // ---- bin-tokenizer sub-library ----
        .file(core_dir.join("bin-tokenizer/bin-tokenizer.cpp"))
        // ---- moonshine-utils sub-library ----
        .file(core_dir.join("moonshine-utils/string-utils.cpp"))
        .file(core_dir.join("moonshine-utils/debug-utils.cpp"))
        // ---- Include directories ----
        .include(core_dir)
        .include(core_dir.join("moonshine-utils"))
        .include(core_dir.join("ort-utils"))
        .include(core_dir.join("bin-tokenizer"))
        .include(core_dir.join("third-party/onnxruntime/include"))
        .include(core_dir.join("third-party/utf-8"))
        .compile("moonshine_core");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Runs a command, panicking with a helpful message on failure.
fn run_command(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to run {}: {e}", cmd.get_program().display()));
    assert!(status.success(), "Command {} failed with {status}", cmd.get_program().display());
}
