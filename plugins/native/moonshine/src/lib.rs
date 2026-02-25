// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Moonshine STT native plugin for StreamKit
//!
//! Provides streaming speech-to-text transcription using the Moonshine model family.
//! Supports both streaming (partial results) and non-streaming modes with built-in VAD.

mod config;
mod ffi;
mod moonshine_node;

use moonshine_node::MoonshineNode;
use streamkit_plugin_sdk_native::prelude::*;

// Export the plugin entry point
native_plugin_entry!(MoonshineNode);
