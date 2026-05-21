// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Test fixture plugin that exercises FFI failure modes.
//!
//! Behaviour is controlled by the `mode` field in the JSON params:
//!
//! - `"panic_process"` (default): panics inside `process` so the host's
//!   `catch_unwind` is exercised.
//! - `"error_process"`: returns `Err("..")` from `process`, exercising
//!   the plugin error-message extraction path.
//! - `"error_new"`: returns `Err(..)` from `new`, exercising the
//!   `create_instance` returning null path.
//! - `"error_update_params"`: returns `Err(..)` from `update_params`.
//! - `"error_flush"`: returns `Err(..)` from `flush`.
//! - `"passthrough"`: returns Ok from every method, forwards the packet
//!   to the output pin.

use std::sync::atomic::{AtomicUsize, Ordering};

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::{native_plugin_entry, NativeProcessorNode};

static PROCESS_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    PanicProcess,
    ErrorProcess,
    ErrorUpdateParams,
    ErrorFlush,
    Passthrough,
}

impl Mode {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "panic_process" => Some(Self::PanicProcess),
            "error_process" => Some(Self::ErrorProcess),
            "error_update_params" => Some(Self::ErrorUpdateParams),
            "error_flush" => Some(Self::ErrorFlush),
            "passthrough" => Some(Self::Passthrough),
            _ => None,
        }
    }
}

struct PanickingPlugin {
    mode: Mode,
}

impl NativeProcessorNode for PanickingPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("panicking")
            .description("Test plugin that exercises FFI failure modes")
            .input("input", &[PacketType::Text])
            .output("output", PacketType::Text)
            .build()
    }

    fn new(params: Option<serde_json::Value>, _logger: Logger) -> Result<Self, String> {
        let mode_str = params
            .as_ref()
            .and_then(|v| v.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("panic_process");

        if mode_str == "error_new" {
            return Err(format!("panicking-plugin refused to construct (mode={mode_str})"));
        }

        let mode = Mode::from_str(mode_str)
            .ok_or_else(|| format!("unknown panicking-plugin mode '{mode_str}'"))?;

        Ok(Self { mode })
    }

    fn process(
        &mut self,
        _pin: &str,
        packet: Packet,
        output: &OutputSender,
    ) -> Result<(), String> {
        PROCESS_CALLS.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::PanicProcess => {
                panic!("panicking-plugin: intentional panic in process_packet");
            },
            Mode::ErrorProcess => {
                Err("panicking-plugin: intentional error in process_packet".to_string())
            },
            Mode::Passthrough => output.send("output", &packet),
            // For modes that target other entry points, `process` is a no-op.
            Mode::ErrorUpdateParams | Mode::ErrorFlush => Ok(()),
        }
    }

    fn update_params(
        &mut self,
        _params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        if matches!(self.mode, Mode::ErrorUpdateParams) {
            return Err("panicking-plugin: intentional error in update_params".to_string());
        }
        Ok(())
    }

    fn flush(&mut self, _output: &OutputSender) -> Result<(), String> {
        if matches!(self.mode, Mode::ErrorFlush) {
            return Err("panicking-plugin: intentional error in flush".to_string());
        }
        Ok(())
    }
}

native_plugin_entry!(PanickingPlugin);
