// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for streamkit-nodes.
//!
//! When the `svt_av1` feature is active this script makes the SVT-AV1 encoder
//! library available to the crate.  Two mutually-exclusive linking strategies
//! are supported:
//!
//! * **`svt_av1_static`** (recommended) — downloads a pinned SVT-AV1 release
//!   at build time, compiles it with `cmake`, and links statically.  No prior
//!   system installation is required; only `cmake`, a C compiler, and
//!   (optionally) `nasm` need to be present.
//!
//! * **`svt_av1`** (without `svt_av1_static`) — probes for a system-installed
//!   `SvtAv1Enc` via `pkg-config` and links dynamically.
//!
//! In both cases a small C snippet is compiled to verify that the Rust-side
//! opaque configuration buffer is large enough for the installed headers.

/// Pinned SVT-AV1 version used by the static build path.
#[cfg(feature = "svt_av1")]
const SVT_AV1_VERSION: &str = "4.1.0";

fn main() {
    #[cfg(feature = "svt_av1")]
    {
        let include_paths: Vec<std::path::PathBuf>;

        if cfg!(feature = "svt_av1_static") {
            include_paths = build_svt_av1_static();
        } else {
            include_paths = probe_svt_av1_pkgconfig();
        }

        // Compile a tiny C program that static-asserts our Rust-side opaque
        // buffer (8192 bytes) is at least as large as the real struct.
        // This catches ABI breakage at build time rather than at runtime.
        let mut build = cc::Build::new();
        build.file("src/video/svt_av1_config_size_check.c");
        for path in &include_paths {
            build.include(path);
        }
        build.compile("svt_av1_config_size_check");
    }
}

/// Probe for a system-installed SVT-AV1 via pkg-config (existing path).
#[cfg(feature = "svt_av1")]
fn probe_svt_av1_pkgconfig() -> Vec<std::path::PathBuf> {
    let lib = match pkg_config::Config::new().atleast_version("4.0.0").probe("SvtAv1Enc") {
        Ok(lib) => lib,
        Err(e) => panic!(
            "SVT-AV1 >= 4.0 not found: {e}.  Install libsvtav1enc (>= 4.0) from source \
             — see crates/nodes/SVT_AV1.md for instructions.  \
             Ubuntu/Debian distro packages ship very old versions and are NOT sufficient."
        ),
    };
    lib.include_paths
}

/// Download (if needed), build, and statically link SVT-AV1.
#[cfg(feature = "svt_av1")]
fn build_svt_av1_static() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=SVT_AV1_SRC_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    // 1. Determine source directory.
    let src_dir = if let Ok(dir) = std::env::var("SVT_AV1_SRC_DIR") {
        let p = PathBuf::from(dir);
        assert!(
            p.join("CMakeLists.txt").exists(),
            "SVT_AV1_SRC_DIR={} does not contain a CMakeLists.txt",
            p.display()
        );
        p
    } else {
        let dir_name = format!("SVT-AV1-v{SVT_AV1_VERSION}");
        let src = out_dir.join(&dir_name);

        // 2. Download if the source tree is not already present.
        if !src.join("CMakeLists.txt").exists() {
            let url = format!(
                "https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/v{V}/SVT-AV1-v{V}.tar.gz",
                V = SVT_AV1_VERSION
            );
            eprintln!("Downloading SVT-AV1 v{SVT_AV1_VERSION} from {url}");

            let curl = std::process::Command::new("curl")
                .args(["-fsSL", &url])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("failed to start curl — is curl installed?");

            let status = std::process::Command::new("tar")
                .args(["xz", "-C"])
                .arg(&out_dir)
                .stdin(curl.stdout.expect("curl stdout missing"))
                .status()
                .expect("failed to run tar");

            assert!(status.success(), "curl | tar failed for SVT-AV1 download");
            assert!(
                src.join("CMakeLists.txt").exists(),
                "SVT-AV1 source not found at {} after download",
                src.display()
            );
        }
        src
    };

    // 3. Build with cmake.
    let dst = cmake::Config::new(&src_dir)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_APPS", "OFF")
        .define("BUILD_DEC", "OFF")
        .define("BUILD_TESTING", "OFF")
        .build();

    // 4. Emit linker directives.
    println!("cargo:rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo:rustc-link-lib=static=SvtAv1Enc");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=pthread");

    // 5. Return include paths for the ABI check.
    vec![dst.join("include")]
}
