// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Support crate shared by the official native plugins: FFI bindings to
//! external model libraries and thin Rust wrappers around them.
//!
//! Not a plugin itself — this crate only exists so each plugin does not
//! carry its own copy of the same bindings. It is deliberately excluded
//! from the marketplace plugin enumeration in
//! `scripts/marketplace/generate_official_plugins.py` and
//! `scripts/marketplace/build_official_plugins.sh`.

pub mod sherpa_onnx;
