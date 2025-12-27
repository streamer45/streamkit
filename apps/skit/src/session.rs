// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! server/src/session.rs: Manages live, dynamic pipeline sessions.

use crate::config::Config;
use opentelemetry::global;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use streamkit_api::{Event as ApiEvent, EventPayload, MessageType, Pipeline};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::state::NodeState;
use streamkit_core::stats::NodeStats;
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_engine::{DynamicEngineConfig, DynamicEngineHandle, Engine};
use time::format_description::well_known::Rfc3339;
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

/// Convert SystemTime to ISO 8601 / RFC3339 format string using the time crate
pub fn system_time_to_rfc3339(time: SystemTime) -> String {
    let offset_datetime = time::OffsetDateTime::from(time);
    offset_datetime.format(&Rfc3339).unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn timestamp_us_to_rfc3339(timestamp_us: u64) -> String {
    system_time_to_rfc3339(UNIX_EPOCH + Duration::from_micros(timestamp_us))
}

/// Creates an API event from a telemetry event with server-side redaction.
///
/// This function applies text truncation to protect sensitive content
/// while preserving enough information for debugging and monitoring.
fn create_telemetry_api_event(
    session_id: &str,
    event: &TelemetryEvent,
    max_text_chars: usize,
) -> ApiEvent {
    // Apply redaction to the data payload
    let mut data = event.packet.data.clone();
    redact_telemetry_data(&mut data, max_text_chars);

    let timestamp_us = event.packet.metadata.as_ref().and_then(|m| m.timestamp_us);
    let timestamp = timestamp_us
        .map_or_else(|| system_time_to_rfc3339(SystemTime::now()), timestamp_us_to_rfc3339);

    ApiEvent {
        message_type: MessageType::Event,
        correlation_id: None,
        payload: EventPayload::NodeTelemetry {
            session_id: session_id.to_string(),
            node_id: event.node_id.clone(),
            type_id: event.packet.type_id.clone(),
            data,
            timestamp_us,
            timestamp,
        },
    }
}

/// Recursively truncates string values in JSON data to enforce text limits.
///
/// This is applied server-side to ensure nodes cannot leak sensitive content
/// (e.g., full transcriptions, LLM responses) through telemetry.
fn redact_telemetry_data(value: &mut serde_json::Value, max_chars: usize) {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max_chars {
                // Truncate and add indicator
                let truncated: String = s.chars().take(max_chars).collect();
                *s = format!("{truncated}...[truncated]");
            }
        },
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                redact_telemetry_data(v, max_chars);
            }
        },
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_telemetry_data(v, max_chars);
            }
        },
        _ => {}, // Numbers, bools, null don't need redaction
    }
}

/// Represents a single, stateful, dynamic pipeline session.
#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    /// The handle to send control messages to the running DynamicEngine actor.
    engine_handle: Arc<DynamicEngineHandle>,
    pub pipeline: Arc<Mutex<Pipeline>>,
    /// Timestamp when the session was created
    pub created_at: SystemTime,
    /// User/role who created this session (for permission filtering)
    pub created_by: Option<String>,
}

impl Session {
    /// Forwards a control message to this session's specific engine actor.
    pub async fn send_control_message(&self, msg: EngineControlMessage) {
        if let Err(e) = self.engine_handle.send_control(msg).await {
            tracing::error!(session_id = %self.id, error = %e, "Failed to send control message");
        }
    }

    /// Shuts down the session's engine actor and waits for it to complete.
    ///
    /// # Errors
    ///
    /// Returns an error if shutdown is requested multiple times or times out.
    pub async fn shutdown_and_wait(&self) -> Result<(), String> {
        self.engine_handle.shutdown_and_wait().await
    }

    /// Creates a new session by starting a dynamic engine actor and spawning forwarding tasks.
    ///
    /// This does not register the session with `SessionManager`. Callers should insert the
    /// returned session into the manager under the appropriate lock.
    ///
    /// # Errors
    ///
    /// Returns an error if subscribing to state or stats updates fails.
    pub async fn create(
        engine: &Engine,
        config: &Config,
        name: Option<String>,
        event_tx: broadcast::Sender<ApiEvent>,
        created_by: Option<String>,
    ) -> Result<Self, String> {
        let session_id = Uuid::new_v4().to_string();
        let display_name = name.as_deref().unwrap_or(&session_id);
        tracing::info!(session_id = %session_id, name = %display_name, "Creating new dynamic session");

        let node_input_capacity = config.engine.resolved_node_input_capacity();
        let pin_distributor_capacity = config.engine.resolved_pin_distributor_capacity();

        tracing::info!(
            session_id = %session_id,
            engine_profile = ?config.engine.profile,
            packet_batch_size = config.engine.packet_batch_size,
            node_input_capacity,
            pin_distributor_capacity,
            "Starting dynamic engine"
        );

        let engine_config = DynamicEngineConfig {
            packet_batch_size: config.engine.packet_batch_size,
            session_id: Some(session_id.clone()),
            node_input_capacity,
            pin_distributor_capacity,
        };

        // Start the long-running dynamic engine actor for this session.
        let engine_handle = engine.start_dynamic_actor(engine_config);

        // Subscribe to state and stats updates from the engine
        let mut state_rx = engine_handle
            .subscribe_state()
            .await
            .map_err(|e| format!("Failed to subscribe to state updates: {e}"))?;
        let mut stats_rx = engine_handle
            .subscribe_stats()
            .await
            .map_err(|e| format!("Failed to subscribe to stats updates: {e}"))?;

        // Spawn task to forward state updates to WebSocket clients
        let session_id_for_state = session_id.clone();
        let event_tx_for_state = event_tx.clone();
        tokio::spawn(async move {
            while let Some(update) = state_rx.recv().await {
                let event = ApiEvent {
                    message_type: MessageType::Event,
                    correlation_id: None,
                    payload: EventPayload::NodeStateChanged {
                        session_id: session_id_for_state.clone(),
                        node_id: update.node_id,
                        state: update.state,
                        timestamp: system_time_to_rfc3339(update.timestamp),
                    },
                };
                // broadcast::send() returns Err when there are no active receivers,
                // but that's okay - just keep forwarding for when clients connect
                let _ = event_tx_for_state.send(event);
            }
            tracing::debug!(session_id = %session_id_for_state, "State forwarding task ended");
        });

        // Spawn task to forward stats updates to WebSocket clients
        let session_id_for_statistics = session_id.clone();
        let event_tx_for_statistics = event_tx.clone();
        tokio::spawn(async move {
            while let Some(update) = stats_rx.recv().await {
                let event = ApiEvent {
                    message_type: MessageType::Event,
                    correlation_id: None,
                    payload: EventPayload::NodeStatsUpdated {
                        session_id: session_id_for_statistics.clone(),
                        node_id: update.node_id,
                        stats: update.stats,
                        timestamp: system_time_to_rfc3339(update.timestamp),
                    },
                };
                // broadcast::send() returns Err when there are no active receivers,
                // but that's okay - just keep forwarding for when clients connect
                let _ = event_tx_for_statistics.send(event);
            }
            tracing::debug!(
                session_id = %session_id_for_statistics,
                "Stats forwarding task ended"
            );
        });

        // Subscribe to telemetry events from the engine
        let mut telemetry_rx = engine_handle
            .subscribe_telemetry()
            .await
            .map_err(|e| format!("Failed to subscribe to telemetry updates: {e}"))?;

        // Spawn task to forward telemetry events to WebSocket clients
        let session_id_for_telemetry = session_id.clone();
        let event_tx_for_telemetry = event_tx.clone();
        let max_text_chars = streamkit_core::telemetry::TelemetryConfig::default().max_text_chars;
        tokio::spawn(async move {
            while let Some(telemetry_event) = telemetry_rx.recv().await {
                // Apply server-side redaction/truncation before forwarding
                let event = create_telemetry_api_event(
                    &session_id_for_telemetry,
                    &telemetry_event,
                    // TODO: Make this configurable via a session-level pipeline telemetry config.
                    max_text_chars,
                );
                // broadcast::send() returns Err when there are no active receivers,
                // but that's okay - just keep forwarding for when clients connect
                let _ = event_tx_for_telemetry.send(event);
            }
            tracing::debug!(
                session_id = %session_id_for_telemetry,
                "Telemetry forwarding task ended"
            );
        });

        Ok(Self {
            id: session_id,
            name,
            engine_handle: Arc::new(engine_handle),
            pipeline: Arc::new(Mutex::new(Pipeline::default())),
            created_at: SystemTime::now(),
            created_by,
        })
    }

    /// Gets the current states of all nodes in this session's pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    pub async fn get_node_states(&self) -> Result<HashMap<String, NodeState>, String> {
        self.engine_handle.get_node_states().await
    }

    /// Gets the current statistics of all nodes in this session's pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    #[allow(dead_code)] // Reserved for future statistics API
    pub async fn get_node_stats(&self) -> Result<HashMap<String, NodeStats>, String> {
        self.engine_handle.get_node_stats().await
    }
}

/// A thread-safe manager for all active sessions.
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    // Metrics
    sessions_active_gauge: opentelemetry::metrics::Gauge<u64>,
    sessions_created_counter: opentelemetry::metrics::Counter<u64>,
    sessions_destroyed_counter: opentelemetry::metrics::Counter<u64>,
    session_duration_histogram: opentelemetry::metrics::Histogram<f64>,
}

impl Default for SessionManager {
    fn default() -> Self {
        let meter = global::meter("skit_sessions");
        Self {
            sessions: HashMap::new(),
            sessions_active_gauge: meter
                .u64_gauge("sessions.active")
                .with_description("Number of active sessions")
                .build(),
            sessions_created_counter: meter
                .u64_counter("sessions.created")
                .with_description("Total number of sessions created")
                .build(),
            sessions_destroyed_counter: meter
                .u64_counter("sessions.destroyed")
                .with_description("Total number of sessions destroyed")
                .build(),
            session_duration_histogram: meter
                .f64_histogram("session.duration")
                .with_description("Session lifetime duration in seconds")
                .with_unit("s")
                .build(),
        }
    }
}

impl SessionManager {
    /// Checks whether a session name already exists.
    pub fn is_name_taken(&self, name: &str) -> bool {
        self.sessions.values().any(|session| session.name.as_deref() == Some(name))
    }

    /// Returns the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Adds a session to the manager.
    ///
    /// # Errors
    ///
    /// Returns an error if a session with the same name already exists.
    pub fn add_session(&mut self, session: Session) -> Result<(), String> {
        // Check for duplicate session names
        if let Some(ref session_name) = session.name {
            if self.is_name_taken(session_name) {
                return Err(format!("Session with name '{session_name}' already exists"));
            }
        }

        self.sessions.insert(session.id.clone(), session);

        // Update metrics
        self.sessions_created_counter.add(1, &[]);
        self.sessions_active_gauge.record(self.sessions.len() as u64, &[]);

        Ok(())
    }

    /// Find session by ID or name
    pub fn get_session_by_name_or_id(&self, identifier: &str) -> Option<Session> {
        // First try by ID
        if let Some(session) = self.sessions.get(identifier) {
            return Some(session.clone());
        }

        // Then try by name
        self.sessions.values().find(|session| session.name.as_deref() == Some(identifier)).cloned()
    }

    /// Helper function to record metrics when a session is destroyed
    fn record_session_destruction(&self, duration_secs: f64) {
        self.sessions_destroyed_counter.add(1, &[]);
        self.sessions_active_gauge.record(self.sessions.len() as u64, &[]);
        self.session_duration_histogram.record(duration_secs, &[]);
    }

    /// Removes a session from the manager by ID and records destruction metrics.
    pub fn remove_session_by_id(&mut self, session_id: &str) -> Option<Session> {
        let session = self.sessions.remove(session_id)?;
        tracing::info!(session_id = %session_id, "Removed session from manager");

        let duration = SystemTime::now().duration_since(session.created_at).unwrap_or_default();
        self.record_session_destruction(duration.as_secs_f64());
        Some(session)
    }

    /// Lists all active sessions
    pub fn list_sessions(&self) -> Vec<Session> {
        self.sessions.values().cloned().collect()
    }
}
