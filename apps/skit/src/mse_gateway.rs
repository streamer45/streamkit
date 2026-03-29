// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MSE Gateway: Routes HTTP GET requests to HttpMse nodes for MSE-compatible streaming.
//!
//! The gateway accepts HTTP connections and routes them to the appropriate session's
//! HttpMse node based on URL path matching. Follows the same pattern as `MoqGateway`.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use streamkit_core::mse_gateway::{MseClient, MseGatewayTrait};
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

/// Guard that decrements the active client counter when dropped.
/// Returned alongside the `body_rx` so the counter is decremented
/// when the HTTP response stream ends (handler drops the guard).
pub struct MseClientGuard {
    counter: Arc<AtomicU32>,
}

impl Drop for MseClientGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A stream registration from a path to a client receiver.
struct StreamRoute {
    /// The session ID that owns this route.
    #[allow(dead_code)]
    session_id: String,

    /// The content type for HTTP responses (e.g. `video/webm; codecs="vp9,opus"`).
    content_type: String,

    /// Channel to send new HTTP clients to the node.
    client_tx: mpsc::UnboundedSender<MseClient>,

    /// Maximum number of concurrent clients allowed for this stream.
    max_clients: u32,

    /// Current number of connected clients.
    /// Tracked at the gateway level so `connect_client` can reject with 503
    /// before creating a channel (avoiding empty-200 responses).
    active_clients: Arc<AtomicU32>,
}

/// Routes HTTP GET requests to HttpMse nodes based on path patterns.
pub struct MseGateway {
    /// Map of path patterns to stream routes.
    routes: Arc<RwLock<HashMap<String, StreamRoute>>>,
}

impl MseGateway {
    /// Create a new MSE gateway.
    pub fn new() -> Self {
        Self { routes: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Handle an incoming HTTP GET request by connecting the client to the
    /// appropriate HttpMse node.
    ///
    /// Returns `Ok((content_type, body_rx))` on success, where `body_rx` receives
    /// WebM chunks to stream to the HTTP client.
    ///
    /// # Errors
    ///
    /// Returns `Err(MseConnectError::NotFound)` if no route is registered for the path.
    /// Returns `Err(MseConnectError::AtCapacity)` if `max_clients` has been reached.
    /// Returns `Err(MseConnectError::NodeStopped)` if the node's client channel is closed.
    pub async fn connect_client(
        &self,
        path: &str,
    ) -> Result<(String, mpsc::Receiver<bytes::Bytes>, MseClientGuard), MseConnectError> {
        let (content_type, client_tx, active_clients, max_clients) = self
            .routes
            .read()
            .await
            .get(path)
            .map(|route| {
                (
                    route.content_type.clone(),
                    route.client_tx.clone(),
                    Arc::clone(&route.active_clients),
                    route.max_clients,
                )
            })
            .ok_or(MseConnectError::NotFound)?;

        // Atomically check and increment the client count.
        // If we overshoot due to a race, decrement and reject.
        let prev = active_clients.fetch_add(1, Ordering::AcqRel);
        if prev >= max_clients {
            active_clients.fetch_sub(1, Ordering::AcqRel);
            return Err(MseConnectError::AtCapacity);
        }

        // Create a channel for streaming chunks to this client's HTTP response body.
        // Capacity of 64 provides ~2s of buffer at 30fps before backpressure.
        let (body_tx, body_rx) = mpsc::channel(64);

        let client = MseClient { body_tx };

        if client_tx.send(client).is_err() {
            active_clients.fetch_sub(1, Ordering::AcqRel);
            return Err(MseConnectError::NodeStopped);
        }

        // Return a guard that decrements the active client counter on drop,
        // ensuring the count stays accurate when the HTTP response ends.
        let guard = MseClientGuard { counter: active_clients };

        Ok((content_type, body_rx, guard))
    }

    /// Get the number of registered routes (for testing).
    #[cfg(test)]
    pub async fn route_count(&self) -> usize {
        self.routes.read().await.len()
    }
}

impl Default for MseGateway {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur when connecting an HTTP client to an MSE stream.
#[derive(Debug)]
pub enum MseConnectError {
    /// No MSE stream is registered for the requested path.
    NotFound,
    /// The stream has reached its maximum number of concurrent clients.
    AtCapacity,
    /// The MSE stream node is no longer running.
    NodeStopped,
}

impl std::error::Error for MseConnectError {}

impl std::fmt::Display for MseConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "No MSE stream registered for this path"),
            Self::AtCapacity => write!(f, "MSE stream at maximum client capacity"),
            Self::NodeStopped => write!(f, "MSE stream node is no longer running"),
        }
    }
}

#[async_trait]
impl MseGatewayTrait for MseGateway {
    async fn register_stream(
        &self,
        path: String,
        session_id: String,
        content_type: String,
        max_clients: u32,
    ) -> Result<mpsc::UnboundedReceiver<MseClient>, String> {
        let (client_tx, client_rx) = mpsc::unbounded_channel();

        let route = StreamRoute {
            session_id: session_id.clone(),
            content_type,
            client_tx,
            max_clients,
            active_clients: Arc::new(AtomicU32::new(0)),
        };

        {
            let mut routes = self.routes.write().await;

            if routes.contains_key(&path) {
                return Err(format!("MSE stream path '{path}' is already registered"));
            }

            routes.insert(path.clone(), route);
        }

        info!(
            path = %path,
            session_id = %session_id,
            max_clients,
            "Registered MSE stream route"
        );

        Ok(client_rx)
    }

    async fn unregister_stream(&self, path: &str) {
        let mut routes = self.routes.write().await;
        if routes.remove(path).is_some() {
            info!(path = %path, "Unregistered MSE stream route");
        } else {
            warn!(path = %path, "Attempted to unregister unknown MSE stream route");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_stream_registration() {
        let gateway = MseGateway::new();

        let _rx = gateway
            .register_stream(
                "/mse/test/video".to_string(),
                "session-1".to_string(),
                "video/webm".to_string(),
                10,
            )
            .await
            .expect("Failed to register stream");

        assert_eq!(gateway.route_count().await, 1);

        // Try to register the same path again
        let result = gateway
            .register_stream(
                "/mse/test/video".to_string(),
                "session-2".to_string(),
                "video/webm".to_string(),
                10,
            )
            .await;

        assert!(result.is_err());
        assert_eq!(gateway.route_count().await, 1);
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_stream_unregistration() {
        let gateway = MseGateway::new();

        let _rx = gateway
            .register_stream(
                "/mse/test/video".to_string(),
                "session-1".to_string(),
                "video/webm".to_string(),
                10,
            )
            .await
            .expect("Failed to register stream");

        assert_eq!(gateway.route_count().await, 1);

        gateway.unregister_stream("/mse/test/video").await;

        assert_eq!(gateway.route_count().await, 0);
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_connect_client() {
        let gateway = MseGateway::new();

        let mut client_rx = gateway
            .register_stream(
                "/mse/test/video".to_string(),
                "session-1".to_string(),
                "video/webm; codecs=\"vp9,opus\"".to_string(),
                10,
            )
            .await
            .expect("Failed to register stream");

        // Connect a client
        let (content_type, _body_rx, _guard) =
            gateway.connect_client("/mse/test/video").await.expect("Failed to connect client");

        assert_eq!(content_type, "video/webm; codecs=\"vp9,opus\"");

        // Verify the node receives the client
        let client = client_rx.recv().await;
        assert!(client.is_some());
    }

    #[tokio::test]
    async fn test_connect_client_no_route() {
        let gateway = MseGateway::new();

        let result = gateway.connect_client("/mse/nonexistent").await;
        assert!(matches!(result, Err(MseConnectError::NotFound)));
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_connect_client_at_capacity() {
        let gateway = MseGateway::new();

        let mut _client_rx = gateway
            .register_stream(
                "/mse/test/video".to_string(),
                "session-1".to_string(),
                "video/webm".to_string(),
                2, // Only allow 2 clients
            )
            .await
            .expect("Failed to register stream");

        // Connect 2 clients (at capacity)
        let _c1 = gateway.connect_client("/mse/test/video").await.expect("Client 1 should connect");
        let _c2 = gateway.connect_client("/mse/test/video").await.expect("Client 2 should connect");

        // Third client should be rejected
        let result = gateway.connect_client("/mse/test/video").await;
        assert!(matches!(result, Err(MseConnectError::AtCapacity)));
    }
}
