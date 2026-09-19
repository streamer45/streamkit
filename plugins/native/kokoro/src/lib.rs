// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod kokoro_node;

use kokoro_node::KokoroTtsNode;
use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};

native_plugin_entry!(KokoroTtsNode);
