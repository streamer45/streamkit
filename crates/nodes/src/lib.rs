// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::NodeRegistry;

// Declare the top-level feature modules directly.
pub mod audio;
pub mod core;
// pub mod video;
pub mod containers;
pub mod transport;

// Shared utilities
pub mod streaming_utils;

#[cfg(test)]
pub mod test_utils;

/// A single function to register all built-in nodes.
///
/// For the script node, pass the global fetch allowlist and secrets from server configuration.
#[cfg(feature = "script")]
#[allow(clippy::implicit_hasher)]
pub fn register_nodes(
    registry: &mut NodeRegistry,
    global_script_allowlist: Option<Vec<core::script::AllowlistRule>>,
    secrets: std::collections::HashMap<String, core::script::ScriptSecret>,
) {
    // Call the registration function for each feature module.
    core::register_core_nodes(registry, global_script_allowlist, secrets);
    audio::register_audio_nodes(registry);
    containers::register_container_nodes(registry);
    transport::register_transport_nodes(registry);
    // video::register_video_nodes(registry);

    tracing::info!("Finished registering built-in nodes.");
}

/// A single function to register all built-in nodes (without script configuration).
#[cfg(not(feature = "script"))]
pub fn register_nodes(registry: &mut NodeRegistry) {
    // Call the registration function for each feature module.
    core::register_core_nodes(registry);
    audio::register_audio_nodes(registry);
    containers::register_container_nodes(registry);
    transport::register_transport_nodes(registry);
    // video::register_video_nodes(registry);

    tracing::info!("Finished registering built-in nodes.");
}
