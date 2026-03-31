// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Gateway trait for MSE (Media Source Extensions) HTTP streaming
//!
//! This module defines the gateway interface that nodes can use to register
//! HTTP streaming endpoints for MSE-compatible media delivery.
//! The actual implementation lives in the server crate, but the interface is
//! defined here in core to avoid circular dependencies.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Represents a connected HTTP client wanting to receive the MSE stream.
///
/// When an HTTP GET request arrives at a registered MSE path, the server
/// creates an `MseClient` and sends it to the node via the registered channel.
/// The node then sends WebM chunks through `body_tx` to stream media to this client.
pub struct MseClient {
    /// Sender to push WebM chunks to this client's HTTP response body.
    pub body_tx: mpsc::Sender<bytes::Bytes>,
}

/// Gateway interface for MSE HTTP streaming that nodes use to register endpoints.
///
/// Follows the same pattern as [`MoqGatewayTrait`](crate::moq_gateway::MoqGatewayTrait):
/// trait defined in core, implementation in the server crate.
#[async_trait]
pub trait MseGatewayTrait: Send + Sync {
    /// Register an MSE streaming path for a session.
    ///
    /// `max_clients` is the maximum number of concurrent HTTP clients the gateway
    /// should accept for this stream. When the limit is reached, new clients are
    /// rejected with HTTP 503.
    ///
    /// Returns a receiver that delivers new HTTP clients as they connect.
    /// The node should send WebM chunks to each client's `body_tx`.
    async fn register_stream(
        &self,
        path: String,
        session_id: String,
        content_type: String,
        max_clients: u32,
    ) -> Result<mpsc::UnboundedReceiver<MseClient>, String>;

    /// Unregister an MSE streaming path, disconnecting all clients.
    async fn unregister_stream(&self, path: &str);
}

/// Global gateway registry — nodes call [`get_mse_gateway`] to obtain the gateway.
static GATEWAY: std::sync::OnceLock<Arc<dyn MseGatewayTrait>> = std::sync::OnceLock::new();

/// Initialize the global MSE gateway (called by the server at startup).
pub fn init_mse_gateway(gateway: Arc<dyn MseGatewayTrait>) {
    if GATEWAY.set(gateway).is_err() {
        tracing::warn!("MSE gateway already initialized");
    }
}

/// Get the global MSE gateway (called by nodes).
pub fn get_mse_gateway() -> Option<Arc<dyn MseGatewayTrait>> {
    GATEWAY.get().cloned()
}
