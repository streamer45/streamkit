// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Gateway: Routes WebTransport connections to moq_peer nodes based on path patterns.
//!
//! The gateway accepts WebTransport connections on the server's main port and routes them
//! to the appropriate session's moq_peer node based on URL path matching.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use streamkit_core::moq_gateway::{MoqConnection, MoqConnectionResult, MoqGatewayTrait};
use tokio::sync::{mpsc, oneshot, Notify, RwLock};
use tracing::{debug, error, info, warn};

/// A route registration from a path pattern to a connection receiver
struct Route {
    /// The session ID that owns this route
    #[allow(dead_code)]
    session_id: String,

    /// Channel to send accepted connections to the node
    connection_tx: mpsc::UnboundedSender<MoqConnection>,
}

/// Routes WebTransport connections to moq_peer nodes based on path patterns
pub struct MoqGateway {
    /// Map of path patterns to routes
    /// For now, we use exact path matching. Could be extended to support wildcards.
    routes: Arc<RwLock<HashMap<String, Route>>>,

    /// Notifies waiters when routes are registered/unregistered.
    ///
    /// This supports a "pre-connect" flow where the browser establishes WebTransport
    /// before a dynamic session spins up and registers its MoQ route.
    route_notify: Arc<Notify>,

    /// Certificate fingerprints for local development (served via HTTP)
    #[cfg(feature = "moq")]
    fingerprints: Arc<RwLock<Vec<String>>>,
}

impl MoqGateway {
    /// Create a new MoQ gateway
    pub fn new() -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            route_notify: Arc::new(Notify::new()),
            #[cfg(feature = "moq")]
            fingerprints: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Set certificate fingerprints (called when WebTransport server starts)
    #[cfg(feature = "moq")]
    pub async fn set_fingerprints(&self, fingerprints: Vec<String>) {
        let mut fps = self.fingerprints.write().await;
        *fps = fingerprints;
    }

    /// Get certificate fingerprints for serving via HTTP
    #[cfg(feature = "moq")]
    pub async fn get_fingerprints(&self) -> Vec<String> {
        self.fingerprints.read().await.clone()
    }

    /// Handle an incoming WebTransport connection by routing it to the appropriate node
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No route is registered for the given path
    /// - The connection channel to the node is closed
    /// - The node rejects the connection
    /// - The node doesn't respond to the connection request
    #[cfg(feature = "moq")]
    #[allow(clippy::cognitive_complexity)]
    pub async fn accept_connection(
        &self,
        session: moq_native::web_transport_quinn::Session,
        path: String,
    ) -> Result<(), String> {
        debug!(path = %path, "Received WebTransport connection");

        // Best-effort wait for route registration to support pre-connect.
        //
        // The UI currently connects WebTransport before creating the dynamic session; when the
        // session starts, it registers its routes. Without waiting here, the server would drop
        // the connection before the route exists.
        let connection_tx: Option<mpsc::UnboundedSender<MoqConnection>> = {
            const MAX_WAIT: Duration = Duration::from_secs(30);
            const POLL_INTERVAL: Duration = Duration::from_millis(200);

            let mut waited = Duration::from_secs(0);
            loop {
                if let Some(tx) = {
                    let routes = self.routes.read().await;
                    routes.get(&path).map(|r| r.connection_tx.clone())
                } {
                    break Some(tx);
                }

                if waited >= MAX_WAIT {
                    break None;
                }

                tokio::select! {
                    () = self.route_notify.notified() => {}
                    () = tokio::time::sleep(POLL_INTERVAL) => {}
                }
                waited = waited.saturating_add(POLL_INTERVAL);
            }
        };

        if let Some(connection_tx) = connection_tx {
            let (response_tx, response_rx) = oneshot::channel();

            // Type-erase the WebTransport session
            let session_boxed: streamkit_core::moq_gateway::WebTransportSession = Box::new(session);

            let conn = MoqConnection { path: path.clone(), session: session_boxed, response_tx };

            // Send connection to the node
            if connection_tx.send(conn).is_err() {
                error!(path = %path, "Failed to send connection to node (channel closed)");
                return Err("Node disconnected".to_string());
            }

            // Wait for node to accept or reject
            match response_rx.await {
                Ok(MoqConnectionResult::Accepted) => {
                    info!(path = %path, "Connection accepted by node");
                    Ok(())
                },
                Ok(MoqConnectionResult::Rejected(reason)) => {
                    warn!(path = %path, reason = %reason, "Connection rejected by node");
                    Err(reason)
                },
                Err(_) => {
                    error!(path = %path, "Node dropped connection without responding");
                    Err("Node did not respond".to_string())
                },
            }
        } else {
            warn!(path = %path, "No route registered for path");
            Err(format!("No route found for path '{path}'"))
        }
    }

    /// Get the number of registered routes
    #[cfg(test)]
    pub async fn route_count(&self) -> usize {
        self.routes.read().await.len()
    }

    /// List all registered path patterns
    #[cfg(test)]
    pub async fn list_routes(&self) -> Vec<(String, String)> {
        let routes = self.routes.read().await;
        routes.iter().map(|(path, route)| (path.clone(), route.session_id.clone())).collect()
    }
}

impl Default for MoqGateway {
    fn default() -> Self {
        Self::new()
    }
}

// Implement the trait from core so nodes can use it
#[async_trait]
impl MoqGatewayTrait for MoqGateway {
    async fn register_route(
        &self,
        path_pattern: String,
        session_id: String,
    ) -> Result<mpsc::UnboundedReceiver<MoqConnection>, String> {
        let (connection_tx, connection_rx) = mpsc::unbounded_channel();

        let route = Route { session_id: session_id.clone(), connection_tx };

        {
            let mut routes = self.routes.write().await;

            // Check if this path is already registered
            if routes.contains_key(&path_pattern) {
                return Err(format!("Path pattern '{path_pattern}' is already registered"));
            }

            routes.insert(path_pattern.clone(), route);
        }

        self.route_notify.notify_waiters();

        info!(
            path_pattern = %path_pattern,
            session_id = %session_id,
            "Registered MoQ route"
        );

        Ok(connection_rx)
    }

    async fn unregister_route(&self, path_pattern: &str) {
        let mut routes = self.routes.write().await;
        if routes.remove(path_pattern).is_some() {
            self.route_notify.notify_waiters();
            info!(path_pattern = %path_pattern, "Unregistered MoQ route");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_route_registration() {
        let gateway = MoqGateway::new();

        // Register a route
        let _rx = gateway
            .register_route("/moq/anon".to_string(), "session-1".to_string())
            .await
            .expect("Failed to register route");

        assert_eq!(gateway.route_count().await, 1);

        // Try to register the same path again
        let result = gateway.register_route("/moq/anon".to_string(), "session-2".to_string()).await;

        assert!(result.is_err());
        assert_eq!(gateway.route_count().await, 1);
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn test_route_unregistration() {
        let gateway = MoqGateway::new();

        let _rx = gateway
            .register_route("/moq/test".to_string(), "session-1".to_string())
            .await
            .expect("Failed to register route");

        assert_eq!(gateway.route_count().await, 1);

        gateway.unregister_route("/moq/test").await;

        assert_eq!(gateway.route_count().await, 0);
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_list_routes() {
        let gateway = MoqGateway::new();

        let _rx1 = gateway
            .register_route("/moq/path1".to_string(), "session-1".to_string())
            .await
            .unwrap();

        let _rx2 = gateway
            .register_route("/moq/path2".to_string(), "session-2".to_string())
            .await
            .unwrap();

        let routes = gateway.list_routes().await;
        assert_eq!(routes.len(), 2);

        // Check that both routes are present (order doesn't matter)
        assert!(routes.iter().any(|(p, s)| p == "/moq/path1" && s == "session-1"));
        assert!(routes.iter().any(|(p, s)| p == "/moq/path2" && s == "session-2"));
    }
}
