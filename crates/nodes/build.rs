// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for streamkit-nodes.
//!
//! When the `svt_av1` feature is active, probes for `SvtAv1Enc` via pkg-config
//! and emits the necessary `rustc-link-lib` / `rustc-link-search` directives.
//! It also compiles a small C snippet to verify that our opaque configuration
//! buffer is large enough for the installed version of SVT-AV1.

fn main() {
    #[cfg(feature = "svt_av1")]
    {
        let lib = match pkg_config::Config::new().atleast_version("4.0.0").probe("SvtAv1Enc") {
            Ok(lib) => lib,
            Err(e) => panic!(
                "SVT-AV1 >= 4.0 not found: {e}.  Install libsvtav1enc (>= 4.0) from source \
                 — see crates/nodes/SVT_AV1.md for instructions.  \
                 Ubuntu/Debian distro packages ship very old versions and are NOT sufficient."
            ),
        };

        // Compile a tiny C program that static-asserts our Rust-side opaque
        // buffer (8192 bytes) is at least as large as the real struct.
        // This catches ABI breakage at build time rather than at runtime.
        let mut build = cc::Build::new();
        build.file("src/video/svt_av1_config_size_check.c");
        for path in &lib.include_paths {
            build.include(path);
        }
        build.compile("svt_av1_config_size_check");
    }
}
