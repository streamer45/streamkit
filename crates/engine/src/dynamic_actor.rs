// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Dynamic engine actor implementation (control plane).
//!
//! The DynamicEngine is the control plane actor that manages the pipeline graph,
//! validates connections, tracks node states and statistics, and handles dynamic
//! reconfiguration of the running pipeline.

use crate::{
    constants::DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY,
    dynamic_config::CONTROL_CAPACITY,
    dynamic_messages::{NodeAddedNotification, PinConfigMsg, QueryMessage, RuntimeSchemaUpdate},
    dynamic_pin_distributor::PinDistributorActor,
    graph_builder,
};
use futures::future::FutureExt;
use opentelemetry::KeyValue;
use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, RwLock};
use streamkit_core::control::{EngineControlMessage, NodeControlMessage};
use streamkit_core::error::StreamKitError;
use streamkit_core::frame_pool::{AudioFramePool, VideoFramePool};
use streamkit_core::node::{InitContext, NodeContext, OutputRouting, OutputSender};
use streamkit_core::pins::PinUpdate;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::{NodeState, NodeStateSender, NodeStateUpdate, StopReason};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::view_data::NodeViewDataUpdate;
use streamkit_core::PinCardinality;
use tokio::sync::mpsc;
use tracing::Instrument;

/// Best-effort extraction of a human-readable message from a caught panic
/// payload (`catch_unwind` yields `Box<dyn Any + Send>`).
fn panic_reason(panic: &(dyn Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Metadata about a node's pins, used for runtime type validation in dynamic pipelines.
#[derive(Debug, Clone)]
pub struct NodePinMetadata {
    pub input_pins: Vec<streamkit_core::InputPin>,
    pub output_pins: Vec<streamkit_core::OutputPin>,
}

/// Pre-built OTel metric labels for a node, allocated once on creation and
/// reused on every stats/state update to avoid per-update `String` allocations.
#[derive(Clone)]
pub struct NodeMetricLabels {
    /// `[node_id, node_kind]` plus any bounded pipeline attributes — used by
    /// stats counters.
    pub(crate) stats: Vec<KeyValue>,
    /// Standalone `node_id` label — combined with a varying `state` label.
    pub(crate) node_id_kv: KeyValue,
    /// Bounded pipeline attributes for this node, resolved once at creation.
    /// Extended onto every node-scoped label set (state, transitions) so all of
    /// a node's metrics carry the same attributes from one chokepoint.
    pub(crate) attrs: Vec<KeyValue>,
}

/// Bundle of broadcast channel senders shared by the engine actor loop.
///
/// Grouped into a struct to keep function signatures concise (avoids
/// clippy::too_many_arguments on helpers like `initialize_node`).
struct NodeChannels {
    state: mpsc::Sender<NodeStateUpdate>,
    stats: mpsc::Sender<NodeStatsUpdate>,
    telemetry: mpsc::Sender<TelemetryEvent>,
    view_data: mpsc::Sender<NodeViewDataUpdate>,
}

/// Result of a background node creation task, sent back to the actor loop.
pub struct NodeCreatedEvent {
    node_id: String,
    kind: String,
    creation_id: u64,
    /// Original params from the AddNode request, retained so the success
    /// path can include them in the `NodeAddedNotification` it emits to
    /// session-level forwarders.  `create_node` only borrows them, so we
    /// keep the owned value alongside the result.
    params: Option<serde_json::Value>,
    result: Result<Box<dyn streamkit_core::ProcessorNode>, StreamKitError>,
}

/// A connection request deferred because one or both endpoints are still in
/// `Creating` state and not yet present in `live_nodes`.
#[derive(Debug)]
pub struct PendingConnection {
    from_node: String,
    from_pin: String,
    to_node: String,
    to_pin: String,
    mode: crate::dynamic_messages::ConnectionMode,
}

/// A `TuneNode` message deferred because the target node is still in
/// `Creating` state. Replayed once the node finishes initialization and
/// enters `live_nodes`.
#[derive(Debug)]
pub struct PendingTune {
    node_id: String,
    message: NodeControlMessage,
}

/// The state for the long-running, dynamic engine actor (Control Plane).
pub struct DynamicEngine {
    pub(super) registry: Arc<RwLock<NodeRegistry>>,
    pub(super) control_rx: mpsc::Receiver<EngineControlMessage>,
    pub(super) query_rx: mpsc::Receiver<QueryMessage>,
    pub(super) live_nodes: HashMap<String, graph_builder::LiveNode>,
    /// Map of input Senders: (NodeId, PinName) -> Sender (used when connecting)
    pub(super) node_inputs: HashMap<(String, String), mpsc::Sender<streamkit_core::types::Packet>>,
    /// Map of Pin Distributor configuration Senders: (NodeId, PinName) -> Config Sender
    pub(super) pin_distributors: HashMap<(String, String), mpsc::Sender<PinConfigMsg>>,
    /// Map of Pin Management Senders: NodeId -> Pin Management Sender.
    /// Always created for every node in dynamic pipelines so the engine
    /// can deliver `InputTypeResolved` (and, for dynamic-pin nodes,
    /// `AddedInputPin` / `RemoveInputPin` etc.).
    pub(super) pin_management_txs:
        HashMap<String, mpsc::Sender<streamkit_core::pins::PinManagementMessage>>,
    /// Nodes that declared `supports_dynamic_pins() = true`.  Used to gate
    /// dynamic pin creation requests (`RequestAddInputPin` /
    /// `RequestAddOutputPin`) and to skip strict type validation when a pin
    /// is not yet in metadata.
    pub(super) dynamic_pin_nodes: std::collections::HashSet<String>,
    /// Map of node pin metadata: NodeId -> Pin Metadata (for runtime type validation)
    pub(super) node_pin_metadata: HashMap<String, NodePinMetadata>,
    /// Active connections: (dest_node, dest_pin) -> (source_node, source_pin).
    /// Used for backward tracing when resolving `Passthrough` types in
    /// `InputTypeResolved` delivery.
    pub(super) connections: HashMap<(String, String), (String, String)>,
    /// Map of node_id -> node_kind for labeling metrics
    pub(super) node_kinds: HashMap<String, String>,
    /// Pre-built OTel metric labels per node, allocated once on node creation.
    pub(super) node_metric_labels: HashMap<String, NodeMetricLabels>,
    /// Bounded metric attributes merged into this session's node metrics.
    pub(super) node_attributes: Arc<crate::ResolvedAttributes>,
    pub(super) batch_size: usize,
    /// Session ID for gateway registration (if applicable)
    pub(super) session_id: Option<String>,
    /// Per-pipeline audio buffer pool for hot paths (e.g., Opus decode).
    pub(super) audio_pool: std::sync::Arc<AudioFramePool>,
    /// Per-pipeline video buffer pool for hot paths (e.g., video decode).
    pub(super) video_pool: std::sync::Arc<VideoFramePool>,
    /// Buffer capacity for node input channels
    pub(super) node_input_capacity: usize,
    /// Buffer capacity for pin distributor channels
    pub(super) pin_distributor_capacity: usize,
    pub(super) asset_root: std::path::PathBuf,
    /// Tracks the current state of each node in the pipeline.
    /// Wrapped in `Arc` so that query handlers can cheaply clone the snapshot
    /// instead of deep-copying the entire map.
    pub(super) node_states: Arc<HashMap<String, NodeState>>,
    /// Subscribers that want to receive node state updates
    pub(super) state_subscribers: Vec<mpsc::Sender<NodeStateUpdate>>,
    /// Tracks the current statistics of each node in the pipeline.
    /// Wrapped in `Arc` for cheap query snapshots (see `node_states`).
    pub(super) node_stats: Arc<HashMap<String, NodeStats>>,
    /// Subscribers that want to receive node statistics updates
    pub(super) stats_subscribers: Vec<mpsc::Sender<NodeStatsUpdate>>,
    /// Subscribers that want to receive telemetry events
    pub(super) telemetry_subscribers: Vec<mpsc::Sender<TelemetryEvent>>,
    /// Latest view data per node (e.g., compositor resolved layout).
    /// Wrapped in `Arc` for cheap query snapshots (see `node_states`).
    pub(super) node_view_data: Arc<HashMap<String, serde_json::Value>>,
    /// Subscribers that want to receive node view data updates
    pub(super) view_data_subscribers: Vec<mpsc::Sender<NodeViewDataUpdate>>,
    /// Per-instance runtime param schema overrides discovered after node init.
    /// Only populated for nodes whose `ProcessorNode::runtime_param_schema()`
    /// returns `Some`.
    pub(super) runtime_schemas: HashMap<String, serde_json::Value>,
    /// Subscribers that want to receive runtime schema discovery notifications.
    /// Unbounded because schema discovery is one-per-node and low-frequency;
    /// a bounded channel risks silently dropping a notification that leaves
    /// the UI permanently stale.
    pub(super) runtime_schema_subscribers: Vec<mpsc::UnboundedSender<RuntimeSchemaUpdate>>,
    /// Subscribers that want to receive a notification when a node is
    /// fully created and initialized (i.e. transitioned from `Creating`
    /// to `Initializing`).  This is what session-level forwarders turn
    /// into the public `NodeAdded` event.  Failures are visible via the
    /// existing state subscribers (`NodeState::Failed`) and never appear
    /// here, so a `NodeAddedNotification` always means success.
    ///
    /// Unbounded because node creations are one-per-node and very
    /// low-frequency; a bounded channel risks silently dropping a
    /// notification that leaves the UI permanently without a
    /// `nodeadded` event for that node.  Same model as
    /// `runtime_schema_subscribers` above.
    pub(super) node_added_subscribers: Vec<mpsc::UnboundedSender<NodeAddedNotification>>,
    // Metrics
    pub(super) nodes_active_gauge: opentelemetry::metrics::Gauge<u64>,
    pub(super) node_state_transitions_counter: opentelemetry::metrics::Counter<u64>,
    pub(super) engine_operations_counter: opentelemetry::metrics::Counter<u64>,
    // Node-level packet metrics (counters, not gauges - for proper rate() calculation)
    pub(super) node_packets_received_counter: opentelemetry::metrics::Counter<u64>,
    pub(super) node_packets_sent_counter: opentelemetry::metrics::Counter<u64>,
    pub(super) node_packets_discarded_counter: opentelemetry::metrics::Counter<u64>,
    pub(super) node_packets_errored_counter: opentelemetry::metrics::Counter<u64>,
    // Node state metric (1=running, 0=not running)
    pub(super) node_state_gauge: opentelemetry::metrics::Gauge<u64>,
    /// Clone of the engine's own control sender, handed to every node via
    /// [`NodeContext::engine_control_tx`] so that nodes can emit
    /// [`EngineControlMessage::TuneNode`] to sibling nodes.
    pub(super) engine_control_tx: mpsc::Sender<EngineControlMessage>,
    /// Sender half of the internal channel for background node creation results.
    /// Cloned into each spawned creation task.
    pub(super) node_created_tx: mpsc::Sender<NodeCreatedEvent>,
    /// Receiver half — polled in the actor `select!` loop.
    pub(super) node_created_rx: mpsc::Receiver<NodeCreatedEvent>,
    /// Connections deferred because one or both endpoints are still `Creating`.
    pub(super) pending_connections: Vec<PendingConnection>,
    /// TuneNode messages deferred because the target node is still `Creating`.
    pub(super) pending_tunes: Vec<PendingTune>,
    /// Monotonically increasing counter used to tag each spawned creation task.
    /// Lets `handle_node_created` distinguish stale results (from a previous
    /// Remove → re-Add cycle) from the current active creation.
    pub(super) next_creation_id: u64,
    /// Maps node_id → creation_id for nodes currently in `Creating` state.
    /// When `NodeCreated` arrives, its `creation_id` must match the active
    /// entry; otherwise the result is stale and discarded.
    pub(super) active_creations: HashMap<String, u64>,
}
impl DynamicEngine {
    const fn node_state_name(state: &NodeState) -> &'static str {
        match state {
            NodeState::Creating => "creating",
            NodeState::Initializing => "initializing",
            NodeState::Ready => "ready",
            NodeState::Running => "running",
            NodeState::Recovering { .. } => "recovering",
            NodeState::Degraded { .. } => "degraded",
            NodeState::Failed { .. } => "failed",
            NodeState::Stopped { .. } => "stopped",
        }
    }

    const fn is_terminal(state: &NodeState) -> bool {
        matches!(state, NodeState::Failed { .. } | NodeState::Stopped { .. })
    }

    /// The main actor loop for the dynamic engine (Control Plane).
    pub(super) async fn run(mut self) {
        tracing::info!("Dynamic Engine actor started (Per-Pin Distributor Architecture).");
        let (state_tx, mut state_rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
        let (stats_tx, mut stats_rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
        let (telemetry_tx, mut telemetry_rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
        let (view_data_tx, mut view_data_rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);

        let channels = NodeChannels {
            state: state_tx,
            stats: stats_tx,
            telemetry: telemetry_tx,
            view_data: view_data_tx,
        };

        loop {
            tokio::select! {
                Some(control_msg) = self.control_rx.recv() => {
                    // Shutdown pauses this select loop, so state_rx stops being
                    // drained while the handler joins node tasks. Each task
                    // awaits a terminal send on the state channel after run()
                    // returns (the backstop in initialize_node); closing the
                    // receiver makes a full channel fail those sends fast rather
                    // than block until the join timeout. The terminal states are
                    // discarded during shutdown anyway.
                    if matches!(control_msg, EngineControlMessage::Shutdown) {
                        state_rx.close();
                    }
                    if !self.handle_engine_control(control_msg).await {
                        break; // Shutdown requested
                    }
                },
                Some(created) = self.node_created_rx.recv() => {
                    self.handle_node_created(created, &channels).await;
                },
                Some(query_msg) = self.query_rx.recv() => {
                    self.handle_query(query_msg).await;
                },
                Some(state_update) = state_rx.recv() => {
                    self.handle_state_update(&state_update);
                },
                Some(stats_update) = stats_rx.recv() => {
                    self.handle_stats_update(&stats_update);
                },
                Some(telemetry_event) = telemetry_rx.recv() => {
                    self.handle_telemetry_event(&telemetry_event);
                },
                Some(view_data_update) = view_data_rx.recv() => {
                    self.handle_view_data_update(&view_data_update);
                },
                else => break,
            }
        }
        tracing::info!("Dynamic Engine actor shutting down.");
    }

    /// Handles query messages for retrieving information without modifying state.
    async fn handle_query(&mut self, msg: QueryMessage) {
        match msg {
            QueryMessage::GetNodeStates { response_tx } => {
                let _ = response_tx.send(Arc::clone(&self.node_states)).await;
            },
            QueryMessage::GetNodeStats { response_tx } => {
                let _ = response_tx.send(Arc::clone(&self.node_stats)).await;
            },
            QueryMessage::SubscribeState { response_tx } => {
                let (tx, rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
                self.state_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
            QueryMessage::SubscribeStats { response_tx } => {
                let (tx, rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
                self.stats_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
            QueryMessage::SubscribeTelemetry { response_tx } => {
                let (tx, rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
                self.telemetry_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
            QueryMessage::SubscribeViewData { response_tx } => {
                let (tx, rx) = mpsc::channel(DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY);
                self.view_data_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
            QueryMessage::GetNodeViewData { response_tx } => {
                let _ = response_tx.send(Arc::clone(&self.node_view_data)).await;
            },
            QueryMessage::GetRuntimeSchemas { response_tx } => {
                let _ = response_tx.send(self.runtime_schemas.clone()).await;
            },
            QueryMessage::SubscribeRuntimeSchemas { response_tx } => {
                let (tx, rx) = mpsc::unbounded_channel();
                self.runtime_schema_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
            QueryMessage::SubscribeNodeAdded { response_tx } => {
                let (tx, rx) = mpsc::unbounded_channel();
                self.node_added_subscribers.push(tx);
                let _ = response_tx.send(rx).await;
            },
        }
    }

    /// Sends `Start` to source nodes once every node has left `Creating`.
    pub(crate) fn check_and_activate_pipeline(&self) {
        use tokio::sync::mpsc::error::TrySendError;

        if self.node_states.is_empty() {
            return;
        }

        // Check if all nodes are in an active state.
        // Degraded and Recovering count as activatable because nodes like the mixer
        // enter Degraded while waiting for input, and transport nodes enter Recovering
        // on transient connection failures — neither should block source node activation.
        // Failed and Stopped nodes are excluded: starting sources into a broken pipeline
        // would just produce packets that go nowhere.
        let all_ready = self.node_states.values().all(|state| {
            matches!(
                state,
                NodeState::Ready
                    | NodeState::Running
                    | NodeState::Degraded { .. }
                    | NodeState::Recovering { .. }
            )
        });

        if !all_ready {
            return;
        }

        let ready_nodes: Vec<String> = self
            .node_states
            .iter()
            .filter_map(|(node_id, state)| {
                if matches!(state, NodeState::Ready) {
                    Some(node_id.clone())
                } else {
                    None
                }
            })
            .collect();

        if ready_nodes.is_empty() {
            return;
        }

        // Prefer sending Start only to source nodes (nodes with no inputs).
        // If we don't have metadata for a node (unexpected), fall back to starting it
        // to preserve the prior behavior (start all Ready nodes).
        let start_targets: Vec<String> = ready_nodes
            .into_iter()
            .filter(|node_id| {
                self.node_pin_metadata.get(node_id).is_none_or(|meta| meta.input_pins.is_empty())
            })
            .collect();

        if start_targets.is_empty() {
            return;
        }

        tracing::info!(
            "All {} nodes ready, activating {} source nodes",
            self.node_states.len(),
            start_targets.len()
        );

        // try_send fast-path; fall back to spawned send on backpressure
        // to avoid stalling the control-plane task.
        for node_id in start_targets {
            if let Some(live_node) = self.live_nodes.get(&node_id) {
                tracing::info!("Sending Start signal to node: {}", node_id);
                match live_node.control_tx.try_send(NodeControlMessage::Start) {
                    Ok(()) => {},
                    Err(TrySendError::Full(_)) => {
                        let tx = live_node.control_tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(NodeControlMessage::Start).await;
                        });
                    },
                    Err(TrySendError::Closed(_)) => {
                        tracing::debug!(
                            "Cannot send Start to '{}': control channel closed",
                            node_id
                        );
                    },
                }
            }
        }
    }

    pub(crate) fn handle_state_update(&mut self, update: &NodeStateUpdate) {
        let Some(live_node) = self.live_nodes.get(&update.node_id) else {
            tracing::trace!(
                node = %update.node_id,
                state = ?update.state,
                "Ignoring state update for removed node"
            );
            return;
        };

        // Discard updates stamped for a previous incarnation of this id: a
        // terminal update enqueued by an old instance (guaranteed in flight by
        // the #600 backstop) must not be applied to a new node that reused the
        // id after a remove → re-add (#606).
        if update.generation != live_node.generation {
            tracing::debug!(
                node = %update.node_id,
                state = ?update.state,
                update_generation = update.generation,
                live_generation = live_node.generation,
                "Discarding state update from a stale node generation"
            );
            return;
        }

        // Once a node is terminal, a later terminal update carries no new
        // information — it is the actor-level mirror of the task result that
        // backstops a dropped best-effort emission (see initialize_node).
        // Collapsing it avoids double-counting transitions and re-notifying
        // subscribers with a state they already have.
        if Self::is_terminal(&update.state)
            && self.node_states.get(&update.node_id).is_some_and(Self::is_terminal)
        {
            tracing::trace!(
                node = %update.node_id,
                state = ?update.state,
                "Ignoring redundant terminal state update"
            );
            return;
        }

        tracing::debug!(
            node = %update.node_id,
            state = ?update.state,
            "Node state updated"
        );

        let state_name = Self::node_state_name(&update.state);
        self.node_state_transitions_counter
            .add(1, &self.node_state_labels(&update.node_id, state_name));

        // One-hot gauge: zero-out previous state, set new state to 1.
        if let Some(prev_state) = self.node_states.get(&update.node_id) {
            let prev_state_name = Self::node_state_name(prev_state);
            if prev_state_name != state_name {
                self.node_state_gauge
                    .record(0, &self.node_state_labels(&update.node_id, prev_state_name));
            }
        }
        self.node_state_gauge.record(1, &self.node_state_labels(&update.node_id, state_name));

        Arc::make_mut(&mut self.node_states).insert(update.node_id.clone(), update.state.clone());

        self.check_and_activate_pipeline();

        // State updates are retried on backpressure (not dropped) to avoid
        // clients showing stale status.
        self.state_subscribers.retain(|subscriber| match subscriber.try_send(update.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let subscriber = subscriber.clone();
                let update = update.clone();
                tokio::spawn(async move {
                    let _ = subscriber.send(update).await;
                });
                true
            },
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    pub(crate) fn handle_telemetry_event(&mut self, event: &TelemetryEvent) {
        self.telemetry_subscribers.retain(|subscriber| match subscriber.try_send(event.clone()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// View data is best-effort: dropped updates are acceptable.
    pub(crate) fn handle_view_data_update(&mut self, update: &NodeViewDataUpdate) {
        if !self.live_nodes.contains_key(&update.node_id) {
            tracing::trace!(
                node = %update.node_id,
                "Ignoring view data update for removed node"
            );
            return;
        }

        Arc::make_mut(&mut self.node_view_data).insert(update.node_id.clone(), update.data.clone());
        self.view_data_subscribers.retain(|subscriber| match subscriber.try_send(update.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!(
                    node = %update.node_id,
                    "View data update dropped (subscriber channel full)"
                );
                true
            },
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Stores a stats update and broadcasts it to subscribers.
    pub(crate) fn handle_stats_update(&mut self, update: &NodeStatsUpdate) {
        if !self.live_nodes.contains_key(&update.node_id) {
            tracing::trace!(
                node = %update.node_id,
                "Ignoring stats update for removed node"
            );
            return;
        }

        tracing::trace!(
            node = %update.node_id,
            received = update.stats.received,
            sent = update.stats.sent,
            discarded = update.stats.discarded,
            errored = update.stats.errored,
            "Node stats updated"
        );

        let fallback;
        let labels = if let Some(cached) = self.node_metric_labels.get(&update.node_id) {
            &cached.stats
        } else {
            tracing::warn!(
                node = %update.node_id,
                "Missing cached metric labels for live node; using fallback"
            );
            let node_kind = self.node_kinds.get(&update.node_id).map_or("unknown", String::as_str);
            let mut labels = vec![
                KeyValue::new("node_id", update.node_id.clone()),
                KeyValue::new("node_kind", node_kind.to_string()),
            ];
            labels.extend(self.node_attributes.for_node(&update.node_id));
            fallback = labels;
            &fallback
        };

        let prev_stats = self.node_stats.get(&update.node_id);

        let delta_received = prev_stats.map_or(update.stats.received, |prev| {
            if update.stats.received < prev.received {
                update.stats.received
            } else {
                update.stats.received - prev.received
            }
        });
        let delta_sent = prev_stats.map_or(update.stats.sent, |prev| {
            if update.stats.sent < prev.sent {
                update.stats.sent
            } else {
                update.stats.sent - prev.sent
            }
        });
        let delta_discarded = prev_stats.map_or(update.stats.discarded, |prev| {
            if update.stats.discarded < prev.discarded {
                update.stats.discarded
            } else {
                update.stats.discarded - prev.discarded
            }
        });
        let delta_errored = prev_stats.map_or(update.stats.errored, |prev| {
            if update.stats.errored < prev.errored {
                update.stats.errored
            } else {
                update.stats.errored - prev.errored
            }
        });

        self.node_packets_received_counter.add(delta_received, labels);
        self.node_packets_sent_counter.add(delta_sent, labels);
        self.node_packets_discarded_counter.add(delta_discarded, labels);
        self.node_packets_errored_counter.add(delta_errored, labels);

        Arc::make_mut(&mut self.node_stats).insert(update.node_id.clone(), update.stats.clone());

        // Broadcast to all subscribers
        self.stats_subscribers.retain(|subscriber| {
            // Keep subscribers on transient backpressure (Full); remove only when Closed.
            //
            // Stats are high-frequency, best-effort updates; dropping an update is acceptable.
            match subscriber.try_send(update.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }

    /// Initialize a node and spawn its I/O actors (Pin Distributors).
    ///
    /// `generation` is the node instance's epoch (its `creation_id`); it is
    /// stamped onto every state emission so a stale update from a prior
    /// incarnation can be discarded by `handle_state_update` (#606).
    async fn initialize_node(
        &mut self,
        node: Box<dyn streamkit_core::ProcessorNode>,
        node_id: &str,
        kind: &str,
        generation: u64,
        channels: &NodeChannels,
    ) -> Result<(), StreamKitError> {
        let mut node = node;

        let state_tx = NodeStateSender::new(channels.state.clone(), generation);
        let init_ctx = InitContext { node_id: node_id.to_string(), state_tx: state_tx.clone() };
        match node.initialize(&init_ctx).await {
            Ok(PinUpdate::NoChange | PinUpdate::Updated { .. }) => {},
            Err(e) => {
                return Err(e);
            },
        }

        // Query runtime param schema after init (before spawning the run loop,
        // which consumes the node via `Box<Self>`).
        if let Some(schema) = node.runtime_param_schema() {
            self.runtime_schemas.insert(node_id.to_string(), schema.clone());

            let update = RuntimeSchemaUpdate { node_id: node_id.to_string(), schema };
            self.runtime_schema_subscribers
                .retain(|subscriber| subscriber.send(update.clone()).is_ok());
        }

        let (control_tx, control_rx) = mpsc::channel(CONTROL_CAPACITY);

        let input_pins = node.input_pins();
        let output_pins = node.output_pins();
        self.node_pin_metadata.insert(
            node_id.to_string(),
            NodePinMetadata { input_pins: input_pins.clone(), output_pins: output_pins.clone() },
        );

        let mut node_inputs_map = HashMap::new();
        for pin in input_pins {
            let (tx, rx) = mpsc::channel(self.node_input_capacity);
            self.node_inputs.insert((node_id.to_string(), pin.name.clone()), tx);
            node_inputs_map.insert(pin.name, rx);
        }

        let mut node_outputs_map = HashMap::new();
        for pin in output_pins {
            let (data_tx, data_rx) = mpsc::channel(self.pin_distributor_capacity);
            let (config_tx, config_rx) = mpsc::channel(CONTROL_CAPACITY);

            let distributor = PinDistributorActor::new(
                data_rx,
                config_rx,
                node_id.to_string(),
                pin.name.clone(),
                self.node_attributes.for_node(node_id),
            );
            tokio::spawn(distributor.run());

            self.pin_distributors.insert((node_id.to_string(), pin.name.clone()), config_tx);
            node_outputs_map.insert(pin.name.clone(), data_tx);
        }

        // broadcast_state_update zeroes the previous gauge atomically.
        self.broadcast_state_update(node_id, NodeState::Initializing);
        Arc::make_mut(&mut self.node_stats).insert(node_id.to_string(), NodeStats::default());

        // Always created: all nodes need `InputTypeResolved`; dynamic-pin
        // nodes also receive `AddedInputPin`/`RemoveInputPin` here.
        let (pin_management_tx, pin_management_rx) = mpsc::channel(CONTROL_CAPACITY);
        self.pin_management_txs.insert(node_id.to_string(), pin_management_tx);
        if node.supports_dynamic_pins() {
            self.dynamic_pin_nodes.insert(node_id.to_string());
        }

        let context = NodeContext {
            inputs: node_inputs_map,
            // Dynamic pipelines wire connections after nodes are spawned, so
            // input types are not known at construction time.
            input_types: HashMap::new(),
            control_rx,
            output_sender: OutputSender::new(
                node_id.to_string(),
                OutputRouting::Direct(node_outputs_map),
            ),
            batch_size: self.batch_size,
            state_tx,
            stats_tx: Some(channels.stats.clone()),
            telemetry_tx: Some(channels.telemetry.clone()),
            session_id: self.session_id.clone(),
            cancellation_token: None, // Dynamic pipelines don't use cancellation tokens
            pin_management_rx: Some(pin_management_rx),
            audio_pool: Some(self.audio_pool.clone()),
            video_pool: Some(self.video_pool.clone()),
            pipeline_mode: streamkit_core::PipelineMode::Dynamic,
            view_data_tx: Some(channels.view_data.clone()),
            engine_control_tx: Some(self.engine_control_tx.clone()),
            asset_root: self.asset_root.clone(),
        };

        let final_state_tx = context.state_tx.clone();
        let node_id_for_task = node_id.to_string();
        let run_span = tracing::info_span!(
            "node_run",
            session.id = %self.session_id.as_deref().unwrap_or("<unknown>"),
            node.name = %node_id,
            node.kind = %kind
        );
        // A dead/panicked worker surfaces only as the task's `Err`; the node's
        // own terminal `Failed` is a best-effort `try_send` that backpressure
        // can drop, leaving the node stuck at its last good state. Mirror the
        // task result onto the shared state channel with an awaited send (as
        // graph_builder does for oneshot) so the terminal state is guaranteed.
        // FIFO ordering means any terminal state the node already emitted is
        // processed first; handle_state_update collapses the redundant follow-up.
        //
        // `run()` is wrapped in `catch_unwind` so a panic *inside the node
        // future itself* (as opposed to a native worker thread, which is caught
        // at the FFI boundary and surfaces as `Err`) is converted into a
        // terminal `Failed` instead of unwinding the task before the
        // reconciliation send below ever runs (#605).
        let task_handle = tokio::spawn(
            async move {
                let run_result = AssertUnwindSafe(node.run(context)).catch_unwind().await;
                let result: Result<(), StreamKitError> = match run_result {
                    Ok(result) => result,
                    Err(panic) => {
                        let reason = panic_reason(&panic);
                        tracing::error!(
                            node = %node_id_for_task,
                            panic = %reason,
                            "node.run() future panicked; reporting terminal Failed"
                        );
                        Err(StreamKitError::Runtime(format!("node panicked: {reason}")))
                    },
                };
                let final_state = match &result {
                    Ok(()) => NodeState::Stopped { reason: StopReason::Completed },
                    Err(e) => NodeState::Failed { reason: e.to_string() },
                };
                let _ =
                    final_state_tx.send(NodeStateUpdate::new(node_id_for_task, final_state)).await;
                result
            }
            .instrument(run_span),
        );
        self.live_nodes.insert(
            node_id.to_string(),
            graph_builder::LiveNode { control_tx, task_handle, generation },
        );
        self.nodes_active_gauge
            .record(self.live_nodes.len() as u64, &self.node_attributes.pipeline);
        Ok(())
    }

    /// Runtime type-check for a proposed connection. Passthrough types are
    /// allowed and resolved later.
    pub(crate) fn validate_connection_types(
        &self,
        from_node: &str,
        from_pin: &str,
        to_node: &str,
        to_pin: &str,
    ) -> Result<(), String> {
        fn match_dynamic_pin<'a>(
            pins: &'a [streamkit_core::InputPin],
            pin: &str,
        ) -> Option<&'a streamkit_core::InputPin> {
            pins.iter().find(|p| {
                matches!(&p.cardinality, PinCardinality::Dynamic { prefix } if PinCardinality::is_dynamic_pin_match(prefix, pin))
            })
        }

        fn match_dynamic_output_pin<'a>(
            pins: &'a [streamkit_core::OutputPin],
            pin: &str,
        ) -> Option<&'a streamkit_core::OutputPin> {
            pins.iter().find(|p| {
                matches!(&p.cardinality, PinCardinality::Dynamic { prefix } if PinCardinality::is_dynamic_pin_match(prefix, pin))
            })
        }

        let source_metadata = self
            .node_pin_metadata
            .get(from_node)
            .ok_or_else(|| format!("Source node '{from_node}' not found"))?;
        let dest_metadata = self
            .node_pin_metadata
            .get(to_node)
            .ok_or_else(|| format!("Destination node '{to_node}' not found"))?;

        let source_pin = source_metadata
            .output_pins
            .iter()
            .find(|p| p.name == from_pin)
            .or_else(|| match_dynamic_output_pin(&source_metadata.output_pins, from_pin));
        let Some(source_pin) = source_pin else {
            // Dynamic-pin nodes create pins on-demand; skip validation.
            if self.dynamic_pin_nodes.contains(from_node) {
                tracing::debug!(
                    "Source pin {}.{} not in metadata, but node supports dynamic pins; skipping strict type validation",
                    from_node,
                    from_pin
                );
                return Ok(());
            }
            return Err(format!("Source pin '{from_pin}' not found on node '{from_node}'"));
        };

        let dest_pin = dest_metadata
            .input_pins
            .iter()
            .find(|p| p.name == to_pin)
            .or_else(|| match_dynamic_pin(&dest_metadata.input_pins, to_pin));
        let Some(dest_pin) = dest_pin else {
            if self.dynamic_pin_nodes.contains(to_node) {
                tracing::debug!(
                    "Destination pin {}.{} not in metadata, but node supports dynamic pins; skipping strict type validation",
                    to_node,
                    to_pin
                );
                return Ok(());
            }
            return Err(format!("Destination pin '{to_pin}' not found on node '{to_node}'"));
        };

        if matches!(source_pin.produces_type, streamkit_core::types::PacketType::Passthrough) {
            tracing::debug!(
                "Source pin {}.{} uses Passthrough - type will be resolved at runtime",
                from_node,
                from_pin
            );
            return Ok(());
        }

        if dest_pin
            .accepts_types
            .iter()
            .any(|t| matches!(t, streamkit_core::types::PacketType::Any))
        {
            return Ok(());
        }

        if dest_pin
            .accepts_types
            .iter()
            .any(|t| matches!(t, streamkit_core::types::PacketType::Passthrough))
        {
            tracing::debug!(
                "Destination pin {}.{} accepts Passthrough - type will be resolved at runtime",
                to_node,
                to_pin
            );
            return Ok(());
        }

        let registry = streamkit_core::packet_meta::packet_type_registry();
        if !streamkit_core::packet_meta::can_connect_any(
            &source_pin.produces_type,
            &dest_pin.accepts_types,
            registry,
        ) {
            return Err(format!(
                "Type mismatch: source produces {:?}, but destination accepts {:?}",
                source_pin.produces_type, dest_pin.accepts_types
            ));
        }

        Ok(())
    }

    /// Helper function to connect nodes by configuring the Pin Distributor.
    ///
    /// May create dynamic pins on-demand if the destination node supports them.
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    async fn connect_nodes(
        &mut self,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
        mode: crate::dynamic_messages::ConnectionMode,
    ) {
        tracing::info!(
            "Connecting {}.{} -> {}.{} (mode: {:?})",
            from_node,
            from_pin,
            to_node,
            to_pin,
            mode
        );

        if let Err(e) = self.validate_connection_types(&from_node, &from_pin, &to_node, &to_pin) {
            tracing::error!(
                "Cannot connect {}.{} -> {}.{}: {}",
                from_node,
                from_pin,
                to_node,
                to_pin,
                e
            );
            return;
        }

        // AddedInputPin is deferred until after source output pin resolution
        // so the channel is ready before InputTypeResolved arrives.
        let mut created_dynamic_input: Option<String> = None;
        let mut pending_input_pin_activation: Option<(
            streamkit_core::InputPin,
            mpsc::Receiver<streamkit_core::types::Packet>,
            Option<mpsc::Sender<streamkit_core::UpstreamHint>>,
        )> = None;
        let mut pending_hint_rx: Option<mpsc::Receiver<streamkit_core::UpstreamHint>>;
        let dest_tx = if let Some(tx) = self.node_inputs.get(&(to_node.clone(), to_pin.clone())) {
            let (hint_tx, hint_rx) = mpsc::channel::<streamkit_core::UpstreamHint>(1);
            pending_hint_rx = Some(hint_rx);
            if let Some(pin_mgmt_tx) = self.pin_management_txs.get(&to_node) {
                let msg = streamkit_core::pins::PinManagementMessage::AttachHintSender {
                    pin_name: to_pin.clone(),
                    hint_tx,
                };
                if pin_mgmt_tx.send(msg).await.is_err() {
                    tracing::warn!(
                        "Failed to send AttachHintSender to '{}' for pin '{}' — node may have stopped",
                        to_node, to_pin
                    );
                }
            }
            tx.clone()
        } else if self.dynamic_pin_nodes.contains(&to_node) {
            let Some(pin_mgmt_tx) = self.pin_management_txs.get(&to_node) else {
                tracing::error!(
                    "No pin management channel for dynamic-pin node '{}' — cannot create input pin",
                    to_node
                );
                return;
            };
            tracing::info!(
                "Dynamically creating input pin '{}.{}' for connection",
                to_node,
                to_pin
            );

            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let msg = streamkit_core::pins::PinManagementMessage::RequestAddInputPin {
                suggested_name: Some(to_pin.clone()),
                response_tx,
            };

            if pin_mgmt_tx.send(msg).await.is_err() {
                tracing::error!(
                    "Failed to send pin creation request to node '{}'. It may have stopped.",
                    to_node
                );
                return;
            }

            // Timeout avoids blocking the engine if the node is unresponsive.
            let pin =
                match tokio::time::timeout(std::time::Duration::from_secs(5), response_rx).await {
                    Ok(Ok(Ok(pin))) => pin,
                    Ok(Ok(Err(e))) => {
                        tracing::error!("Node '{}' rejected input pin creation: {}", to_node, e);
                        return;
                    },
                    Ok(Err(_)) | Err(_) => {
                        tracing::error!(
                        "Node '{}' did not respond to input pin creation (dropped or timed out)",
                        to_node
                    );
                        return;
                    },
                };

            let (tx, rx) = mpsc::channel(self.node_input_capacity);
            self.node_inputs.insert((to_node.clone(), pin.name.clone()), tx.clone());

            // Hint channel: downstream → upstream advisory hints (e.g. preferred size).
            let (hint_tx, hint_rx) = mpsc::channel::<streamkit_core::UpstreamHint>(1);
            pending_hint_rx = Some(hint_rx);

            let meta = self.node_pin_metadata.entry(to_node.clone()).or_insert_with(|| {
                NodePinMetadata { input_pins: Vec::new(), output_pins: Vec::new() }
            });
            if !meta.input_pins.iter().any(|p| p.name == pin.name) {
                meta.input_pins.push(pin.clone());
            }

            // Defer sending AddedInputPin until after the source output pin
            // is resolved so AddedInputPin arrives before InputTypeResolved —
            // the node needs the channel ready before it receives type info.
            created_dynamic_input = Some(pin.name.clone());
            pending_input_pin_activation = Some((pin, rx, Some(hint_tx)));
            tx
        } else {
            tracing::error!(
                "Cannot connect: Destination input '{}.{}' not found and node doesn't support dynamic pins.",
                to_node,
                to_pin
            );
            return;
        };

        let config_tx;
        let source_produces_type: streamkit_core::types::PacketType;
        let mut created_dynamic_output: Option<String> = None;

        if let Some(tx) = self.pin_distributors.get(&(from_node.clone(), from_pin.clone())) {
            config_tx = tx.clone();

            // Look up produces_type from existing pin metadata.
            source_produces_type = self
                .node_pin_metadata
                .get(&from_node)
                .and_then(|m| m.output_pins.iter().find(|p| p.name == from_pin))
                .map_or(streamkit_core::types::PacketType::Any, |p| p.produces_type.clone());
        } else if self.dynamic_pin_nodes.contains(&from_node) {
            let Some(pin_mgmt_tx) = self.pin_management_txs.get(&from_node) else {
                tracing::error!(
                    "No pin management channel for dynamic-pin node '{}' — cannot create output pin",
                    from_node
                );
                return;
            };
            tracing::info!(
                "Dynamically creating output pin '{}.{}' for connection",
                from_node,
                from_pin
            );

            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let msg = streamkit_core::pins::PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some(from_pin.clone()),
                response_tx,
            };

            if pin_mgmt_tx.send(msg).await.is_err() {
                tracing::error!(
                    "Failed to send output pin creation request to node '{}'. It may have stopped.",
                    from_node
                );
                if let Some(ref input_pin) = created_dynamic_input {
                    self.rollback_dynamic_input(&to_node, input_pin).await;
                }
                return;
            }

            // Timeout avoids blocking the engine if the node is unresponsive.
            let pin =
                match tokio::time::timeout(std::time::Duration::from_secs(5), response_rx).await {
                    Ok(Ok(Ok(pin))) => pin,
                    Ok(Ok(Err(e))) => {
                        tracing::error!("Node '{}' rejected output pin creation: {}", from_node, e);
                        if let Some(ref input_pin) = created_dynamic_input {
                            self.rollback_dynamic_input(&to_node, input_pin).await;
                        }
                        return;
                    },
                    Ok(Err(_)) | Err(_) => {
                        tracing::error!(
                        "Node '{}' did not respond to output pin creation (dropped or timed out)",
                        from_node
                    );
                        if let Some(ref input_pin) = created_dynamic_input {
                            self.rollback_dynamic_input(&to_node, input_pin).await;
                        }
                        return;
                    },
                };

            // The engine uses `from_pin` as the connection key while the
            // distributor is stored under `pin.name`.  These must match;
            // a divergence would cause disconnect_nodes to miss the entry.
            debug_assert_eq!(
                pin.name, from_pin,
                "Node returned pin name '{}' but engine expected '{}'",
                pin.name, from_pin
            );

            let (data_tx, data_rx) = mpsc::channel(self.pin_distributor_capacity);
            let (cfg_tx, cfg_rx) = mpsc::channel(CONTROL_CAPACITY);

            let distributor = PinDistributorActor::new(
                data_rx,
                cfg_rx,
                from_node.clone(),
                pin.name.clone(),
                self.node_attributes.for_node(&from_node),
            );
            tokio::spawn(distributor.run());

            self.pin_distributors.insert((from_node.clone(), pin.name.clone()), cfg_tx.clone());

            let meta = self.node_pin_metadata.entry(from_node.clone()).or_insert_with(|| {
                NodePinMetadata { input_pins: Vec::new(), output_pins: Vec::new() }
            });
            if !meta.output_pins.iter().any(|p| p.name == pin.name) {
                meta.output_pins.push(pin.clone());
            }

            // Validate type compatibility now that the pin is concrete.
            if let Some(dest_meta) = self.node_pin_metadata.get(&to_node) {
                let dest_pin_def = dest_meta.input_pins.iter().find(|p| p.name == to_pin);
                if let Some(dest_pin_def) = dest_pin_def {
                    let registry = streamkit_core::packet_meta::packet_type_registry();
                    if !streamkit_core::packet_meta::can_connect_any(
                        &pin.produces_type,
                        &dest_pin_def.accepts_types,
                        registry,
                    ) {
                        tracing::error!(
                            "Type mismatch after dynamic pin creation: {}.{} produces {:?}, but {}.{} accepts {:?}",
                            from_node, pin.name, pin.produces_type,
                            to_node, to_pin, dest_pin_def.accepts_types
                        );
                        if let Some(cfg) =
                            self.pin_distributors.remove(&(from_node.clone(), pin.name.clone()))
                        {
                            let _ = cfg.send(PinConfigMsg::Shutdown).await;
                        }
                        if let Some(meta) = self.node_pin_metadata.get_mut(&from_node) {
                            meta.output_pins.retain(|p| p.name != pin.name);
                        }
                        if let Some(ref input_pin) = created_dynamic_input {
                            self.rollback_dynamic_input(&to_node, input_pin).await;
                        }
                        return;
                    }
                }
            }

            let pin_name_for_cleanup = pin.name.clone();
            source_produces_type = pin.produces_type.clone();
            let added_msg = streamkit_core::pins::PinManagementMessage::AddedOutputPin {
                pin,
                channel: data_tx,
            };

            if pin_mgmt_tx.send(added_msg).await.is_err() {
                tracing::error!(
                    "Failed to send output pin activation to node '{}'. It may have stopped.",
                    from_node
                );
                if let Some(cfg) =
                    self.pin_distributors.remove(&(from_node.clone(), pin_name_for_cleanup.clone()))
                {
                    let _ = cfg.send(PinConfigMsg::Shutdown).await;
                }
                if let Some(meta) = self.node_pin_metadata.get_mut(&from_node) {
                    meta.output_pins.retain(|p| p.name != pin_name_for_cleanup);
                }
                if let Some(ref input_pin) = created_dynamic_input {
                    self.rollback_dynamic_input(&to_node, input_pin).await;
                }
                return;
            }

            created_dynamic_output = Some(from_pin.clone());
            config_tx = cfg_tx;
        } else {
            tracing::error!(
                    "Cannot connect: Source output '{}.{}' distributor not found and node doesn't support dynamic pins.",
                    from_node,
                    from_pin
                );
            if let Some(ref input_pin) = created_dynamic_input {
                self.rollback_dynamic_input(&to_node, input_pin).await;
            }
            return;
        }

        if let Some((input_pin, input_rx, hint_tx)) = pending_input_pin_activation {
            if let Some(pin_mgmt_tx) = self.pin_management_txs.get(&to_node) {
                let msg = streamkit_core::pins::PinManagementMessage::AddedInputPin {
                    pin: input_pin,
                    channel: input_rx,
                    hint_tx,
                };

                if pin_mgmt_tx.send(msg).await.is_err() {
                    tracing::error!(
                        "Failed to send pin activation message to node '{}'. It may have stopped.",
                        to_node
                    );
                    if let Some(ref output_pin_name) = created_dynamic_output {
                        self.rollback_dynamic_output(&from_node, output_pin_name).await;
                    }
                    if let Some(ref input_pin_name) = created_dynamic_input {
                        self.rollback_dynamic_input(&to_node, input_pin_name).await;
                    }
                    return;
                }
            } else {
                tracing::error!(
                    "No pin management channel for node '{}' — cannot activate dynamic input pin. \
                     Rolling back connection.",
                    to_node
                );
                if let Some(ref output_pin_name) = created_dynamic_output {
                    self.rollback_dynamic_output(&from_node, output_pin_name).await;
                }
                if let Some(ref input_pin_name) = created_dynamic_input {
                    self.rollback_dynamic_input(&to_node, input_pin_name).await;
                }
                return;
            }
        }

        let resolved_type =
            if matches!(source_produces_type, streamkit_core::types::PacketType::Passthrough) {
                self.resolve_passthrough_type(&from_node, &from_pin)
            } else {
                source_produces_type
            };
        if let Some(pin_mgmt_tx) = self.pin_management_txs.get(&to_node) {
            let msg = streamkit_core::pins::PinManagementMessage::InputTypeResolved {
                pin_name: to_pin.clone(),
                packet_type: resolved_type,
            };
            if pin_mgmt_tx.send(msg).await.is_err() {
                tracing::warn!(
                    "Failed to send InputTypeResolved to '{}' for pin '{}' — node may have stopped",
                    to_node,
                    to_pin
                );
            }
        }

        // Deliver the hint receiver so the source can receive advisory
        // hints (e.g. preferred output size) from downstream.
        if let Some(hint_rx) = pending_hint_rx.take() {
            if let Some(pin_mgmt_tx) = self.pin_management_txs.get(&from_node) {
                tracing::info!(
                    "Delivering OutputHintChannel to source '{}' for pin '{}'",
                    from_node,
                    from_pin
                );
                let msg = streamkit_core::pins::PinManagementMessage::OutputHintChannel {
                    pin_name: from_pin.clone(),
                    hint_rx,
                };
                if pin_mgmt_tx.send(msg).await.is_err() {
                    tracing::warn!(
                        "Failed to send OutputHintChannel to '{}' for pin '{}' — node may have stopped",
                        from_node,
                        from_pin
                    );
                }
            }
        }

        let connection_id = crate::dynamic_messages::ConnectionId::new(
            from_node.clone(),
            from_pin.clone(),
            to_node.clone(),
            to_pin.clone(),
        );
        let msg = PinConfigMsg::AddConnection { id: connection_id, tx: dest_tx, mode };

        if config_tx.send(msg).await.is_err() {
            tracing::error!(
                "Failed to send configuration to Pin Distributor for '{}.{}'. It may have stopped.",
                from_node,
                from_pin
            );
        }

        self.connections.insert((to_node.clone(), to_pin.clone()), (from_node, from_pin));
    }

    /// Resolve a `Passthrough` type by tracing backward through the
    /// connection graph.  Returns the first non-Passthrough type found,
    /// or `PacketType::Any` if the chain is unresolved (max 50 hops to
    /// guard against cycles).
    pub(super) fn resolve_passthrough_type(
        &self,
        source_node: &str,
        source_pin: &str,
    ) -> streamkit_core::types::PacketType {
        use streamkit_core::types::PacketType;

        let mut current_node = source_node.to_string();
        let mut current_pin = source_pin.to_string();

        for _ in 0..50 {
            let produces = self
                .node_pin_metadata
                .get(&current_node)
                .and_then(|m| m.output_pins.iter().find(|p| p.name == current_pin))
                .map_or(PacketType::Any, |p| p.produces_type.clone());

            if !matches!(produces, PacketType::Passthrough) {
                return produces;
            }

            // Trace backward through input connections.
            let input_pins = self
                .node_pin_metadata
                .get(&current_node)
                .map(|m| m.input_pins.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default();

            let mut found_upstream = false;
            for input_pin in &input_pins {
                if let Some((upstream_node, upstream_pin)) =
                    self.connections.get(&(current_node.clone(), input_pin.clone()))
                {
                    current_node.clone_from(upstream_node);
                    current_pin.clone_from(upstream_pin);
                    found_upstream = true;
                    break;
                }
            }

            if !found_upstream {
                tracing::debug!(
                    "Cannot resolve Passthrough for {}.{}: no upstream connection",
                    current_node,
                    current_pin
                );
                return PacketType::Any;
            }
        }

        tracing::warn!(
            "Passthrough resolution exceeded 50 hops from {}.{} — possible cycle",
            source_node,
            source_pin
        );
        PacketType::Any
    }

    /// Roll back a dynamically created input pin when a subsequent step in
    /// `connect_nodes` fails. Removes the pin's channel from `node_inputs`,
    /// prunes the metadata entry, and notifies the destination node via
    /// `RemoveInputPin` so it can clean up its internal state (e.g. drop a
    /// `DynamicInputState` in `MoqPushNode` or abort a forwarder task in
    /// `MoqPeerNode`).
    async fn rollback_dynamic_input(&mut self, node_id: &str, pin_name: &str) {
        self.node_inputs.remove(&(node_id.to_string(), pin_name.to_string()));
        if let Some(meta) = self.node_pin_metadata.get_mut(node_id) {
            meta.input_pins.retain(|p| p.name != pin_name);
        }
        if let Some(pin_mgmt_tx) = self.pin_management_txs.get(node_id) {
            let msg = streamkit_core::pins::PinManagementMessage::RemoveInputPin {
                pin_name: pin_name.to_string(),
            };
            let _ = pin_mgmt_tx.send(msg).await;
        }
    }

    /// Roll back a dynamically created output pin: shut down its distributor,
    /// remove it from metadata, and tell the source node to drop it.
    async fn rollback_dynamic_output(&mut self, source_node: &str, output_pin_name: &str) {
        if let Some(cfg) =
            self.pin_distributors.remove(&(source_node.to_string(), output_pin_name.to_string()))
        {
            let _ = cfg.send(PinConfigMsg::Shutdown).await;
        }
        if let Some(meta) = self.node_pin_metadata.get_mut(source_node) {
            meta.output_pins.retain(|p| p.name != output_pin_name);
        }
        // The source node already received AddedOutputPin and holds a
        // data_tx pointing to the now-dead distributor.  Tell it to drop
        // the pin so it doesn't send into a closed channel.
        if let Some(src_pin_mgmt_tx) = self.pin_management_txs.get(source_node) {
            let _ = src_pin_mgmt_tx
                .send(streamkit_core::pins::PinManagementMessage::RemoveOutputPin {
                    pin_name: output_pin_name.to_string(),
                })
                .await;
        }
    }

    async fn disconnect_nodes(
        &mut self,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    ) {
        tracing::info!("Disconnecting {}.{} -> {}.{}", from_node, from_pin, to_node, to_pin);

        self.connections.remove(&(to_node.clone(), to_pin.clone()));

        let Some(config_tx) = self.pin_distributors.get(&(from_node.clone(), from_pin.clone()))
        else {
            tracing::warn!(
                "Cannot disconnect: Source output '{}.{}' distributor not found.",
                from_node,
                from_pin
            );
            return;
        };

        let connection_id = crate::dynamic_messages::ConnectionId::new(
            from_node.clone(),
            from_pin.clone(),
            to_node.clone(),
            to_pin.clone(),
        );
        let msg = PinConfigMsg::RemoveConnection { id: connection_id };

        if config_tx.send(msg).await.is_err() {
            tracing::warn!(
                "Failed to send configuration to Pin Distributor for '{}.{}'. It may have stopped.",
                from_node,
                from_pin
            );
        }
    }

    /// Discard deferred state referencing a node that is being removed or has
    /// failed, so a later node with the same id cannot resurrect it.
    fn prune_pending_for(&mut self, node_id: &str) {
        self.pending_connections.retain(|pc| pc.from_node != node_id && pc.to_node != node_id);
        self.pending_tunes.retain(|pt| pt.node_id != node_id);
    }

    /// Gracefully shut down a node and its associated actors.
    async fn shutdown_node(&mut self, node_id: &str) {
        if let Some(state) = self.node_states.get(node_id) {
            self.zero_state_gauge(node_id, state);
        }

        if let Some(live_node) = self.live_nodes.remove(node_id) {
            if live_node.control_tx.send(NodeControlMessage::Shutdown).await.is_ok() {
                let mut task_handle = live_node.task_handle;
                let shutdown_result =
                    tokio::time::timeout(std::time::Duration::from_secs(5), &mut task_handle).await;

                if shutdown_result.is_ok() {
                    tracing::debug!(node_id = %node_id, "Node shut down gracefully");
                } else {
                    tracing::warn!(
                        node_id = %node_id,
                        "Node did not shut down within 5s, aborting"
                    );
                    task_handle.abort();
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(1), task_handle).await;
                }
            } else {
                tracing::debug!(node_id = %node_id, "Node control channel closed, assuming exited");
            }
        }

        self.node_inputs.retain(|(name, _), _| name != node_id);

        let distributors_to_remove: Vec<(String, String)> =
            self.pin_distributors.keys().filter(|(name, _)| name == node_id).cloned().collect();

        for key in distributors_to_remove {
            if let Some(config_tx) = self.pin_distributors.remove(&key) {
                let _ = config_tx.send(PinConfigMsg::Shutdown).await;
            }
        }

        Arc::make_mut(&mut self.node_states).remove(node_id);
        Arc::make_mut(&mut self.node_stats).remove(node_id);
        Arc::make_mut(&mut self.node_view_data).remove(node_id);
        self.node_pin_metadata.remove(node_id);
        self.pin_management_txs.remove(node_id);
        self.dynamic_pin_nodes.remove(node_id);
        self.runtime_schemas.remove(node_id);
        self.connections.retain(|(to, _), (from, _)| to != node_id && from != node_id);
        self.prune_pending_for(node_id);
        self.node_kinds.remove(node_id);
        self.node_metric_labels.remove(node_id);
        self.nodes_active_gauge
            .record(self.live_nodes.len() as u64, &self.node_attributes.pipeline);
    }

    /// Handles a completed background node creation.
    ///
    /// On success: initializes the node, then flushes any pending connections
    /// whose endpoints are now both realized.
    /// On failure: transitions the node to `Failed`, drains pending connections
    /// referencing the failed node.
    async fn handle_node_created(&mut self, event: NodeCreatedEvent, channels: &NodeChannels) {
        let NodeCreatedEvent { node_id, kind, creation_id, params, result } = event;

        // Check whether this creation result is still the active one.
        // A mismatch means either:
        //   - RemoveNode was called while Creating (entry removed), or
        //   - Remove → re-Add happened and a newer creation superseded this one.
        // In both cases, discard the stale result.
        match self.active_creations.get(&node_id) {
            Some(&active_id) if active_id == creation_id => {
                // This is the current active creation — remove the tracking
                // entry and proceed with initialization.
                self.active_creations.remove(&node_id);
            },
            _ => {
                tracing::info!(
                    node = %node_id,
                    creation_id,
                    "Discarding stale/cancelled creation result"
                );
                return;
            },
        }

        match result {
            Ok(node) => {
                tracing::info!(node = %node_id, kind = %kind, "Node created successfully, initializing");

                if let Err(e) =
                    self.initialize_node(node, &node_id, &kind, creation_id, channels).await
                {
                    tracing::error!(
                        node_id = %node_id,
                        kind = %kind,
                        error = %e,
                        "Failed to initialize node after async creation"
                    );

                    self.broadcast_state_update(
                        &node_id,
                        NodeState::Failed { reason: e.to_string() },
                    );
                    self.node_kinds.remove(&node_id);
                    self.node_metric_labels.remove(&node_id);
                    self.prune_pending_for(&node_id);
                    return;
                }

                self.flush_pending_connections().await;
                self.flush_pending_tunes(&node_id).await;

                let notification = NodeAddedNotification { node_id: node_id.clone(), kind, params };
                self.node_added_subscribers
                    .retain(|subscriber| subscriber.send(notification.clone()).is_ok());
            },
            Err(e) => {
                tracing::error!(
                    node_id = %node_id,
                    kind = %kind,
                    error = %e,
                    "Background node creation failed"
                );

                self.broadcast_state_update(&node_id, NodeState::Failed { reason: e.to_string() });

                // Keep Failed in node_states so clients can observe it;
                // RemoveNode clears it.
                self.node_kinds.remove(&node_id);
                self.node_metric_labels.remove(&node_id);
                self.prune_pending_for(&node_id);
            },
        }
    }

    /// Build the label set for a node's state metrics: `[node_id, state]` plus
    /// the node's bounded pipeline attributes. Single chokepoint so every
    /// node-scoped metric (state gauge, state transitions) carries the same
    /// attributes as the packet counters. When the per-node cache is gone (e.g.
    /// a failed node whose labels were removed but whose state lingers until
    /// the user removes it), the attributes are rebuilt from the session set so
    /// the zeroing datapoint still matches the earlier attributed series.
    fn node_state_labels(&self, node_id: &str, state_name: &'static str) -> Vec<KeyValue> {
        self.node_metric_labels.get(node_id).map_or_else(
            || {
                let attrs = self.node_attributes.for_node(node_id);
                let mut labels = Vec::with_capacity(2 + attrs.len());
                labels.push(KeyValue::new("node_id", node_id.to_owned()));
                labels.push(KeyValue::new("state", state_name));
                labels.extend(attrs);
                labels
            },
            |c| {
                let mut labels = Vec::with_capacity(2 + c.attrs.len());
                labels.push(c.node_id_kv.clone());
                labels.push(KeyValue::new("state", state_name));
                labels.extend(c.attrs.iter().cloned());
                labels
            },
        )
    }

    /// Zero-out the gauge for a specific state (one-hot pattern helper).
    fn zero_state_gauge(&self, node_id: &str, state: &NodeState) {
        let state_name = Self::node_state_name(state);
        self.node_state_gauge.record(0, &self.node_state_labels(node_id, state_name));
    }

    /// Broadcast a state update to all subscribers. Reads the previous state
    /// before inserting so one-hot gauge zeroing is correct.
    fn broadcast_state_update(&mut self, node_id: &str, new_state: NodeState) {
        let state_name = Self::node_state_name(&new_state);
        self.node_state_transitions_counter.add(1, &self.node_state_labels(node_id, state_name));

        if let Some(prev_state) = self.node_states.get(node_id) {
            let prev_state_name = Self::node_state_name(prev_state);
            if prev_state_name != state_name {
                self.node_state_gauge.record(0, &self.node_state_labels(node_id, prev_state_name));
            }
        }

        // Insert the new state AFTER reading the previous one.
        Arc::make_mut(&mut self.node_states).insert(node_id.to_owned(), new_state.clone());

        self.node_state_gauge.record(1, &self.node_state_labels(node_id, state_name));

        let update = NodeStateUpdate::new(node_id.to_owned(), new_state);
        self.state_subscribers.retain(|subscriber| match subscriber.try_send(update.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let subscriber = subscriber.clone();
                let update = update.clone();
                tokio::spawn(async move {
                    let _ = subscriber.send(update).await;
                });
                true
            },
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Execute any pending connections whose both endpoints are now realized
    /// (i.e., present in `live_nodes`).
    async fn flush_pending_connections(&mut self) {
        let pending = std::mem::take(&mut self.pending_connections);
        let mut still_pending = Vec::new();

        for pc in pending {
            let from_realized = self.live_nodes.contains_key(&pc.from_node);
            let to_realized = self.live_nodes.contains_key(&pc.to_node);

            if from_realized && to_realized {
                tracing::info!(
                    "Replaying deferred connection {}.{} -> {}.{}",
                    pc.from_node,
                    pc.from_pin,
                    pc.to_node,
                    pc.to_pin
                );
                self.connect_nodes(pc.from_node, pc.from_pin, pc.to_node, pc.to_pin, pc.mode).await;
                self.check_and_activate_pipeline();
            } else {
                still_pending.push(pc);
            }
        }

        self.pending_connections = still_pending;
    }

    /// Replay any deferred `TuneNode` messages for a node that has just been
    /// initialized and is now present in `live_nodes`.
    async fn flush_pending_tunes(&mut self, node_id: &str) {
        let (for_node, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_tunes)
            .into_iter()
            .partition(|pt| pt.node_id == node_id);

        self.pending_tunes = rest;

        for pt in for_node {
            if let Some(node) = self.live_nodes.get(&pt.node_id) {
                tracing::info!(
                    node_id = %pt.node_id,
                    "Replaying deferred TuneNode message"
                );
                if node.control_tx.send(pt.message).await.is_err() {
                    tracing::warn!(
                        "Could not replay TuneNode for '{}': node may have shut down",
                        pt.node_id
                    );
                }
            }
        }
    }

    /// Returns `true` if the node is in `Creating` state (not yet in `live_nodes`).
    fn is_node_creating(&self, node_id: &str) -> bool {
        matches!(self.node_states.get(node_id), Some(NodeState::Creating))
    }

    /// Returns `true` to continue running, `false` on shutdown.
    #[allow(clippy::cognitive_complexity)]
    async fn handle_engine_control(&mut self, msg: EngineControlMessage) -> bool {
        match msg {
            EngineControlMessage::AddNode { node_id, kind, params } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "add_node")]);
                tracing::info!(name = %node_id, kind = %kind, "Adding node to graph (async)");

                // Defence-in-depth: the WS handler rejects duplicates
                // before they reach this actor; this guards non-WS callers.
                if self.node_states.contains_key(&node_id) {
                    tracing::error!(
                        node_id = %node_id,
                        kind = %kind,
                        "Cannot add node: a node with this ID already exists"
                    );
                    return true;
                }

                // Assign a unique creation ID so handle_node_created can
                // distinguish stale results from a previous Remove → re-Add
                // cycle.
                let creation_id = self.next_creation_id;
                self.next_creation_id += 1;
                self.active_creations.insert(node_id.clone(), creation_id);

                self.node_kinds.insert(node_id.clone(), kind.clone());
                let attrs = self.node_attributes.for_node(&node_id);
                let mut stats = vec![
                    KeyValue::new("node_id", node_id.clone()),
                    KeyValue::new("node_kind", kind.clone()),
                ];
                stats.extend(attrs.iter().cloned());
                self.node_metric_labels.insert(
                    node_id.clone(),
                    NodeMetricLabels {
                        stats,
                        node_id_kv: KeyValue::new("node_id", node_id.clone()),
                        attrs,
                    },
                );

                self.broadcast_state_update(&node_id, NodeState::Creating);

                // Spawn background creation: `create_node` may invoke FFI
                // that blocks for 10-20+ seconds (ONNX model loading).
                let registry = Arc::clone(&self.registry);
                let tx = self.node_created_tx.clone();
                let spawn_node_id = node_id;
                let spawn_kind = kind.clone();
                // Cloned: `create_node` borrows them while the owned
                // value travels via `NodeCreatedEvent` for the notification.
                let spawn_params = params.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let guard = match registry.read() {
                            Ok(g) => g,
                            Err(err) => {
                                return Err(StreamKitError::Runtime(format!(
                                    "Registry lock poisoned: {err}"
                                )));
                            },
                        };
                        guard.create_node(&spawn_kind, spawn_params.as_ref())
                    })
                    .await;

                    let result = match result {
                        Ok(inner) => inner,
                        Err(join_err) => Err(StreamKitError::Runtime(format!(
                            "Node creation task panicked: {join_err}"
                        ))),
                    };

                    let _ = tx
                        .send(NodeCreatedEvent {
                            node_id: spawn_node_id,
                            kind,
                            creation_id,
                            params,
                            result,
                        })
                        .await;
                });
            },
            EngineControlMessage::RemoveNode { node_id } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "remove_node")]);
                tracing::info!(name = %node_id, "Removing node from graph");

                if self.is_node_creating(&node_id) {
                    // Remove tracking so the background result is discarded.
                    tracing::info!(
                        node_id = %node_id,
                        "Node is still Creating — cancelling"
                    );
                    self.active_creations.remove(&node_id);
                    // Zero the gauge before removing state (mirrors shutdown_node).
                    self.zero_state_gauge(&node_id, &NodeState::Creating);
                    Arc::make_mut(&mut self.node_states).remove(&node_id);
                    self.node_kinds.remove(&node_id);
                    self.node_metric_labels.remove(&node_id);
                    self.prune_pending_for(&node_id);
                } else {
                    self.shutdown_node(&node_id).await;
                }
            },
            EngineControlMessage::Connect { from_node, from_pin, to_node, to_pin, mode } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "connect")]);

                // Both endpoints must at least exist in node_states
                // (Creating or fully initialized). If either is completely
                // unknown, the connection would be deferred forever.
                let from_exists = self.node_states.contains_key(&from_node);
                let to_exists = self.node_states.contains_key(&to_node);
                if !from_exists || !to_exists {
                    tracing::error!(
                        "Cannot connect {}.{} -> {}.{}: endpoint(s) not found \
                         (from_exists={}, to_exists={})",
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                        from_exists,
                        to_exists
                    );
                    return true;
                }

                // If either endpoint is still Creating, defer the connection.
                let from_creating = self.is_node_creating(&from_node);
                let to_creating = self.is_node_creating(&to_node);

                if from_creating || to_creating {
                    tracing::info!(
                        "Deferring connection {}.{} -> {}.{} (from_creating={}, to_creating={})",
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                        from_creating,
                        to_creating
                    );
                    self.pending_connections.push(PendingConnection {
                        from_node,
                        from_pin,
                        to_node,
                        to_pin,
                        mode,
                    });
                } else {
                    self.connect_nodes(from_node, from_pin, to_node, to_pin, mode).await;
                    self.check_and_activate_pipeline();
                }
            },
            EngineControlMessage::Disconnect { from_node, from_pin, to_node, to_pin } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "disconnect")]);

                // Also remove any matching deferred connection so it isn't
                // replayed later by `flush_pending_connections`.
                self.pending_connections.retain(|pc| {
                    !(pc.from_node == from_node
                        && pc.from_pin == from_pin
                        && pc.to_node == to_node
                        && pc.to_pin == to_pin)
                });

                self.disconnect_nodes(from_node, from_pin, to_node, to_pin).await;
            },
            EngineControlMessage::TuneNode { node_id, message } => {
                if let Some(node) = self.live_nodes.get(&node_id) {
                    if node.control_tx.send(message).await.is_err() {
                        tracing::warn!(
                            "Could not send control message to node '{}' as it may have shut down.",
                            node_id
                        );
                    }
                } else if self.is_node_creating(&node_id) {
                    tracing::info!("Deferring TuneNode for '{}': still in Creating state", node_id);
                    self.pending_tunes.push(PendingTune { node_id, message });
                } else {
                    tracing::warn!("Could not tune non-existent node '{}'", node_id);
                }
            },
            EngineControlMessage::Shutdown => {
                tracing::info!("Received shutdown signal, stopping all nodes");

                self.active_creations.clear();
                self.pending_connections.clear();
                self.pending_tunes.clear();

                // Close input channels so nodes blocked on recv() exit.
                self.node_inputs.clear();
                tracing::debug!("Closed all node input channels");

                for (_, config_tx) in self.pin_distributors.drain() {
                    drop(config_tx.try_send(PinConfigMsg::Shutdown));
                }
                tracing::debug!("Sent shutdown to all pin distributors");

                let mut shutdown_handles = Vec::new();
                for (node_id, live_node) in self.live_nodes.drain() {
                    match live_node.control_tx.try_send(NodeControlMessage::Shutdown) {
                        Ok(()) => {
                            tracing::debug!(node_id = %node_id, "Sent shutdown signal to node");
                        },
                        Err(_) => {
                            tracing::debug!(node_id = %node_id, "Node control channel full or closed");
                        },
                    }
                    shutdown_handles.push((node_id, live_node.task_handle));
                }

                let shutdown_futures = shutdown_handles
                    .into_iter()
                    .map(|(node_id, handle)| async move {
                        let mut handle = handle;
                        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut handle)
                            .await
                        {
                            Ok(Ok(Ok(()))) => {
                                tracing::debug!(node_id = %node_id, "Node shut down gracefully");
                            }
                            Ok(Ok(Err(e))) => {
                                tracing::error!(node_id = %node_id, error = ?e, "Node returned error during shutdown");
                            }
                            Ok(Err(e)) => {
                                tracing::error!(node_id = %node_id, error = %e, "Node task panicked during shutdown");
                            }
                            Err(_) => {
                                tracing::warn!(
                                    node_id = %node_id,
                                    "Node did not shut down within 2s, this indicates a bug (node not checking control_rx or output send errors)"
                                );
                                handle.abort();
                                let _ = tokio::time::timeout(
                                    std::time::Duration::from_secs(1),
                                    handle,
                                )
                                .await;
                            }
                        }
                    });

                futures::future::join_all(shutdown_futures).await;

                let zero_labels: Vec<_> = self
                    .node_states
                    .iter()
                    .map(|(node_id, state)| {
                        self.node_state_labels(node_id, Self::node_state_name(state))
                    })
                    .collect();
                for labels in &zero_labels {
                    self.node_state_gauge.record(0, labels);
                }
                Arc::make_mut(&mut self.node_states).clear();
                Arc::make_mut(&mut self.node_stats).clear();
                Arc::make_mut(&mut self.node_view_data).clear();
                self.nodes_active_gauge.record(0, &self.node_attributes.pipeline);

                tracing::info!("All nodes shut down successfully");
                return false; // Signal to shut down the engine
            },
        }
        true // Continue running
    }
}
