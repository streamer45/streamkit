// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! SenseVoice STT native plugin for StreamKit
//!
//! Provides multilingual speech recognition for Chinese (Mandarin/Cantonese),
//! English, Japanese, and Korean using sherpa-onnx.

mod config;
mod ffi;
mod sensevoice_node;
mod vad;

use sensevoice_node::SenseVoiceNode;
use streamkit_plugin_sdk_native::prelude::*;

// Export the plugin entry point
native_plugin_entry!(SenseVoiceNode);
