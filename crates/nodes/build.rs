// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Build script for streamkit-nodes.
//!
//! When the `svt_av1` feature is active, probes for `SvtAv1Enc` via pkg-config
//! and emits the necessary `rustc-link-lib` / `rustc-link-search` directives.

fn main() {
    #[cfg(feature = "svt_av1")]
    {
        if let Err(e) = pkg_config::Config::new().atleast_version("2.0.0").probe("SvtAv1Enc") {
            panic!(
                "SVT-AV1 >= 2.0 not found: {e}.  Install libsvtav1enc-dev (or build \
                 from source) and ensure pkg-config can locate SvtAv1Enc.pc.  \
                 On Ubuntu/Debian: sudo apt install libsvtav1enc-dev"
            );
        }
    }
}
