// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Public client handle for controlling a running dynamic engine.

use crate::dynamic_messages::{NodeLifecycleNotification, QueryMessage, RuntimeSchemaUpdate};
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

    /// # Errors
    /// Returns an error if the engine actor has shut down.
    pub async fn send_control(&self, msg: EngineControlMessage) -> Result<(), String> {
        self.control_tx.send(msg).await.map_err(|_| "Engine actor has shut down".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_node_states(&self) -> Result<Arc<HashMap<String, NodeState>>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetNodeStates { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_node_stats(&self) -> Result<Arc<HashMap<String, NodeStats>>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetNodeStats { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_state(&self) -> Result<mpsc::Receiver<NodeStateUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeState { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_stats(&self) -> Result<mpsc::Receiver<NodeStatsUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeStats { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_telemetry(&self) -> Result<mpsc::Receiver<TelemetryEvent>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeTelemetry { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_view_data(&self) -> Result<mpsc::Receiver<NodeViewDataUpdate>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeViewData { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
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

    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn get_runtime_schemas(&self) -> Result<HashMap<String, serde_json::Value>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::GetRuntimeSchemas { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
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

    /// Ordered stream of node add/remove events. `Added` fires once a node is
    /// successfully created; `Removed` fires when the actor tears it down.
    /// Failures appear on `subscribe_state` as `NodeState::Failed`. A single
    /// stream guarantees a removal is observed after its add — see #607.
    ///
    /// # Errors
    /// Returns an error if the engine actor has shut down or fails to respond.
    pub async fn subscribe_node_lifecycle(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<NodeLifecycleNotification>, String> {
        let (response_tx, mut response_rx) = mpsc::channel(1);
        self.query_tx
            .send(QueryMessage::SubscribeNodeLifecycle { response_tx })
            .await
            .map_err(|_| "Engine actor has shut down".to_string())?;

        response_rx.recv().await.ok_or_else(|| "Failed to receive response from engine".to_string())
    }

    /// # Errors
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
