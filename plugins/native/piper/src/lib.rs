// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::cognitive_complexity)] // Complex TTS initialization

mod config;
mod ffi;
mod piper_node;
mod sentence_splitter;

use piper_node::PiperTtsNode;
use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};

native_plugin_entry!(PiperTtsNode);
