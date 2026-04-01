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
//! When the `dav1d` feature is active this script makes the C dav1d decoder
//! library available.  Two linking strategies mirror the SVT-AV1 pattern:
//!
//! * **`dav1d_static`** — downloads a pinned dav1d release at build time,
//!   compiles it with `meson` + `ninja`, and links statically.
//!
//! * **`dav1d`** (without `dav1d_static`) — probes for a system-installed
//!   `dav1d` via `pkg-config` and links dynamically.
//!
//! **Note:** the static build paths currently target Linux (uses `sha256sum`
//! for checksum verification).  macOS developers should use the pkg-config
//! path or set `SVT_AV1_SRC_DIR` / `DAV1D_SRC_DIR` to a pre-downloaded
//! source tree.
//!
//! In both cases a small C snippet is compiled to verify that the Rust-side
//! opaque configuration buffers are large enough for the installed headers.

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

    #[cfg(feature = "dav1d")]
    {
        #[cfg(feature = "dav1d_static")]
        let dav1d_includes = build_dav1d_static();
        #[cfg(not(feature = "dav1d_static"))]
        let dav1d_includes = probe_dav1d_pkgconfig();

        // Compile the ABI check — verifies opaque buffer sizes and field
        // offsets match the installed dav1d headers.
        let mut build = cc::Build::new();
        build.file("src/video/dav1d_abi_check.c");
        for path in &dav1d_includes {
            build.include(path);
        }
        build.compile("dav1d_abi_check");
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

// ---------------------------------------------------------------------------
// dav1d (C AV1 decoder)
// ---------------------------------------------------------------------------

/// Pinned dav1d version used by the static build path.
#[cfg(feature = "dav1d_static")]
const DAV1D_VERSION: &str = "1.5.3";

/// SHA-256 of the pinned dav1d source tarball for integrity verification.
#[cfg(feature = "dav1d_static")]
const DAV1D_SHA256: &str = "cbe212b02faf8c6eed5b6d55ef8a6e363aaab83f15112e960701a9c3df813686";

/// Probe for a system-installed dav1d via pkg-config.
#[cfg(all(feature = "dav1d", not(feature = "dav1d_static")))]
fn probe_dav1d_pkgconfig() -> Vec<std::path::PathBuf> {
    let lib = match pkg_config::Config::new().atleast_version("1.0.0").probe("dav1d") {
        Ok(lib) => lib,
        Err(e) => panic!(
            "dav1d >= 1.0 not found: {e}.  Install libdav1d-dev (>= 1.0) or \
             enable the `dav1d_static` feature to build from source."
        ),
    };
    lib.include_paths
}

/// Download (if needed), build, and statically link dav1d via meson + ninja.
#[cfg(feature = "dav1d_static")]
fn build_dav1d_static() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=DAV1D_SRC_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    // 1. Determine source directory.
    let src_dir = if let Ok(dir) = std::env::var("DAV1D_SRC_DIR") {
        let p = PathBuf::from(dir);
        assert!(
            p.join("meson.build").exists(),
            "DAV1D_SRC_DIR={} does not contain a meson.build",
            p.display()
        );
        println!(
            "cargo:warning=dav1d_static: using pre-downloaded source from DAV1D_SRC_DIR={}",
            p.display()
        );
        p
    } else {
        let dir_name = format!("dav1d-{DAV1D_VERSION}");
        let src = out_dir.join(&dir_name);

        // 2. Download if the source tree is not already present.
        let sentinel = src.join(".dav1d_ok");
        if !sentinel.exists() {
            if src.exists() {
                std::fs::remove_dir_all(&src).ok();
            }
            let url = format!(
                "https://code.videolan.org/videolan/dav1d/-/archive/{V}/dav1d-{V}.tar.gz",
                V = DAV1D_VERSION
            );
            println!("cargo:warning=dav1d_static: downloading dav1d v{DAV1D_VERSION} ...");

            let tarball = out_dir.join(format!("dav1d-{DAV1D_VERSION}.tar.gz"));

            let curl_status = std::process::Command::new("curl")
                .args(["-fsSL", "-o"])
                .arg(&tarball)
                .arg(&url)
                .status()
                .expect("failed to run curl — is curl installed?");
            assert!(
                curl_status.success(),
                "curl failed to download dav1d tarball (exit status: {curl_status})"
            );

            println!("cargo:warning=dav1d_static: download complete, verifying SHA-256 ...");

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
            assert_eq!(actual_sha, DAV1D_SHA256, "dav1d tarball SHA-256 mismatch");

            println!("cargo:warning=dav1d_static: checksum OK, extracting ...");

            let tar_status = std::process::Command::new("tar")
                .args(["-xzf"])
                .arg(&tarball)
                .arg("-C")
                .arg(&out_dir)
                .status()
                .expect("failed to run tar");
            assert!(tar_status.success(), "tar extraction failed for dav1d");

            std::fs::remove_file(&tarball).ok();
            assert!(
                src.join("meson.build").exists(),
                "dav1d source not found at {} after download",
                src.display()
            );

            std::fs::write(&sentinel, b"ok").ok();
        }
        src
    };

    // 3. Build with meson + ninja.
    let install_dir = out_dir.join("dav1d-install");
    let build_dir = out_dir.join("dav1d-build");

    // Only rebuild if the install dir doesn't already exist (cached builds).
    if !install_dir.join("lib").exists() && !install_dir.join("lib64").exists() {
        if build_dir.exists() {
            std::fs::remove_dir_all(&build_dir).ok();
        }
        if install_dir.exists() {
            std::fs::remove_dir_all(&install_dir).ok();
        }

        println!("cargo:warning=dav1d_static: building dav1d with meson + ninja (this may take a minute) ...");

        let meson_status = std::process::Command::new("meson")
            .arg("setup")
            .arg(&build_dir)
            .arg(&src_dir)
            .arg("--default-library=static")
            .arg(format!("--prefix={}", install_dir.display()))
            .arg("--buildtype=release")
            .arg("-Denable_tools=false")
            .arg("-Denable_tests=false")
            .arg("-Denable_examples=false")
            .status()
            .expect("failed to run meson — is meson installed?");
        assert!(meson_status.success(), "meson setup failed (exit status: {meson_status})");

        let ninja_status = std::process::Command::new("ninja")
            .arg("-C")
            .arg(&build_dir)
            .arg("install")
            .status()
            .expect("failed to run ninja — is ninja installed?");
        assert!(ninja_status.success(), "ninja install failed (exit status: {ninja_status})");
    }

    println!("cargo:warning=dav1d_static: build complete, linking statically");

    // 4. Emit linker directives.
    // The lib directory may be `lib/`, `lib64/`, or `lib/x86_64-linux-gnu/`
    // depending on the distro and meson version.
    let lib_dir = if install_dir.join("lib/x86_64-linux-gnu").exists() {
        install_dir.join("lib/x86_64-linux-gnu")
    } else if install_dir.join("lib64").exists() {
        install_dir.join("lib64")
    } else {
        install_dir.join("lib")
    };

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=dav1d");
    println!("cargo:rustc-link-lib=pthread");

    // 5. Return include paths for the ABI check.
    vec![install_dir.join("include")]
}
