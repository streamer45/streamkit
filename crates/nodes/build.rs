// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for streamkit-nodes.
//!
//! When the `svt_av1` feature is active this script makes the SVT-AV1 encoder
//! library available to the crate.  Two linking strategies are supported;
//! when `svt_av1_static` is active it takes precedence over the pkg-config
//! path:
//!
//! * **`svt_av1_static`** (recommended) — downloads a pinned SVT-AV1 release
//!   at build time, compiles it with `cmake`, and links statically.  No prior
//!   system installation is required; only `cmake`, a C compiler, and
//!   (optionally) `nasm` need to be present.
//!
//! * **`svt_av1`** (without `svt_av1_static`) — probes for a system-installed
//!   `SvtAv1Enc` via `pkg-config` and links dynamically.
//!
//! **Note:** the static build path currently targets Linux (uses `sha256sum`
//! for checksum verification).  macOS developers should use the pkg-config
//! path or set `SVT_AV1_SRC_DIR` to a pre-downloaded source tree.
//!
//! In both cases a small C snippet is compiled to verify that the Rust-side
//! opaque configuration buffer is large enough for the installed headers.

/// Pinned SVT-AV1 version used by the static build path.
/// NOTE: keep in sync with the version referenced in `SVT_AV1.md`.
#[cfg(feature = "svt_av1_static")]
const SVT_AV1_VERSION: &str = "4.1.0";

/// SHA-256 of the pinned SVT-AV1 source tarball for integrity verification.
#[cfg(feature = "svt_av1_static")]
const SVT_AV1_SHA256: &str = "6c4c0c44ff0ba3d136d6f57f3a707f9de8e9c866f50f809c1d22a43f0d8c9583";

fn main() {
    #[cfg(feature = "svt_av1")]
    {
        #[cfg(feature = "svt_av1_static")]
        let include_paths = build_svt_av1_static();
        #[cfg(not(feature = "svt_av1_static"))]
        let include_paths = probe_svt_av1_pkgconfig();

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
#[cfg(all(feature = "svt_av1", not(feature = "svt_av1_static")))]
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
#[cfg(feature = "svt_av1_static")]
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
        println!(
            "cargo:warning=svt_av1_static: using pre-downloaded source from SVT_AV1_SRC_DIR={}",
            p.display()
        );
        p
    } else {
        let dir_name = format!("SVT-AV1-v{SVT_AV1_VERSION}");
        let src = out_dir.join(&dir_name);

        // 2. Download if the source tree is not already present.
        // A sentinel file gates the cache so partial extractions don't
        // poison subsequent builds.
        let sentinel = src.join(".svt_av1_ok");
        if !sentinel.exists() {
            // Clean up any partial previous extraction.
            if src.exists() {
                std::fs::remove_dir_all(&src).ok();
            }
            let url = format!(
                "https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/v{V}/SVT-AV1-v{V}.tar.gz",
                V = SVT_AV1_VERSION
            );
            println!("cargo:warning=svt_av1_static: downloading SVT-AV1 v{SVT_AV1_VERSION} ...");

            let tarball = out_dir.join(format!("SVT-AV1-v{SVT_AV1_VERSION}.tar.gz"));

            let curl_status = std::process::Command::new("curl")
                .args(["-fsSL", "-o"])
                .arg(&tarball)
                .arg(&url)
                .status()
                .expect("failed to run curl — is curl installed?");
            assert!(
                curl_status.success(),
                "curl failed to download SVT-AV1 tarball (exit status: {curl_status})"
            );

            println!("cargo:warning=svt_av1_static: download complete, verifying SHA-256 ...");

            // Verify tarball integrity.
            let sha_output = std::process::Command::new("sha256sum")
                .arg(&tarball)
                .output()
                .expect("failed to run sha256sum");
            assert!(
                sha_output.status.success(),
                "sha256sum failed (exit status: {})",
                sha_output.status
            );
            let sha_line = String::from_utf8_lossy(&sha_output.stdout);
            let actual_sha = sha_line.split_whitespace().next().unwrap_or("");
            assert_eq!(actual_sha, SVT_AV1_SHA256, "SVT-AV1 tarball SHA-256 mismatch");

            println!("cargo:warning=svt_av1_static: checksum OK, extracting ...");

            let tar_status = std::process::Command::new("tar")
                .args(["-xzf"])
                .arg(&tarball)
                .arg("-C")
                .arg(&out_dir)
                .status()
                .expect("failed to run tar");
            assert!(tar_status.success(), "tar extraction failed for SVT-AV1");

            std::fs::remove_file(&tarball).ok();
            assert!(
                src.join("CMakeLists.txt").exists(),
                "SVT-AV1 source not found at {} after download",
                src.display()
            );

            // Mark extraction as complete so we don't re-download next time.
            std::fs::write(&sentinel, b"ok").ok();
        }
        src
    };

    // 3. Build with cmake.
    // Always build SVT-AV1 in Release mode regardless of the Cargo profile.
    // The cmake crate inherits OPT_LEVEL from Cargo, which means `cargo build`
    // (debug) would produce CMAKE_BUILD_TYPE=Debug — an un-optimised SVT-AV1
    // that is too slow for real-time encoding and causes audio/video desync.
    println!("cargo:warning=svt_av1_static: building SVT-AV1 with cmake (this may take a few minutes) ...");
    let dst = cmake::Config::new(&src_dir)
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_APPS", "OFF")
        .define("BUILD_DEC", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .build();

    println!("cargo:warning=svt_av1_static: build complete, linking statically");

    // 4. Emit linker directives.
    println!("cargo:rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo:rustc-link-lib=static=SvtAv1Enc");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=pthread");

    // 5. Return include paths for the ABI check.
    vec![dst.join("include")]
}
