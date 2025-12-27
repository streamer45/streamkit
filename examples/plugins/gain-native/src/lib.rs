// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A simple gain (volume) filter native plugin for StreamKit
//!
//! This plugin demonstrates how to write a basic audio processing node
//! using the StreamKit Native Plugin SDK with C ABI.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{AudioFormat, SampleFormat};

/// Configuration for the gain plugin
#[derive(Serialize, Deserialize)]
struct GainConfig {
    /// Gain in decibels (dB)
    #[serde(default = "default_gain")]
    gain_db: f32,
}

fn default_gain() -> f32 {
    0.0
}

/// The gain plugin state
///
/// Note: The native plugin runtime ensures that `process()` and `update_params()`
/// are never called concurrently - they are handled sequentially by the wrapper's
/// tokio::select! loop, so no additional synchronization is needed.
pub struct GainPlugin {
    /// Linear gain multiplier (converted from dB)
    gain: f32,
}

impl NativeProcessorNode for GainPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("gain")
            .input(
                "in",
                &[PacketType::RawAudio(AudioFormat {
                    sample_rate: 0, // Wildcard - accepts any sample rate
                    channels: 0,    // Wildcard - accepts any number of channels
                    sample_format: SampleFormat::F32,
                })],
            )
            .output(
                "out",
                PacketType::RawAudio(AudioFormat {
                    sample_rate: 0, // Wildcard - outputs same sample rate as input
                    channels: 0,    // Wildcard - outputs same channels as input
                    sample_format: SampleFormat::F32,
                }),
            )
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "gain_db": {
                        "type": "number",
                        "description": "Gain in decibels",
                        "default": 0.0,
                        "minimum": -60.0,
                        "maximum": 20.0
                    }
                }
            }))
            .category("audio")
            .category("filters")
            .build()
    }

    fn new(params: Option<Value>, _logger: Logger) -> Result<Self, String> {
        let config: GainConfig = if let Some(p) = params {
            serde_json::from_value(p).map_err(|e| format!("Invalid config: {}", e))?
        } else {
            GainConfig {
                gain_db: default_gain(),
            }
        };

        Ok(Self {
            gain: db_to_linear(config.gain_db),
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Audio(mut frame) => {
                // Apply gain to all samples using copy-on-write
                for sample in frame.make_samples_mut() {
                    *sample *= self.gain;
                }
                output.send("out", &Packet::Audio(frame))?;
                Ok(())
            }
            // We only accept audio packets (enforced by type system)
            _ => Err("Gain plugin only accepts audio packets".to_string()),
        }
    }

    fn update_params(&mut self, params: Option<Value>) -> Result<(), String> {
        if let Some(p) = params {
            let config: GainConfig =
                serde_json::from_value(p).map_err(|e| format!("Invalid config: {}", e))?;
            self.gain = db_to_linear(config.gain_db);
        }
        Ok(())
    }
}

/// Convert decibels to linear gain
fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

// Export the plugin entry point
native_plugin_entry!(GainPlugin);
