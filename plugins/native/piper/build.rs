// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow: println! in build.rs is the standard way to communicate with Cargo, not logging
#![allow(clippy::disallowed_macros)]

fn main() {
    // Link against libsherpa-onnx-c-api (not libsherpa-onnx)
    println!("cargo:rustc-link-lib=sherpa-onnx-c-api");

    // Common library search paths
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/opt/homebrew/lib");

    // Add rpath so the plugin can find sherpa-onnx at runtime
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");
}
