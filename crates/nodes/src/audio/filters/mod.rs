// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Use the node structs from their respective files.
use streamkit_core::{
    config_helpers, registry::StaticPins, NodeRegistry, ProcessorNode, StreamKitError,
};

pub mod gain;
use gain::{AudioGainConfig, AudioGainNode};
pub mod mixer;
use mixer::{AudioMixerConfig, AudioMixerNode};
pub mod resampler;
use resampler::{AudioResamplerConfig, AudioResamplerNode};

use schemars::schema_for;

/// Registers all available audio filter nodes with the engine's registry.
///
/// This function is only compiled if at least one audio feature is enabled.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_audio_filters(registry: &mut NodeRegistry) {
    // --- Register AudioGainNode ---
    #[cfg(feature = "audio_gain")]
    {
        let default_node = AudioGainNode::new(AudioGainConfig::default())
            .expect("Default AudioGainConfig should always be valid");
        registry.register_static_with_description(
            "audio::gain",
            |params: Option<&serde_json::Value>| {
                let config = config_helpers::parse_config_optional(params)?;
                let node = AudioGainNode::new(config).map_err(|e| {
                    StreamKitError::Configuration(format!("Invalid gain configuration: {e}"))
                })?;
                Ok(Box::new(node) as Box<dyn ProcessorNode>)
            },
            serde_json::to_value(schema_for!(AudioGainConfig))
                .expect("AudioGainConfig schema should serialize to JSON"),
            StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
            vec!["audio".to_string(), "filters".to_string()],
            false,
            "Adjusts audio volume by applying a linear gain multiplier to all samples. \
             Supports real-time parameter tuning for live volume control.",
        );
    }

    // --- Register AudioMixerNode ---
    #[cfg(feature = "audio_mixer")]
    {
        let (def_inputs, def_outputs) = AudioMixerNode::definition_pins();
        registry.register_static_with_description(
            "audio::mixer",
            |params: Option<&serde_json::Value>| {
                let config = match params {
                    Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
                        StreamKitError::Configuration(format!(
                            "Failed to parse audio::mixer params: {e}"
                        ))
                    })?,
                    None => AudioMixerConfig::default(), // Use default config
                };
                Ok(Box::new(AudioMixerNode::new(config)))
            },
            serde_json::to_value(schema_for!(AudioMixerConfig))
                .expect("AudioMixerConfig schema should serialize to JSON"),
            StaticPins { inputs: def_inputs, outputs: def_outputs },
            vec!["audio".to_string(), "filters".to_string()],
            false,
            "Combines multiple audio streams into a single output by summing samples. \
             Supports configurable number of input channels with per-channel gain control.",
        );
    }

    // --- Register AudioResamplerNode ---
    #[cfg(feature = "audio_resampler")]
    {
        let factory = AudioResamplerNode::factory();
        registry.register_dynamic_with_description(
            "audio::resampler",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(AudioResamplerConfig))
                .expect("AudioResamplerConfig schema should serialize to JSON"),
            vec!["audio".to_string(), "filters".to_string()],
            false,
            "Converts audio between different sample rates using high-quality resampling. \
             Essential for connecting nodes that operate at different sample rates.",
        );
    }
}
