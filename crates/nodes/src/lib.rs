// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::constraints::GlobalNodeConstraints;
use streamkit_core::NodeRegistry;

// Declare the top-level feature modules directly.
pub mod audio;
pub mod containers;
pub mod core;
pub mod transport;
pub mod video;

// Shared utilities
pub mod codec_utils;
pub mod streaming_utils;

#[cfg(test)]
pub mod test_utils;

/// Register all built-in nodes.
///
/// Server-level constraints (script allowlist, compositor limits, etc.) are
/// passed via the generic [`GlobalNodeConstraints`] container.  Each node
/// module extracts only the constraint types it needs.
pub fn register_nodes(registry: &mut NodeRegistry, constraints: &GlobalNodeConstraints) {
    core::register_core_nodes(registry, constraints);
    audio::register_audio_nodes(registry);
    containers::register_container_nodes(registry);
    transport::register_transport_nodes(registry);
    video::register_video_nodes(registry, constraints);

    tracing::info!("Finished registering built-in nodes.");
}
