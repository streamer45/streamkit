// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::NodeRegistry;

pub mod moq;

#[cfg(feature = "http")]
pub mod http;

#[cfg(feature = "http")]
pub mod http_mse;

#[cfg(feature = "rtmp")]
mod rtmp_client;

#[cfg(feature = "rtmp")]
pub mod rtmp;

pub fn register_transport_nodes(registry: &mut NodeRegistry) {
    moq::register_moq_nodes(registry);

    #[cfg(feature = "http")]
    http::register_http_nodes(registry);

    #[cfg(feature = "http")]
    http_mse::register_http_mse_nodes(registry);

    #[cfg(feature = "rtmp")]
    rtmp::register_rtmp_nodes(registry);
}
