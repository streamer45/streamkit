// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! This module contains all built-in container format (muxer/demuxer) nodes.

use streamkit_core::NodeRegistry;

// Declare the submodules for each container format.
pub mod ogg;
pub mod wav;
pub mod webm;

// Integration tests for container nodes
#[cfg(test)]
mod tests;

/// Registers all available container nodes with the engine's registry.
pub fn register_container_nodes(registry: &mut NodeRegistry) {
    // Call the registration function from each submodule.
    ogg::register_ogg_nodes(registry);
    wav::register_wav_nodes(registry);
    webm::register_webm_nodes(registry);
}
