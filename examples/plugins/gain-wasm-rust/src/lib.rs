// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A simple gain (volume) filter plugin for StreamKit
//!
//! This plugin demonstrates how to write a basic audio processing node
//! using the StreamKit Plugin SDK with the Component Model.

use serde_json::Value;
use std::sync::Mutex;
use streamkit_plugin_sdk_wasm as sdk;

// Generate bindings, reusing SDK types for faster compilation
wit_bindgen::generate!({
    world: "plugin",
    path: "../../../wit",
    generate_all,
    with: {
        "streamkit:plugin/types@0.1.0": sdk::types,
        "streamkit:plugin/host@0.1.0": sdk::host,
    },
});

// Import the generated traits
use exports::streamkit::plugin::node::{Guest, GuestNodeInstance};

// Use SDK types directly
use sdk::{AudioFormat, InputPin, NodeMetadata, OutputPin, Packet, PacketType, SampleFormat};

// Root type for this plugin export
struct GainPlugin;

// Per-instance state for a single node instance
struct GainInstance {
    gain_linear: Mutex<f32>,
}

impl Guest for GainPlugin {
    type NodeInstance = GainInstance;

    fn metadata() -> NodeMetadata {
        NodeMetadata {
            kind: "gain_filter_rust".to_string(),
            inputs: vec![InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::RawAudio(AudioFormat {
                    sample_rate: 48000,
                    channels: 1,
                    sample_format: SampleFormat::Float32,
                })],
            }],
            outputs: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::RawAudio(AudioFormat {
                    sample_rate: 48000,
                    channels: 1,
                    sample_format: SampleFormat::Float32,
                }),
            }],
            param_schema: r#"{
                 "type": "object",
                 "properties": {
                     "gain_db": {
                         "type": "number",
                         "default": 0.0,
                         "description": "Gain in decibels (dB)",
                         "minimum": -60.0,
                         "maximum": 20.0
                     }
                 }
             }"#
            .to_string(),
            categories: vec!["audio".to_string(), "filters".to_string()],
        }
    }
}

impl GuestNodeInstance for GainInstance {
    fn new(params: Option<String>) -> Self {
        // Parse parameters (expecting {"gain_db": <number>})
        let gain_db = params
            .as_deref()
            .and_then(|params_str| serde_json::from_str::<Value>(params_str).ok())
            .and_then(|value| value.get("gain_db").and_then(|v| v.as_f64()))
            .unwrap_or(0.0) as f32;

        let gain_linear = 10.0_f32.powf(gain_db / 20.0);

        sdk::host::log(
            sdk::host::LogLevel::Info,
            &format!(
                "Gain filter instance constructed: {}dB (linear: {:.3})",
                gain_db, gain_linear
            ),
        );

        Self {
            gain_linear: Mutex::new(gain_linear),
        }
    }

    fn process(&self, _input_pin: String, packet: Packet) -> Result<(), String> {
        match packet {
            Packet::Audio(mut audio_frame) => {
                let gain = *self
                    .gain_linear
                    .lock()
                    .map_err(|_| "Gain state lock poisoned".to_string())?;
                // Apply gain to all samples
                for sample in &mut audio_frame.samples {
                    *sample *= gain;
                }

                // Send the processed audio to the output
                sdk::host::send_output("out", &Packet::Audio(audio_frame))?;
                Ok(())
            }
            _ => Err("Gain filter only accepts audio packets".to_string()),
        }
    }

    fn update_params(&self, params: Option<String>) -> Result<(), String> {
        let Some(params_str) = params else {
            return Ok(());
        };

        let value = serde_json::from_str::<Value>(&params_str)
            .map_err(|e| format!("Failed to parse params JSON: {e}"))?;

        if let Some(gain_db_val) = value.get("gain_db").and_then(|v| v.as_f64()) {
            let gain_db = gain_db_val as f32;
            let gain_linear = 10.0_f32.powf(gain_db / 20.0);

            let mut guard = self
                .gain_linear
                .lock()
                .map_err(|_| "Gain state lock poisoned".to_string())?;
            *guard = gain_linear;

            sdk::host::log(
                sdk::host::LogLevel::Info,
                &format!("Gain updated via params: {gain_db}dB (linear: {gain_linear:.3})"),
            );
        }

        Ok(())
    }

    fn cleanup(&self) {
        sdk::host::log(sdk::host::LogLevel::Info, "Gain filter instance shutting down");
    }
}

// Export the plugin using the generated macro
export!(GainPlugin);
