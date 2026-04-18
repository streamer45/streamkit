// SPDX-FileCopyrightText: (c) 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod servo_node;
mod servo_thread;

use servo_node::ServoSourcePlugin;
use streamkit_plugin_sdk_native::{native_source_plugin_entry, NativeSourceNode};

native_source_plugin_entry!(ServoSourcePlugin);
