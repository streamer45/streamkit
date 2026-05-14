// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! This module contains all built-in audio node implementations and their registration logic.

use streamkit_core::NodeRegistry;

pub mod codecs;
pub mod filters;
pub mod pacer;

/// Registers all available audio nodes with the engine's registry.
pub fn register_audio_nodes(registry: &mut NodeRegistry) {
    filters::register_audio_filters(registry);
    codecs::register_audio_codecs(registry);

    #[cfg(feature = "audio_pacer")]
    {
        register_dynamic_node!(
            registry,
            "audio::pacer",
            pacer::AudioPacerNode,
            pacer::AudioPacerConfig,
            ["audio", "timing"],
            "Controls audio playback timing by releasing frames at their natural rate. \
             Useful for real-time streaming where audio should play at the correct speed \
             rather than as fast as possible.",
        );
    }
}
