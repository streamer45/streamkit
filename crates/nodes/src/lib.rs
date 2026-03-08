// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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

/// A single function to register all built-in nodes.
///
/// For the script node, pass the global fetch allowlist and secrets from server configuration.
/// For the compositor node, pass the global compositor config from server configuration.
#[cfg(feature = "script")]
#[allow(clippy::implicit_hasher)]
pub fn register_nodes(
    registry: &mut NodeRegistry,
    global_script_allowlist: Option<Vec<core::script::AllowlistRule>>,
    secrets: std::collections::HashMap<String, core::script::ScriptSecret>,
    #[cfg(feature = "compositor")] global_compositor_config: Option<
        video::compositor::config::GlobalCompositorConfig,
    >,
) {
    // Call the registration function for each feature module.
    core::register_core_nodes(registry, global_script_allowlist, secrets);
    audio::register_audio_nodes(registry);
    containers::register_container_nodes(registry);
    transport::register_transport_nodes(registry);
    video::register_video_nodes(
        registry,
        #[cfg(feature = "compositor")]
        global_compositor_config,
    );

    tracing::info!("Finished registering built-in nodes.");
}

/// A single function to register all built-in nodes (without script configuration).
#[cfg(not(feature = "script"))]
pub fn register_nodes(
    registry: &mut NodeRegistry,
    #[cfg(feature = "compositor")] global_compositor_config: Option<
        video::compositor::config::GlobalCompositorConfig,
    >,
) {
    // Call the registration function for each feature module.
    core::register_core_nodes(registry);
    audio::register_audio_nodes(registry);
    containers::register_container_nodes(registry);
    transport::register_transport_nodes(registry);
    video::register_video_nodes(
        registry,
        #[cfg(feature = "compositor")]
        global_compositor_config,
    );

    tracing::info!("Finished registering built-in nodes.");
}
