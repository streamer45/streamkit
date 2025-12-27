// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Voice Activity Detection (VAD) plugin using ten-vad
//!
//! This plugin provides real-time voice activity detection with configurable output modes:
//! - **Events mode**: Emits JSON events on speech start/stop transitions
//! - **Filtered audio mode**: Passes through only audio segments containing speech
//!
//! The plugin uses ten-vad from the TEN-framework via sherpa-onnx C API.

mod config;
mod ffi;
mod vad_node;

use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};
use vad_node::VadNode;

native_plugin_entry!(VadNode);
