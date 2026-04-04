// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for streamkit-nodes.
//!
//! When the `svt_av1` feature is active, probes for `SvtAv1Enc` via pkg-config
//! (or builds it from source when `svt_av1_static` is also enabled) and emits
//! the necessary `rustc-link-lib` / `rustc-link-search` directives.  It also
//! compiles a small C snippet to verify that our opaque configuration buffer is
//! large enough for the installed version of SVT-AV1.
//!
//! When `dav1d_static` is active, downloads and builds dav1d from source via
//! meson + ninja, then emits link directives for the resulting static library.
//!
//! ## Static linking workaround (svt_av1_static + dav1d_static)
//!
//! Under the `release-lto` profile (thin LTO via rust-lld), passing both
//! `libSvtAv1Enc.a` and `libdav1d.a` as separate `cargo:rustc-link-lib=static`
//! archives causes rust-lld to silently drop SVT-AV1 symbols.  The workaround
//! extracts the `.o` files from `libSvtAv1Enc.a` and bundles them directly into
//! the `cc::Build` output via `build.object()`, embedding them in the rlib so
//! they travel through the LTO pipeline alongside the Rust code.

#[cfg(feature = "svt_av1_static")]
use std::path::PathBuf;

// ── SVT-AV1 version for static builds ────────────────────────────────────────

/// SVT-AV1 release tag to download when `svt_av1_static` is active.
#[cfg(feature = "svt_av1_static")]
const SVT_AV1_VERSION: &str = "4.1.0";

// ── dav1d version for static builds ──────────────────────────────────────────

/// dav1d release tag to download when `dav1d_static` is active.
#[cfg(feature = "dav1d_static")]
const DAV1D_VERSION: &str = "1.5.0";

// ── Static build helpers ─────────────────────────────────────────────────────

/// Download SVT-AV1 source, build as a static library with cmake.
///
/// Does **not** emit `cargo:rustc-link-lib=static=SvtAv1Enc` — the caller
/// bundles the object files directly via `cc::Build::object()` to work around
/// a rust-lld thin-LTO bug when multiple native archives are present.
///
/// Emits `cargo:rustc-link-lib` for system dependencies (`m`, `pthread`).
///
/// Returns the include paths needed to compile the ABI size-check C file.
#[cfg(feature = "svt_av1_static")]
fn build_svt_av1_static() -> Vec<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let src_dir = out_dir.join("svt-av1-src");

    // Download source if not already cached in OUT_DIR.
    if !src_dir.join("CMakeLists.txt").exists() {
        if src_dir.exists() {
            std::fs::remove_dir_all(&src_dir).ok();
        }
        let status = std::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                &format!("v{SVT_AV1_VERSION}"),
                "https://gitlab.com/AOMediaCodec/SVT-AV1.git",
            ])
            .arg(&src_dir)
            .status()
            .expect("failed to run git — is git installed?");
        assert!(status.success(), "git clone SVT-AV1 v{SVT_AV1_VERSION} failed");
    }

    let dst = cmake::Config::new(&src_dir)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_APPS", "OFF")
        .define("BUILD_DEC", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("SVT_AV1_LTO", "OFF")
        .define("CMAKE_BUILD_TYPE", "Release")
        .build();

    // Only emit system deps — the archive itself is bundled via object files
    // in main() to work around the rust-lld issue.
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=pthread");

    // Return include paths for the ABI size-check compilation.
    vec![dst.join("include").join("svt-av1")]
}

/// Probe for an installed SVT-AV1 (≥ 4.0) via pkg-config.
///
/// Returns the include paths reported by pkg-config.
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

/// Download dav1d source, build as a static library with meson + ninja.
///
/// Emits `cargo:rustc-link-search`, `cargo:rustc-link-lib=static=dav1d`,
/// and system dependency link directives.
#[cfg(feature = "dav1d_static")]
fn build_dav1d_static() {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let src_dir = out_dir.join("dav1d-src");
    let build_dir = out_dir.join("dav1d-build");

    // Download source if not already cached in OUT_DIR.
    if !src_dir.join("meson.build").exists() {
        if src_dir.exists() {
            std::fs::remove_dir_all(&src_dir).ok();
        }
        let status = std::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                DAV1D_VERSION,
                "https://code.videolan.org/videolan/dav1d.git",
            ])
            .arg(&src_dir)
            .status()
            .expect("failed to run git — is git installed?");
        assert!(status.success(), "git clone dav1d {DAV1D_VERSION} failed");
    }

    // Configure with meson (only if not already configured).
    if !build_dir.join("build.ninja").exists() {
        let status = std::process::Command::new("meson")
            .arg("setup")
            .arg(&build_dir)
            .arg(&src_dir)
            .args([
                "--default-library=static",
                "--buildtype=release",
                "-Denable_tools=false",
                "-Denable_tests=false",
            ])
            .status()
            .expect("failed to run meson — are meson, ninja-build, and python3 installed?");
        assert!(status.success(), "meson setup for dav1d failed");
    }

    // Build with ninja.
    let status = std::process::Command::new("ninja")
        .args(["-C"])
        .arg(&build_dir)
        .status()
        .expect("failed to run ninja — is ninja-build installed?");
    assert!(status.success(), "ninja build for dav1d failed");

    // The static library lands in build_dir/src/libdav1d.a
    let lib_dir = build_dir.join("src");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=dav1d");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=dl");
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    // ── SVT-AV1 ──────────────────────────────────────────────────────────
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

        // When linking statically, bundle the SVT-AV1 object files directly
        // into the cc output.  This embeds the symbols in the rlib so that
        // cargo's thin-LTO link step (via rust-lld) finds them reliably.
        // Passing the archive via `cargo:rustc-link-lib=static=SvtAv1Enc`
        // fails under rust-lld + thin LTO when multiple native archives are
        // present (dav1d_static + svt_av1_static).
        #[cfg(feature = "svt_av1_static")]
        {
            let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
            let lib_path = out_dir.join("lib").join("libSvtAv1Enc.a");
            let objs_dir = out_dir.join("svt_av1_objs");
            if objs_dir.exists() {
                std::fs::remove_dir_all(&objs_dir).ok();
            }
            std::fs::create_dir_all(&objs_dir).expect("failed to create svt_av1_objs directory");
            let status = std::process::Command::new("ar")
                .arg("x")
                .arg(&lib_path)
                .current_dir(&objs_dir)
                .status()
                .expect("failed to extract SVT-AV1 archive with ar");
            assert!(status.success(), "ar x failed for libSvtAv1Enc.a");
            let mut obj_files: Vec<PathBuf> = std::fs::read_dir(&objs_dir)
                .expect("failed to read svt_av1_objs directory")
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "o"))
                .map(|e| e.path())
                .collect();
            // Sort for deterministic build output.
            obj_files.sort();
            for obj in &obj_files {
                build.object(obj);
            }
        }

        build.compile("svt_av1_config_size_check");
    }

    // ── dav1d ────────────────────────────────────────────────────────────
    #[cfg(feature = "dav1d_static")]
    build_dav1d_static();
}
