// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::extract::ws::Utf8Bytes;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use streamkit_api::Event as ApiEvent;
use streamkit_engine::Engine;

/// Wrapper around [`ApiEvent`] for the broadcast channel.
///
/// Contains both the structured event (for filtering) and a pre-serialized
/// JSON [`Utf8Bytes`] so that N WebSocket handlers can clone the refcounted
/// buffer (no memcpy) instead of each calling `serde_json::to_string`.
///
/// The UTF-8 invariant is established once at construction via
/// `serde_json::to_string` (which always produces valid UTF-8).
#[derive(Clone, Debug)]
pub struct BroadcastEvent {
    pub event: ApiEvent,
    /// Pre-serialized JSON. Cloning is O(1) (refcount increment).
    pub json: Utf8Bytes,
}

impl BroadcastEvent {
    /// Wrap an event for broadcast to all connections.
    ///
    /// Serializes the event to JSON eagerly so each WebSocket handler can
    /// forward the pre-built bytes without re-serializing.
    pub fn to_all(event: ApiEvent) -> Self {
        let json: Utf8Bytes = match serde_json::to_string(&event) {
            Ok(s) => Utf8Bytes::from(s),
            Err(e) => {
                tracing::error!(error = %e, "Failed to pre-serialize broadcast event");
                Utf8Bytes::default()
            },
        };
        Self { event, json }
    }
}

use crate::auth::AuthState;
use crate::config::Config;
use crate::marketplace_installer::InstallJobQueue;
use crate::plugin_assets::PluginAssetRegistry;
use crate::plugins::SharedUnifiedPluginManager;
use crate::session::SessionManager;

#[cfg(feature = "moq")]
use crate::moq_gateway::MoqGateway;
use crate::mse_gateway::MseGateway;

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
    pub event_tx: broadcast::Sender<BroadcastEvent>,
    pub plugin_manager: SharedUnifiedPluginManager,
    pub marketplace_jobs: InstallJobQueue,
    pub auth: Arc<AuthState>,
    pub shutdown_tracker: ShutdownTracker,
    pub plugin_asset_registry: PluginAssetRegistry,
    #[cfg(feature = "moq")]
    pub moq_gateway: Option<Arc<MoqGateway>>,
    pub mse_gateway: Arc<MseGateway>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn make_test_event() -> ApiEvent {
        use streamkit_api::{EventPayload, Message, MessageType};
        Message {
            message_type: MessageType::Event,
            correlation_id: None,
            payload: EventPayload::SessionCreated {
                session_id: "s1".into(),
                name: None,
                created_at: String::new(),
            },
        }
    }

    #[test]
    fn broadcast_event_to_all_wraps_event() {
        let be = BroadcastEvent::to_all(make_test_event());
        // Ensure the event is accessible after wrapping.
        assert_eq!(be.event.message_type, streamkit_api::MessageType::Event);
    }

    #[tokio::test]
    async fn drain_awaits_all_tracked_tasks() {
        let tracker = ShutdownTracker::default();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = counter.clone();
            let handle = tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
            });
            tracker.track(handle).await;
        }

        let drained = tracker.drain(Duration::from_secs(5)).await;
        assert_eq!(drained, 3);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn drain_returns_zero_when_empty() {
        let tracker = ShutdownTracker::default();
        let drained = tracker.drain(Duration::from_secs(1)).await;
        assert_eq!(drained, 0);
    }

    /// Regression test: tasks tracked while a previous batch is being awaited
    /// must not be orphaned.  The old implementation took all handles and
    /// released the lock before awaiting — any handle pushed during that await
    /// window was never drained.
    #[tokio::test]
    async fn drain_does_not_orphan_tasks_added_during_await() {
        let tracker = ShutdownTracker::default();
        let counter = Arc::new(AtomicUsize::new(0));

        // First task: slow, and while it runs it adds a second task to the tracker.
        let tracker_clone = tracker.clone();
        let counter_clone = counter.clone();
        let inner_counter = counter.clone();
        let handle = tokio::spawn(async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            // Simulate a task that registers follow-up work during shutdown.
            let c = inner_counter;
            let follow_up = tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
            });
            tracker_clone.track(follow_up).await;
        });
        tracker.track(handle).await;

        let drained = tracker.drain(Duration::from_secs(5)).await;
        // Both the original task and the follow-up must have been drained.
        assert!(drained >= 2, "expected at least 2 drained tasks, got {drained}");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
