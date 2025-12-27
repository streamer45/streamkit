// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Build scripts need println! to communicate with Cargo
#![allow(clippy::disallowed_macros)]

fn main() {
    // Tell cargo to rerun this build script if the WIT file changes
    println!("cargo:rerun-if-changed=../../wit/plugin.wit");
}
