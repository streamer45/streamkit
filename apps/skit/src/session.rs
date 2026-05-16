// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! server/src/session.rs: Manages live, dynamic pipeline sessions.

use crate::config::Config;
use crate::state::BroadcastEvent;
use opentelemetry::global;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use streamkit_api::{Event as ApiEvent, EventPayload, MessageType, Pipeline};
use streamkit_core::control::{ConnectionMode, EngineControlMessage};
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

fn normalize_optional_name(name: Option<String>) -> Option<String> {
    name.and_then(|name| {
        let trimmed = name.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn fnv1a_64(input: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn generate_session_name(session_id: &str) -> String {
    // Deterministic "docker-style" name derived from the session id.
    // This keeps names consistent across clients without requiring additional storage or APIs.
    const ADJECTIVES: &[&str] = &[
        "ancient", "brave", "calm", "clever", "dapper", "eager", "fancy", "gentle", "happy",
        "jolly", "kind", "lively", "mighty", "noble", "proud", "quick", "quiet", "shiny", "silly",
        "swift", "wise",
    ];

    const NOUNS: &[&str] = &[
        "antelope", "badger", "beaver", "bison", "cougar", "dolphin", "dragon", "eagle", "falcon",
        "fox", "gecko", "heron", "ibis", "koala", "lemur", "lynx", "otter", "panther", "puma",
        "raven", "tiger", "yak",
    ];

    let hash = fnv1a_64(session_id);
    let adjective_idx = usize::try_from(hash % ADJECTIVES.len() as u64).unwrap_or(0);
    let noun_idx = usize::try_from((hash >> 8) % NOUNS.len() as u64).unwrap_or(0);
    let adjective = ADJECTIVES[adjective_idx];
    let noun = NOUNS[noun_idx];
    let suffix = (hash >> 16) & 0xffff;

    format!("{adjective}-{noun}-{suffix:04x}")
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
        serde_json::Value::String(s) if s.len() > max_chars => {
            let truncated: String = s.chars().take(max_chars).collect();
            *s = format!("{truncated}...[truncated]");
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

/// Maximum number of concurrent previews per session.
pub const MAX_PREVIEWS_PER_SESSION: usize = 2;

/// A single tap point in a pipeline (node + output pin + media classification).
#[derive(Clone, Debug)]
pub struct TapPoint {
    pub node: String,
    pub pin: String,
    pub is_encoded: bool,
    pub is_audio: bool,
    pub is_video: bool,
}

/// Tracks a single active preview tap on a session.
///
/// A preview dynamically injects a subgraph (encoding chain + MoQ peer) into
/// the running pipeline so an admin can "peek" at any point in the graph.
/// The subgraph is torn down when the preview is stopped or the session is
/// destroyed.
#[derive(Clone, Debug)]
pub struct PreviewState {
    pub preview_id: String,
    /// Tap points this preview is connected to (may be multiple for
    /// pipelines with separate audio and video encoder chains).
    pub tap_points: Vec<TapPoint>,
    /// Node IDs and kinds injected for this preview (teardown in reverse order).
    /// Each entry is `(node_id, node_kind)`.
    pub injected_nodes: Vec<(String, String)>,
    /// Connections injected for this preview
    /// `(from_node, from_pin, to_node, to_pin, mode)`.
    pub injected_connections: Vec<(String, String, String, String, ConnectionMode)>,
    pub gateway_path: String,
    pub has_audio: bool,
    pub has_video: bool,
    pub created_at: SystemTime,
}

/// Represents a single, stateful, dynamic pipeline session.
#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    /// The handle to send control messages to the running DynamicEngine actor.
    engine_handle: Arc<DynamicEngineHandle>,
    pub pipeline: Arc<Mutex<Pipeline>>,
    /// Node IDs that the WebSocket layer has accepted into the engine
    /// actor but for which `NodeAdded`/`Failed` has not yet arrived.
    /// `pipeline.nodes` only sees a node after the engine confirms
    /// successful creation; the in-flight set covers the gap so a
    /// second `addnode` for the same id (whether from the same or a
    /// different client) is rejected at the handler instead of being
    /// silently dropped by the actor's duplicate-id guard.  Drained on
    /// success (node-added forwarder), on failure (state forwarder
    /// observes `Failed`), or when an in-flight node is removed.
    pub creating_nodes: Arc<Mutex<HashSet<String>>>,
    /// Timestamp when the session was created
    pub created_at: SystemTime,
    /// User/role who created this session (for permission filtering)
    pub created_by: Option<String>,
    /// Active preview taps, keyed by preview_id.
    #[cfg(feature = "moq")]
    pub previews: Arc<Mutex<HashMap<String, PreviewState>>>,
}

impl Session {
    /// Reserves a node id for an in-flight `addnode`.
    ///
    /// Atomically checks both the live pipeline snapshot and the
    /// in-flight set, then inserts into the in-flight set.  Returns
    /// `Err` describing why the id is unavailable (already live, or
    /// already being added).  The reservation is drained by the
    /// node-added forwarder on success, the state forwarder on a
    /// non-`Creating` state transition, or by `release_node_id` on
    /// remove/cancellation.
    ///
    /// Lock order: `pipeline` first, then `creating_nodes`.  The two
    /// guards are held jointly across both reads to prevent a node
    /// from slipping into `pipeline.nodes` (via the node-added
    /// forwarder) between the two checks and being silently dropped
    /// by the actor's duplicate-id guard later.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a human-readable reason when the id is
    /// already live or already in flight.
    #[allow(clippy::significant_drop_tightening)] // joint lock is the point
    pub async fn reserve_node_id(&self, node_id: &str) -> Result<(), String> {
        let pipeline = self.pipeline.lock().await;
        if pipeline.nodes.contains_key(node_id) {
            return Err(format!("Node '{node_id}' already exists in the pipeline"));
        }
        let mut creating = self.creating_nodes.lock().await;
        if creating.contains(node_id) {
            return Err(format!("Node '{node_id}' is already being added"));
        }
        creating.insert(node_id.to_string());
        Ok(())
    }

    /// Releases an in-flight reservation taken by `reserve_node_id`.
    /// Used on explicit removal of a still-Creating node.  Idempotent.
    pub async fn release_node_id(&self, node_id: &str) {
        self.creating_nodes.lock().await.remove(node_id);
    }

    /// Forwards a control message to this session's specific engine actor.
    pub async fn send_control_message(&self, msg: EngineControlMessage) {
        if let Err(e) = self.engine_handle.send_control(msg).await {
            tracing::error!(session_id = %self.id, error = %e, "Failed to send control message");
        }
    }

    /// Forwards a control message to this session's engine actor, returning
    /// the error instead of logging it.  Used by preview injection where a
    /// failure must trigger rollback.
    ///
    /// # Errors
    ///
    /// Returns an error string if the engine actor has shut down and the
    /// message cannot be delivered.
    pub async fn try_send_control_message(&self, msg: EngineControlMessage) -> Result<(), String> {
        self.engine_handle
            .send_control(msg)
            .await
            .map_err(|e| format!("Engine control message failed: {e}"))
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
        event_tx: broadcast::Sender<BroadcastEvent>,
        created_by: Option<String>,
    ) -> Result<Self, String> {
        let session_id = Uuid::new_v4().to_string();
        let name =
            normalize_optional_name(name).or_else(|| Some(generate_session_name(&session_id)));
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

        // Pre-allocate the in-flight set so the state and node-added
        // forwarders can both reach it.  `pipeline.nodes` only contains
        // confirmed entries; this set fills the gap for accepted-but-
        // not-yet-confirmed addnode requests.
        let creating_nodes: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        // Spawn task to forward state updates to WebSocket clients
        let session_id_for_state = session_id.clone();
        let event_tx_for_state = event_tx.clone();
        let creating_nodes_for_state = creating_nodes.clone();
        tokio::spawn(async move {
            while let Some(update) = state_rx.recv().await {
                // Drain the in-flight entry as soon as a non-Creating
                // state arrives — a Failed transition means the engine
                // gave up on this id, and a later non-Creating state
                // (Ready/Running/Degraded) confirms the node is past
                // creation.  The node-added forwarder also drains on
                // success; the second remove is a no-op.
                if !matches!(update.state, NodeState::Creating) {
                    creating_nodes_for_state.lock().await.remove(&update.node_id);
                }
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
                let _ = event_tx_for_state.send(BroadcastEvent::to_all(event));
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
                let _ = event_tx_for_statistics.send(BroadcastEvent::to_all(event));
            }
            tracing::debug!(
                session_id = %session_id_for_statistics,
                "Stats forwarding task ended"
            );
        });

        // Subscribe to view data updates from the engine
        let mut view_data_rx = engine_handle
            .subscribe_view_data()
            .await
            .map_err(|e| format!("Failed to subscribe to view data updates: {e}"))?;

        // Spawn task to forward view data updates to WebSocket clients
        let session_id_for_view_data = session_id.clone();
        let event_tx_for_view_data = event_tx.clone();
        tokio::spawn(async move {
            while let Some(update) = view_data_rx.recv().await {
                let event = ApiEvent {
                    message_type: MessageType::Event,
                    correlation_id: None,
                    payload: EventPayload::NodeViewDataUpdated {
                        session_id: session_id_for_view_data.clone(),
                        node_id: update.node_id,
                        data: update.data,
                        timestamp: system_time_to_rfc3339(update.timestamp),
                    },
                };
                let _ = event_tx_for_view_data.send(BroadcastEvent::to_all(event));
            }
            tracing::debug!(
                session_id = %session_id_for_view_data,
                "View data forwarding task ended"
            );
        });

        // Subscribe to runtime schema discovery notifications from the engine
        let mut runtime_schema_rx = engine_handle
            .subscribe_runtime_schemas()
            .await
            .map_err(|e| format!("Failed to subscribe to runtime schema updates: {e}"))?;

        // Spawn task to forward runtime schema updates to WebSocket clients
        let session_id_for_schemas = session_id.clone();
        let event_tx_for_schemas = event_tx.clone();
        tokio::spawn(async move {
            while let Some(update) = runtime_schema_rx.recv().await {
                let event = ApiEvent {
                    message_type: MessageType::Event,
                    correlation_id: None,
                    payload: EventPayload::RuntimeSchemasUpdated {
                        session_id: session_id_for_schemas.clone(),
                        node_id: update.node_id,
                        schema: update.schema,
                    },
                };
                let _ = event_tx_for_schemas.send(BroadcastEvent::to_all(event));
            }
            tracing::debug!(
                session_id = %session_id_for_schemas,
                "Runtime schema forwarding task ended"
            );
        });

        // Subscribe to node-added notifications from the engine and use
        // them as the trigger for both updating `pipeline.nodes` and
        // emitting the public `NodeAdded` event.  Doing this here (and
        // not in the WebSocket addnode handler) means clients only see
        // `nodeadded` after the engine has confirmed the plugin's
        // constructor and `initialize_node` returned Ok — never
        // speculatively before the FFI call has even run.  Failures
        // surface as `NodeStateChanged { state: Failed }` via the
        // existing state forwarder above.
        //
        // Subscribing here (vs earlier in `create`) is safe: the engine
        // can't have any nodes until something sends `AddNode`, and the
        // first `AddNode` can't arrive until `create` returns the
        // handle to its caller.
        let pipeline = Arc::new(Mutex::new(Pipeline::default()));
        let mut node_added_rx = engine_handle
            .subscribe_node_added()
            .await
            .map_err(|e| format!("Failed to subscribe to node-added updates: {e}"))?;
        let session_id_for_node_added = session_id.clone();
        let event_tx_for_node_added = event_tx.clone();
        let pipeline_for_node_added = pipeline.clone();
        let creating_nodes_for_node_added = creating_nodes.clone();
        tokio::spawn(async move {
            while let Some(notification) = node_added_rx.recv().await {
                // Update the cached pipeline snapshot first, then
                // broadcast — late subscribers (re-fetching the pipeline
                // immediately after a `nodeadded` event) see a
                // consistent view that already includes the new entry.
                {
                    let mut pip = pipeline_for_node_added.lock().await;
                    pip.nodes.insert(
                        notification.node_id.clone(),
                        streamkit_api::Node {
                            kind: notification.kind.clone(),
                            params: notification.params.clone(),
                            state: None,
                        },
                    );
                }
                // The node is now visible in pipeline.nodes — drop the
                // in-flight reservation so a future addnode for this id
                // (after a removenode) is gated only by the live
                // pipeline check.
                creating_nodes_for_node_added.lock().await.remove(&notification.node_id);
                let event = ApiEvent {
                    message_type: MessageType::Event,
                    correlation_id: None,
                    payload: EventPayload::NodeAdded {
                        session_id: session_id_for_node_added.clone(),
                        node_id: notification.node_id,
                        kind: notification.kind,
                        params: notification.params,
                    },
                };
                let _ = event_tx_for_node_added.send(BroadcastEvent::to_all(event));
            }
            tracing::debug!(
                session_id = %session_id_for_node_added,
                "Node-added forwarding task ended"
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
                let _ = event_tx_for_telemetry.send(BroadcastEvent::to_all(event));
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
            pipeline,
            creating_nodes,
            created_at: SystemTime::now(),
            created_by,
            #[cfg(feature = "moq")]
            previews: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Gets the current states of all nodes in this session's pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    pub async fn get_node_states(&self) -> Result<Arc<HashMap<String, NodeState>>, String> {
        self.engine_handle.get_node_states().await
    }

    /// Gets the current statistics of all nodes in this session's pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    #[allow(dead_code)] // Reserved for future statistics API
    pub async fn get_node_stats(&self) -> Result<Arc<HashMap<String, NodeStats>>, String> {
        self.engine_handle.get_node_stats().await
    }

    /// Gets the current view data for all nodes in this session's pipeline.
    ///
    /// View data contains resolved runtime state that differs from the static
    /// config params (e.g., compositor resolved layout with aspect-fit adjustments).
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    pub async fn get_node_view_data(
        &self,
    ) -> Result<Arc<HashMap<String, serde_json::Value>>, String> {
        self.engine_handle.get_node_view_data().await
    }

    /// Gets the runtime param schema overrides for all nodes in this session.
    ///
    /// Only nodes whose `ProcessorNode::runtime_param_schema()` returned
    /// `Some` after initialization will have entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine handle's oneshot channel fails to receive a response,
    /// which typically indicates the engine actor has stopped or panicked.
    pub async fn get_runtime_schemas(&self) -> Result<HashMap<String, serde_json::Value>, String> {
        self.engine_handle.get_runtime_schemas().await
    }

    /// Registers a new preview, enforcing the per-session limit.
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum number of concurrent previews has been reached.
    #[cfg(feature = "moq")]
    pub async fn add_preview(&self, state: PreviewState) -> Result<(), String> {
        let mut previews = self.previews.lock().await;
        if previews.len() >= MAX_PREVIEWS_PER_SESSION {
            return Err(format!(
                "Maximum of {MAX_PREVIEWS_PER_SESSION} concurrent previews per session"
            ));
        }
        previews.insert(state.preview_id.clone(), state);
        drop(previews);
        Ok(())
    }

    /// Removes and returns a preview by ID.
    #[cfg(feature = "moq")]
    pub async fn remove_preview(&self, preview_id: &str) -> Option<PreviewState> {
        self.previews.lock().await.remove(preview_id)
    }

    /// Returns a snapshot of all active previews.
    #[cfg(feature = "moq")]
    pub async fn list_previews(&self) -> Vec<PreviewState> {
        self.previews.lock().await.values().cloned().collect()
    }

    /// Returns the number of active previews.
    #[cfg(feature = "moq")]
    pub async fn preview_count(&self) -> usize {
        self.previews.lock().await.len()
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
                .with_boundaries(
                    streamkit_core::metrics::HISTOGRAM_BOUNDARIES_SESSION_DURATION.to_vec(),
                )
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
        if let Some(ref session_name) = session.name {
            if self.is_name_taken(session_name) {
                return Err(format!("Session with name '{session_name}' already exists"));
            }
        }

        self.sessions.insert(session.id.clone(), session);

        self.sessions_created_counter.add(1, &[]);
        self.sessions_active_gauge.record(self.sessions.len() as u64, &[]);

        Ok(())
    }

    /// Find session by ID or name
    pub fn get_session_by_name_or_id(&self, identifier: &str) -> Option<Session> {
        if let Some(session) = self.sessions.get(identifier) {
            return Some(session.clone());
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use streamkit_core::types::{CustomEncoding, CustomPacketData};
    use time::OffsetDateTime;

    fn make_event(
        node_id: &str,
        type_id: &str,
        data: serde_json::Value,
        timestamp_us: Option<u64>,
    ) -> TelemetryEvent {
        TelemetryEvent {
            session_id: None,
            node_id: node_id.to_string(),
            packet: CustomPacketData {
                type_id: type_id.to_string(),
                encoding: CustomEncoding::Json,
                data,
                metadata: timestamp_us.map(|ts| streamkit_core::types::PacketMetadata {
                    timestamp_us: Some(ts),
                    duration_us: None,
                    sequence: None,
                    keyframe: None,
                }),
            },
        }
    }

    #[test]
    fn system_time_to_rfc3339_renders_unix_epoch() {
        let rendered = system_time_to_rfc3339(UNIX_EPOCH);
        assert!(
            rendered.starts_with("1970-01-01T00:00:00"),
            "expected 1970-01-01T00:00:00 prefix, got {rendered}"
        );
        assert!(rendered.ends_with('Z'), "expected trailing Z, got {rendered}");
    }

    // 1_700_000_000s after the unix epoch is 2023-11-14T22:13:20 UTC; that fixed
    // instant is the reference value reused across timestamp tests below.
    const REFERENCE_SECS: u64 = 1_700_000_000;
    const REFERENCE_RFC3339: &str = "2023-11-14T22:13:20Z";

    #[test]
    fn system_time_to_rfc3339_renders_known_instant() {
        let instant = UNIX_EPOCH + Duration::from_secs(REFERENCE_SECS);
        let rendered = system_time_to_rfc3339(instant);
        assert_eq!(rendered, REFERENCE_RFC3339);
    }

    #[test]
    fn timestamp_us_to_rfc3339_renders_unix_epoch() {
        let rendered = timestamp_us_to_rfc3339(0);
        assert!(
            rendered.starts_with("1970-01-01T00:00:00"),
            "expected unix epoch prefix, got {rendered}"
        );
        assert!(rendered.ends_with('Z'));
    }

    #[test]
    fn timestamp_us_to_rfc3339_interprets_microseconds() {
        let micros = REFERENCE_SECS * 1_000_000;
        let rendered = timestamp_us_to_rfc3339(micros);
        assert_eq!(rendered, REFERENCE_RFC3339);
    }

    #[test]
    fn timestamp_us_to_rfc3339_matches_system_time_path() {
        let micros = REFERENCE_SECS * 1_000_000 + 123_456;
        let via_us = timestamp_us_to_rfc3339(micros);
        let via_systime = system_time_to_rfc3339(UNIX_EPOCH + Duration::from_micros(micros));
        assert_eq!(via_us, via_systime);
    }

    // Avoids requiring `time/parsing` as a dev-dep: instead of parsing the
    // rendered string, we verify the helper agrees with the canonical
    // OffsetDateTime formatter and that the SystemTime -> OffsetDateTime
    // conversion preserves the instant to within 1 ms.
    #[test]
    fn system_time_to_rfc3339_round_trips_within_one_ms() {
        let now = SystemTime::now();
        let rendered = system_time_to_rfc3339(now);
        let from_systime = OffsetDateTime::from(now);
        let Ok(canonical) = from_systime.format(&Rfc3339) else {
            panic!("OffsetDateTime should always format as RFC3339");
        };
        assert_eq!(rendered, canonical);

        let Ok(elapsed) = now.duration_since(UNIX_EPOCH) else {
            panic!("SystemTime::now() should be at or after UNIX_EPOCH");
        };
        let now_nanos = elapsed.as_nanos();
        let dt_signed_nanos = from_systime.unix_timestamp_nanos();
        let dt_nanos = u128::try_from(dt_signed_nanos).unwrap_or(0);
        let diff_ns = dt_nanos.abs_diff(now_nanos);
        assert!(
            diff_ns <= 1_000_000,
            "OffsetDateTime drifted from SystemTime by {diff_ns}ns (> 1ms)"
        );
    }

    #[test]
    fn normalize_optional_name_returns_none_for_missing_input() {
        assert_eq!(normalize_optional_name(None), None);
    }

    #[test]
    fn normalize_optional_name_returns_none_for_empty_string() {
        assert_eq!(normalize_optional_name(Some(String::new())), None);
    }

    #[test]
    fn normalize_optional_name_returns_none_for_whitespace_only() {
        assert_eq!(normalize_optional_name(Some("   ".to_string())), None);
        assert_eq!(normalize_optional_name(Some("\t\n  ".to_string())), None);
    }

    #[test]
    fn normalize_optional_name_trims_surrounding_whitespace() {
        assert_eq!(
            normalize_optional_name(Some("  hello  ".to_string())),
            Some("hello".to_string())
        );
    }

    #[test]
    fn normalize_optional_name_passes_through_clean_string() {
        assert_eq!(normalize_optional_name(Some("hello".to_string())), Some("hello".to_string()));
    }

    #[test]
    fn fnv1a_64_empty_input_returns_offset_basis() {
        // Canonical FNV-1a 64-bit offset basis per the algorithm spec.
        assert_eq!(fnv1a_64(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fnv1a_64_is_stable_across_calls() {
        let a = fnv1a_64("streamkit");
        let b = fnv1a_64("streamkit");
        assert_eq!(a, b);
    }

    #[test]
    fn fnv1a_64_distinguishes_distinct_inputs() {
        let inputs = ["a", "b", "ab", "ba", "streamkit", "Streamkit", "session-1"];
        let hashes: HashSet<u64> = inputs.iter().map(|s| fnv1a_64(s)).collect();
        assert_eq!(
            hashes.len(),
            inputs.len(),
            "expected distinct hashes for {inputs:?}, got {hashes:?}"
        );
    }

    fn is_valid_session_name(name: &str) -> bool {
        let parts: Vec<&str> = name.split('-').collect();
        let [adj, noun, suffix] = match parts.as_slice() {
            [adj, noun, suffix] => [*adj, *noun, *suffix],
            _ => return false,
        };
        if adj.is_empty() || !adj.chars().all(|c| c.is_ascii_lowercase()) {
            return false;
        }
        if noun.is_empty() || !noun.chars().all(|c| c.is_ascii_lowercase()) {
            return false;
        }
        suffix.len() == 4
            && suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    #[test]
    fn generate_session_name_matches_documented_shape() {
        let name = generate_session_name("abc-123");
        assert!(is_valid_session_name(&name), "name {name} did not match shape");
    }

    #[test]
    fn generate_session_name_is_deterministic() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(generate_session_name(id), generate_session_name(id));
    }

    #[test]
    fn generate_session_name_diversifies_across_ids() {
        let ids: Vec<String> = (0..16).map(|i| format!("session-{i}")).collect();
        let names: HashSet<String> = ids.iter().map(|id| generate_session_name(id)).collect();
        assert!(
            names.len() >= ids.len() - 1,
            "expected nearly distinct names for {ids:?}, got {names:?}"
        );
        assert!(
            names.len() >= 10,
            "expected at least 10 unique names across 16 ids, got {}",
            names.len()
        );
        for name in &names {
            assert!(is_valid_session_name(name), "invalid name in batch: {name}");
        }
    }

    // The suffix is derived from `(hash >> 16) & 0xffff` per the implementation;
    // this guards against accidental refactors that change the formula.
    #[test]
    fn generate_session_name_hex_suffix_derives_from_fnv1a() {
        for id in ["a", "session-xyz", "1234567890", "the quick brown fox"] {
            let name = generate_session_name(id);
            let Some(suffix) = name.split('-').next_back() else {
                panic!("generated name {name} has no '-' separator");
            };
            let expected = format!("{:04x}", (fnv1a_64(id) >> 16) & 0xffff);
            assert_eq!(suffix, expected, "suffix mismatch for id={id} (name={name})");
        }
    }

    #[test]
    fn redact_leaves_short_strings_unchanged() {
        let mut value = serde_json::json!("hello");
        redact_telemetry_data(&mut value, 16);
        assert_eq!(value, serde_json::json!("hello"));
    }

    #[test]
    fn redact_truncates_long_strings_with_marker() {
        let mut value = serde_json::json!("abcdefghijklmnop");
        redact_telemetry_data(&mut value, 5);
        assert_eq!(value, serde_json::json!("abcde...[truncated]"));
    }

    #[test]
    fn redact_with_zero_max_chars_replaces_payload_with_marker() {
        let mut value = serde_json::json!("non-empty");
        redact_telemetry_data(&mut value, 0);
        assert_eq!(value, serde_json::json!("...[truncated]"));
    }

    #[test]
    fn redact_recurses_into_objects_and_arrays() {
        let mut value = serde_json::json!({
            "shallow": "ok",
            "data": {
                "foo": {
                    "bar": "this string is definitely longer than the limit"
                },
                "list": ["short", "this one is also far longer than the limit"]
            }
        });
        redact_telemetry_data(&mut value, 8);

        assert_eq!(value["shallow"], serde_json::json!("ok"));
        assert_eq!(value["data"]["foo"]["bar"], serde_json::json!("this str...[truncated]"));
        assert_eq!(value["data"]["list"][0], serde_json::json!("short"));
        assert_eq!(value["data"]["list"][1], serde_json::json!("this one...[truncated]"));
    }

    #[test]
    fn redact_leaves_non_string_scalars_unchanged() {
        let mut value = serde_json::json!({
            "n": 42,
            "f": 2.5,
            "b": true,
            "null": serde_json::Value::Null,
            "arr": [1, 2, 3],
        });
        let snapshot = value.clone();
        redact_telemetry_data(&mut value, 1);
        assert_eq!(value, snapshot);
    }

    #[test]
    fn redact_preserves_object_keys() {
        let mut value = serde_json::json!({
            "keep_me": "loooooooooooong value here",
            "and_me": "x",
            "nested": {"inner": "loooooooooooong"},
        });
        redact_telemetry_data(&mut value, 3);
        let Some(obj) = value.as_object() else {
            panic!("root must remain an object after redaction");
        };
        let keys: HashSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            HashSet::from(["keep_me", "and_me", "nested"]),
            "object keys must not be dropped by redaction"
        );
        let Some(nested) = value["nested"].as_object() else {
            panic!("nested value must remain an object after redaction");
        };
        assert!(nested.contains_key("inner"));
    }

    fn assert_node_telemetry(
        event: &ApiEvent,
        expected_session: &str,
        expected_node: &str,
        expected_type: &str,
    ) -> (serde_json::Value, Option<u64>, String) {
        assert_eq!(event.message_type, MessageType::Event);
        assert!(event.correlation_id.is_none());
        match &event.payload {
            EventPayload::NodeTelemetry {
                session_id,
                node_id,
                type_id,
                data,
                timestamp_us,
                timestamp,
            } => {
                assert_eq!(session_id, expected_session);
                assert_eq!(node_id, expected_node);
                assert_eq!(type_id, expected_type);
                (data.clone(), *timestamp_us, timestamp.clone())
            },
            other => panic!("expected NodeTelemetry payload, got {other:?}"),
        }
    }

    #[test]
    fn create_telemetry_api_event_populates_envelope_and_redacts_data() {
        let event = make_event(
            "node-42",
            "plugin::native::vad/vad-event@1",
            serde_json::json!({
                "event_type": "transcript",
                "text": "this transcript should certainly be truncated",
                "confidence": 0.97,
            }),
            Some(REFERENCE_SECS * 1_000_000),
        );

        let api_event = create_telemetry_api_event("sess-1", &event, 4);

        let (data, ts_us, ts_str) = assert_node_telemetry(
            &api_event,
            "sess-1",
            "node-42",
            "plugin::native::vad/vad-event@1",
        );
        assert_eq!(ts_us, Some(REFERENCE_SECS * 1_000_000));
        assert_eq!(ts_str, REFERENCE_RFC3339);

        assert_eq!(data["event_type"], serde_json::json!("tran...[truncated]"));
        assert_eq!(
            data["text"],
            serde_json::json!("this...[truncated]"),
            "long text field was not truncated"
        );
        assert_eq!(
            data["confidence"],
            serde_json::json!(0.97),
            "numeric fields must pass through unchanged"
        );
    }

    #[test]
    fn create_telemetry_api_event_falls_back_to_now_when_metadata_absent() {
        let before = SystemTime::now();
        let event = make_event(
            "node-7",
            "core::telemetry/event@1",
            serde_json::json!({"event_type": "ping"}),
            None,
        );

        let api_event = create_telemetry_api_event("sess-x", &event, 64);
        let (_data, ts_us, ts_str) =
            assert_node_telemetry(&api_event, "sess-x", "node-7", "core::telemetry/event@1");
        assert_eq!(ts_us, None);

        // Timestamp must be a synthetic "now" value and look like RFC3339 with a Z suffix.
        assert!(ts_str.ends_with('Z'), "expected RFC3339 Z suffix, got {ts_str}");
        let after = SystemTime::now();
        let lower = system_time_to_rfc3339(before - Duration::from_secs(1));
        let upper = system_time_to_rfc3339(after + Duration::from_secs(1));
        assert!(
            ts_str.as_str() >= lower.as_str() && ts_str.as_str() <= upper.as_str(),
            "timestamp {ts_str} not within [{lower}, {upper}] window"
        );
    }

    // Telemetry data is documented as an object, but the helper must not panic
    // on non-object payloads and string-typed `data` must still be redacted.
    #[test]
    fn create_telemetry_api_event_handles_non_record_payload() {
        let event = make_event(
            "node-bare",
            "plugin::test/raw@1",
            serde_json::json!("this is a fairly long bare-string payload"),
            Some(0),
        );

        let api_event = create_telemetry_api_event("sess-bare", &event, 8);
        let (data, ts_us, ts_str) =
            assert_node_telemetry(&api_event, "sess-bare", "node-bare", "plugin::test/raw@1");
        assert_eq!(ts_us, Some(0));
        assert!(ts_str.starts_with("1970-01-01T00:00:00"));
        assert_eq!(data, serde_json::json!("this is ...[truncated]"));
    }
}
