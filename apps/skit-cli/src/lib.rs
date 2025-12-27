// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow println/eprintln in CLI client - these are for direct user output, not logging
#![allow(clippy::disallowed_macros)]

pub mod client;
pub mod load_test;
pub mod shell;

// Re-export for convenience
pub use client::{
    control_add_node, control_apply_batch, control_connect, control_disconnect,
    control_get_pipeline, control_list_nodes, control_remove_node, control_tune_async,
    control_validate_batch, create_session, delete_audio_asset, delete_plugin, delete_sample,
    destroy_session, get_config, get_permissions, get_pipeline, get_sample, list_audio_assets,
    list_node_schemas, list_packet_schemas, list_plugins, list_samples_dynamic,
    list_samples_oneshot, list_sessions, process_oneshot, save_sample, tune_node,
    upload_audio_asset, upload_plugin, watch_events,
};
pub use load_test::run_load_test;

/// Start an interactive shell session
///
/// # Errors
///
/// Returns an error if:
/// - The server URL is invalid
/// - Failed to establish WebSocket connection
/// - Terminal readline initialization fails
pub async fn start_shell(server_url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut shell = shell::Shell::new(server_url)?;
    shell.run().await
}
