// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod slint_node;
mod slint_thread;

use slint_node::SlintSourcePlugin;
use streamkit_plugin_sdk_native::{native_source_plugin_entry, NativeSourceNode};

native_source_plugin_entry!(SlintSourcePlugin);
