// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Parakeet TDT STT native plugin for StreamKit
//!
//! Provides fast English speech recognition using NVIDIA's Parakeet TDT
//! transducer model via sherpa-onnx. Approximately 10x faster than Whisper
//! on consumer hardware with competitive accuracy.

mod config;
mod ffi;
mod parakeet_node;
mod vad;

use parakeet_node::ParakeetNode;
use streamkit_plugin_sdk_native::prelude::*;

// Export the plugin entry point
native_plugin_entry!(ParakeetNode);
