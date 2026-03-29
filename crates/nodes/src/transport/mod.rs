// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! This module contains all built-in transport node implementations.

use streamkit_core::NodeRegistry;

pub mod moq;

#[cfg(feature = "http")]
pub mod http;

#[cfg(feature = "http")]
pub mod http_mse;

/// Registers all available transport nodes with the engine's registry.
pub fn register_transport_nodes(registry: &mut NodeRegistry) {
    // Call the registration function from each submodule.
    moq::register_moq_nodes(registry);

    #[cfg(feature = "http")]
    http::register_http_nodes(registry);

    #[cfg(feature = "http")]
    http_mse::register_http_mse_nodes(registry);
}
