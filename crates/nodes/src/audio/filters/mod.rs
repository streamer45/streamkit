// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::{
    config_helpers, registry::StaticPins, NodeRegistry, ProcessorNode, StreamKitError,
};

pub mod gain;
use gain::{AudioGainConfig, AudioGainNode};
pub mod mixer;
use mixer::{AudioMixerConfig, AudioMixerNode};
pub mod resampler;
use resampler::{AudioResamplerConfig, AudioResamplerNode};

/// Registers all available audio filter nodes with the engine's registry.
///
/// # Panics
///
/// Panics if default node configs are invalid (should never happen).
#[allow(clippy::expect_used)]
pub fn register_audio_filters(registry: &mut NodeRegistry) {
    #[cfg(feature = "audio_gain")]
    {
        let default_node = AudioGainNode::new(AudioGainConfig::default())
            .expect("Default AudioGainConfig should always be valid");
        register_static_node!(
            registry,
            "audio::gain",
            |params: Option<&serde_json::Value>| {
                let config = config_helpers::parse_config_optional(params)?;
                let node = AudioGainNode::new(config).map_err(|e| {
                    StreamKitError::Configuration(format!("Invalid gain configuration: {e}"))
                })?;
                Ok(Box::new(node) as Box<dyn ProcessorNode>)
            },
            AudioGainConfig,
            StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
            ["audio", "filters"],
            "Adjusts audio volume by applying a linear gain multiplier to all samples. \
             Supports real-time parameter tuning for live volume control.",
        );
    }

    #[cfg(feature = "audio_mixer")]
    {
        let (def_inputs, def_outputs) = AudioMixerNode::definition_pins();
        register_static_node!(
            registry,
            "audio::mixer",
            |params: Option<&serde_json::Value>| {
                let config = match params {
                    Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
                        StreamKitError::Configuration(format!(
                            "Failed to parse audio::mixer params: {e}"
                        ))
                    })?,
                    None => AudioMixerConfig::default(),
                };
                Ok(Box::new(AudioMixerNode::new(config)))
            },
            AudioMixerConfig,
            StaticPins { inputs: def_inputs, outputs: def_outputs },
            ["audio", "filters"],
            "Combines multiple audio streams into a single output by summing samples. \
             Supports configurable number of input channels with per-channel gain control.",
        );
    }

    #[cfg(feature = "audio_resampler")]
    {
        register_dynamic_node!(
            registry,
            "audio::resampler",
            AudioResamplerNode,
            AudioResamplerConfig,
            ["audio", "filters"],
            "Converts audio between different sample rates using high-quality resampling. \
             Essential for connecting nodes that operate at different sample rates.",
        );
    }
}
