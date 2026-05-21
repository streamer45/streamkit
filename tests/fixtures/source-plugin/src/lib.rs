// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Test fixture source plugin that exercises the tick-driven source-plugin
//! lifecycle in the native plugin host.
//!
//! Behaviour is controlled by the `mode` field in the JSON params:
//!
//! - `"emit_n"` (default): emits a fixed number of `Text` packets, one per
//!   tick, then returns `Ok(true)` to signal completion. The number of
//!   ticks is taken from the `max_ticks` JSON param (defaults to 3).
//! - `"error_tick"`: returns `Err(..)` from the first `tick` call so we
//!   exercise the host's tick-error path.

use std::sync::atomic::{AtomicUsize, Ordering};

use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::{native_source_plugin_entry, NativeSourceNode, SourceConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    EmitN,
    ErrorTick,
}

impl Mode {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "emit_n" => Some(Self::EmitN),
            "error_tick" => Some(Self::ErrorTick),
            _ => None,
        }
    }
}

struct TickingSource {
    mode: Mode,
    max_ticks: u64,
    ticks_done: AtomicUsize,
}

impl NativeSourceNode for TickingSource {
    fn metadata() -> NodeMetadata {
        // Declaring a category and an explicit param_schema exercises
        // the host's metadata extraction loop over categories (lines
        // ~293-297 in plugin-native/src/lib.rs) and the
        // non-empty param_schema parse branch (~line 282).
        NodeMetadata::builder("source-plugin")
            .description("Test source plugin that ticks and emits packets")
            .category("test")
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string" },
                    "max_ticks": { "type": "integer" }
                }
            }))
            .output("output", PacketType::Text)
            .build()
    }

    fn source_config(&self) -> SourceConfig {
        SourceConfig {
            // 1ms ticks keep tests fast.
            tick_interval_us: 1_000,
            max_ticks: self.max_ticks,
        }
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        // Exercising the host-supplied log callback during construction
        // forces coverage through `plugin_log_callback_noop` (used by the
        // host's source-config probe) and `host_log_callback` (used by
        // live instances).
        logger.info("source-plugin: constructing TickingSource");


        let mode_str = params
            .as_ref()
            .and_then(|v| v.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("emit_n");

        let max_ticks = params
            .as_ref()
            .and_then(|v| v.get("max_ticks"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(3);

        let mode = Mode::from_str(mode_str)
            .ok_or_else(|| format!("unknown source-plugin mode '{mode_str}'"))?;

        Ok(Self {
            mode,
            max_ticks,
            ticks_done: AtomicUsize::new(0),
        })
    }

    fn tick(&mut self, output: &OutputSender) -> Result<bool, String> {
        match self.mode {
            Mode::ErrorTick => {
                Err("source-plugin: intentional error in tick".to_string())
            },
            Mode::EmitN => {
                let n = self.ticks_done.fetch_add(1, Ordering::SeqCst) + 1;
                let payload = format!("tick-{n}");
                output.send("output", &Packet::Text(payload.into()))?;
                // Signal completion after max_ticks emissions.
                Ok(n as u64 >= self.max_ticks)
            },
        }
    }
}

native_source_plugin_entry!(TickingSource);
