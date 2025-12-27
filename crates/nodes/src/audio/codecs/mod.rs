// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! This module contains all built-in audio codec node implementations.

use streamkit_core::NodeRegistry;

// Declare the submodules for each codec.
pub mod flac;
pub mod mp3;
pub mod opus;

/// Registers all available audio codec nodes with the engine's registry.
pub fn register_audio_codecs(registry: &mut NodeRegistry) {
    // Call the registration function from each submodule.
    opus::register_opus_nodes(registry);
    mp3::register_mp3_nodes(registry);
    flac::register_flac_nodes(registry);
}
