// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

pub mod assets;
pub mod cli;
pub mod config;
pub mod file_security;
pub mod logging;
#[cfg(feature = "moq")]
pub mod moq_gateway;
pub mod permissions;
pub mod plugins;
pub mod profiling;
pub mod role_extractor;
pub mod samples;
pub mod server;
pub mod session;
pub mod state;
pub mod telemetry;
pub mod websocket;
pub mod websocket_handlers;

// Re-export commonly used items for convenience
pub use config::Config;
pub use permissions::{Permissions, PermissionsConfig};
pub use role_extractor::get_permissions;
