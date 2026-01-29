// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod model;
mod sentence_splitter;
mod supertonic_node;
mod voice;

use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};
use supertonic_node::SupertonicNode;

native_plugin_entry!(SupertonicNode);
