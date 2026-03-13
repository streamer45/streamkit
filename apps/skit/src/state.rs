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
    pub async fn track(&self, handle: JoinHandle<()>) {
        self.handles.lock().await.push(handle);
    }

    /// Wait for all tracked shutdown tasks to complete, with a timeout.
    ///
    /// Returns the number of tasks that were still pending when called.
    pub async fn drain(&self, timeout: std::time::Duration) -> usize {
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self.handles.lock().await;
            std::mem::take(&mut *guard)
        };
        let count = handles.len();
        if count == 0 {
            return 0;
        }
        tracing::info!(count, "Draining background shutdown tasks");
        let _ = tokio::time::timeout(timeout, async {
            for handle in handles {
                let _ = handle.await;
            }
        })
        .await;
        count
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
