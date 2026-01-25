// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod model;
mod sentence_splitter;
mod voice;

mod pocket_tts_node;

use pocket_tts_node::PocketTtsNode;
use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};

native_plugin_entry!(PocketTtsNode);
