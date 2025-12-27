// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! This module contains all built-in audio node implementations and their registration logic.

use streamkit_core::NodeRegistry;

pub mod codecs;
pub mod filters;
pub mod pacer;

use schemars::schema_for;

/// Registers all available audio nodes with the engine's registry.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_audio_nodes(registry: &mut NodeRegistry) {
    // Call the registration functions from the submodules.
    filters::register_audio_filters(registry);
    codecs::register_audio_codecs(registry);

    // Register audio pacer
    #[cfg(feature = "audio_pacer")]
    {
        use pacer::{AudioPacerConfig, AudioPacerNode};
        let factory = AudioPacerNode::factory();
        registry.register_dynamic_with_description(
            "audio::pacer",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(AudioPacerConfig))
                .expect("AudioPacerConfig schema should serialize to JSON"),
            vec!["audio".to_string(), "timing".to_string()],
            false,
            "Controls audio playback timing by releasing frames at their natural rate. \
             Useful for real-time streaming where audio should play at the correct speed \
             rather than as fast as possible.",
        );
    }
}
