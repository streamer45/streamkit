// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Public client handle for controlling a running dynamic engine.

use crate::dynamic_messages::{NodeAddedNotification, QueryMessage, RuntimeSchemaUpdate};
use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::control::EngineControlMessage;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::view_data::NodeViewDataUpdate;
use tokio::sync::mpsc;

/// A handle to communicate with a running dynamic engine actor.
pub struct DynamicEngineHandle {
    control_tx: mpsc::Sender<EngineControlMessage>,
    query_tx: mpsc::Sender<QueryMessage>,
    engine_task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl DynamicEngineHandle {
    /// Creates a new handle to communicate with a running engine.
    pub(super) fn new(
        control_tx: mpsc::Sender<EngineControlMessage>,
        query_tx: mpsc::Sender<QueryMessage>,
        engine_task: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            control_tx,
            query_tx,
            engine_task: Arc::new(tokio::sync::Mutex::new(Some(engine_task))),
        }
    }

    /// Sends a control message to the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down.
    pub async fn send_control(&self, msg: EngineControlMessage) -> Result<(), String> {
        self.control_tx.send(msg).await.map_err(|_| "Engine actor has shut down".to_string())
    }

    /// Gets the current states of all nodes in the pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_node_states(&self) -> Result<Arc<HashMap<String, NodeState>>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetNodeStates { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Gets the current statistics of all nodes in the pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_node_stats(&self) -> Result<Arc<HashMap<String, NodeStats>>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetNodeStats { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to node state updates.
    /// Returns a receiver that will receive all subsequent state changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_state(&self) -> Result<mpsc::Receiver<NodeStateUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeState { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to node statistics updates.
    /// Returns a receiver that will receive all subsequent stats updates.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_stats(&self) -> Result<mpsc::Receiver<NodeStatsUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeStats { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to telemetry events.
    /// Returns a receiver that will receive all subsequent telemetry events.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_telemetry(&self) -> Result<mpsc::Receiver<TelemetryEvent>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeTelemetry { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to node view data updates.
    /// Returns a receiver that will receive all subsequent view data updates.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_view_data(&self) -> Result<mpsc::Receiver<NodeViewDataUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeViewData { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Gets the current view data for all nodes in the pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_node_view_data(
        &self,
    ) -> Result<Arc<HashMap<String, serde_json::Value>>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetNodeViewData { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Gets the runtime param schema overrides for all nodes in the pipeline.
    ///
    /// Only nodes whose `ProcessorNode::runtime_param_schema()` returned
    /// `Some` after initialization will have entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_runtime_schemas(&self) -> Result<HashMap<String, serde_json::Value>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetRuntimeSchemas { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to runtime param schema discovery notifications.
    /// Returns a receiver that will receive updates whenever a node's
    /// runtime schema is discovered after initialization.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_runtime_schemas(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<RuntimeSchemaUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeRuntimeSchemas { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Subscribes to node-creation success notifications.
    ///
    /// Yields exactly one `NodeAddedNotification` per node whose
    /// constructor *and* initialization succeeded.  Failures do not
    /// appear on this channel — observe them via `subscribe_state` and
    /// match `NodeState::Failed`.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_node_added(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<NodeAddedNotification>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeNodeAdded { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// Sends a shutdown signal and waits for engine completion.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine was already shut down, timed out
    /// (10s), or panicked.
    #[allow(clippy::cognitive_complexity)]
    pub async fn shutdown_and_wait(&self) -> Result<(), String> {
        self.send_control(EngineControlMessage::Shutdown).await?;

        let join_handle = {
            let mut task_guard = self.engine_task.lock().await;
            task_guard.take()
        };

        if let Some(handle) = join_handle {
            match tokio::time::timeout(std::time::Duration::from_secs(10), handle).await {
                Ok(Ok(())) => {
                    tracing::debug!("Engine shut down gracefully");
                    Ok(())
                },
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "Engine task panicked during shutdown");
                    Err(format!("Engine task panicked: {e}"))
                },
                Err(_) => {
                    tracing::warn!("Engine did not shut down within 10s timeout");
                    Err("Engine shutdown timeout".to_string())
                },
            }
        } else {
            tracing::warn!("shutdown_and_wait called multiple times, engine already shut down");
            Ok(())
        }
    }
}
