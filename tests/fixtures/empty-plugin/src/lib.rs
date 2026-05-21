// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A test fixture that compiles to a valid `.so` but exports no
//! `streamkit_native_plugin_api` symbol. Used to exercise the host's
//! "missing entry symbol" error path.

#[unsafe(no_mangle)]
pub extern "C" fn empty_plugin_unrelated_symbol() {}
