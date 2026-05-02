// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Test fixture plugin that panics inside `process_packet`.
//!
//! Used by the `panicking_plugin` integration test to verify that the
//! host (pipeline) survives a plugin panic without aborting.

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};

struct PanickingPlugin;

impl NativeProcessorNode for PanickingPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("panicking")
            .description("Test plugin that panics in process_packet")
            .input("input", &[PacketType::Text])
            .output("output", PacketType::Text)
            .build()
    }

    fn new(_params: Option<serde_json::Value>, _logger: Logger) -> Result<Self, String> {
        Ok(Self)
    }

    fn process(
        &mut self,
        _pin: &str,
        _packet: Packet,
        _output: &OutputSender,
    ) -> Result<(), String> {
        panic!("panicking-plugin: intentional panic in process_packet");
    }
}

native_plugin_entry!(PanickingPlugin);
