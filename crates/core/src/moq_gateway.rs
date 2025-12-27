// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Gateway trait for MoQ WebTransport routing
//!
//! This module defines the gateway interface that nodes can use to register routes.
//! The actual implementation lives in the server crate, but the interface is defined
//! here in core to avoid circular dependencies.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Opaque type for WebTransport session - actual type defined in moq-native
pub type WebTransportSession = Box<dyn std::any::Any + Send>;

/// Result of attempting to handle a MoQ connection
#[derive(Debug)]
pub enum MoqConnectionResult {
    /// Connection was successfully handled by a node
    Accepted,
    /// Connection was rejected by the node
    Rejected(String),
}

/// A WebTransport connection that needs to be routed to a moq_peer node
pub struct MoqConnection {
    /// The path requested (e.g., "/moq/anon/input")
    pub path: String,

    /// The WebTransport session handle (type-erased)
    pub session: WebTransportSession,

    /// Channel to send response back to gateway
    pub response_tx: tokio::sync::oneshot::Sender<MoqConnectionResult>,
}

/// Gateway interface that nodes can use to register routes
#[async_trait]
pub trait MoqGatewayTrait: Send + Sync {
    /// Register a path pattern for a session's moq_peer node
    ///
    /// Returns a receiver that will receive accepted connections matching this path.
    async fn register_route(
        &self,
        path_pattern: String,
        session_id: String,
    ) -> Result<mpsc::UnboundedReceiver<MoqConnection>, String>;

    /// Unregister a path pattern
    async fn unregister_route(&self, path_pattern: &str);
}

/// Global gateway registry - nodes call this to get the gateway
static GATEWAY: std::sync::OnceLock<Arc<dyn MoqGatewayTrait>> = std::sync::OnceLock::new();

/// Initialize the global MoQ gateway (called by server)
pub fn init_moq_gateway(gateway: Arc<dyn MoqGatewayTrait>) {
    if GATEWAY.set(gateway).is_err() {
        tracing::warn!("MoQ gateway already initialized");
    }
}

/// Get the global MoQ gateway (called by nodes)
pub fn get_moq_gateway() -> Option<Arc<dyn MoqGatewayTrait>> {
    GATEWAY.get().cloned()
}
