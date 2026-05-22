// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A test fixture that compiles to a valid `.so` but exports no
//! `streamkit_native_plugin_api` symbol. Used to exercise the host's
//! "missing entry symbol" error path.

// SAFETY: `no_mangle` is used solely so the dlopen'd host can resolve a
// stable C symbol against this cdylib. The function has no parameters,
// no return value, and no internal state, so there is no FFI contract
// to honour beyond the C ABI itself. It exists only to give the
// resulting `.so` a non-empty exported symbol table while deliberately
// omitting `streamkit_native_plugin_api`.
#[unsafe(no_mangle)]
pub extern "C" fn empty_plugin_unrelated_symbol() {}
