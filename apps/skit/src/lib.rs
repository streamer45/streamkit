// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

pub mod assets;
pub mod auth;
pub mod cli;
pub mod config;
pub mod file_security;
pub mod log_viewer;
pub mod logging;
pub mod marketplace;
pub mod marketplace_installer;
pub mod marketplace_security;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "moq")]
pub mod moq_gateway;
pub mod mse_gateway;
pub mod permissions;
pub mod plugin_assets;
pub mod plugin_paths;
pub mod plugin_records;
pub mod plugins;
pub mod profiling;
pub mod role_extractor;
pub mod sample_discovery;
pub mod samples;
pub mod server;
pub mod session;
pub mod state;
pub mod telemetry;
pub mod websocket;
pub mod websocket_handlers;

pub use config::Config;
pub use permissions::{Permissions, PermissionsConfig};
pub use role_extractor::get_permissions;
