// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;
mod servo_node;
mod servo_thread;

use servo_node::ServoSourcePlugin;
use streamkit_plugin_sdk_native::{native_source_plugin_entry, NativeSourceNode};

native_source_plugin_entry!(ServoSourcePlugin);

/// Internal surface re-exported solely for this crate's integration tests.
///
/// This crate ships as a `cdylib` host plugin and has no stable Rust API;
/// nothing here is part of a public contract and it may change at any time.
#[doc(hidden)]
pub mod test_api {
    pub use crate::config::ServoConfig;
    pub use crate::servo_thread::{send_work, NodeId, ServoThreadResult, ServoWorkItem};
    pub use streamkit_plugin_sdk_native::prelude::{CLogLevel, Logger};
}
