// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow println/eprintln in CLI client - these are for direct user output, not logging
#![allow(clippy::disallowed_macros)]

pub mod client;
pub mod exit_codes;
pub mod graph;
pub mod load_test;
pub mod output;
pub mod shell;

// Re-export trait, concrete implementation, and standalone helpers
pub use client::{Client, InputFile, NetworkClient};
pub use load_test::run_load_test;
pub use output::{CliOutput, OutputFormat};

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
