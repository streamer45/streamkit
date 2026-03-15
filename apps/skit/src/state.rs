// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use streamkit_api::Event as ApiEvent;
use streamkit_engine::Engine;

use crate::auth::AuthState;
use crate::config::Config;
use crate::marketplace_installer::InstallJobQueue;
use crate::plugins::SharedUnifiedPluginManager;
use crate::session::SessionManager;

#[cfg(feature = "moq")]
use crate::moq_gateway::MoqGateway;

/// Tracks background shutdown tasks so they can be drained during server exit.
///
/// When a session is destroyed, `shutdown_and_wait()` runs in a background
/// `tokio::spawn` to avoid blocking the HTTP/WS response.  This tracker
/// collects those `JoinHandle`s so the server can wait for them during
/// graceful shutdown.
#[derive(Clone, Default)]
pub struct ShutdownTracker {
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl ShutdownTracker {
    /// Register a background shutdown task.
    ///
    /// Prunes already-finished handles before pushing the new one so the
    /// internal list stays bounded by the number of *concurrently running*
    /// shutdown tasks rather than growing for the server's entire lifetime.
    pub async fn track(&self, handle: JoinHandle<()>) {
        let mut guard = self.handles.lock().await;
        guard.retain(|h| !h.is_finished());
        guard.push(handle);
    }

    /// Wait for all tracked shutdown tasks to complete, with a timeout.
    ///
    /// Loops until no new handles appear, so tasks tracked while a
    /// previous batch was being awaited are not orphaned.
    /// Returns the total number of tasks that were drained.
    pub async fn drain(&self, timeout: std::time::Duration) -> usize {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut total = 0;

        loop {
            let handles: Vec<JoinHandle<()>> = {
                let mut guard = self.handles.lock().await;
                std::mem::take(&mut *guard)
            };
            if handles.is_empty() {
                break;
            }
            let count = handles.len();
            total += count;
            tracing::info!(count, total, "Draining background shutdown tasks");
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(remaining, futures::future::join_all(handles)).await;
        }

        total
    }
}

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub config: Arc<Config>,
    pub event_tx: broadcast::Sender<ApiEvent>,
    pub plugin_manager: SharedUnifiedPluginManager,
    pub marketplace_jobs: InstallJobQueue,
    pub auth: Arc<AuthState>,
    pub shutdown_tracker: ShutdownTracker,
    #[cfg(feature = "moq")]
    pub moq_gateway: Option<Arc<MoqGateway>>,
}
