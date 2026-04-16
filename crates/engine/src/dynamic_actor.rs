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
    dynamic_messages::{PinConfigMsg, QueryMessage, RuntimeSchemaUpdate},
    dynamic_pin_distributor::PinDistributorActor,
    graph_builder,
};
use opentelemetry::KeyValue;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use streamkit_core::control::{EngineControlMessage, NodeControlMessage};
use streamkit_core::error::StreamKitError;
use streamkit_core::frame_pool::{AudioFramePool, VideoFramePool};
use streamkit_core::node::{InitContext, NodeContext, OutputRouting, OutputSender};
use streamkit_core::pins::PinUpdate;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::{NodeState, NodeStateUpdate};
use streamkit_core::stats::{NodeStats, NodeStatsUpdate};
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::view_data::NodeViewDataUpdate;
use streamkit_core::PinCardinality;
use tokio::sync::mpsc;
use tracing::Instrument;

/// Metadata about a node's pins, used for runtime type validation in dynamic pipelines.
#[derive(Debug, Clone)]
pub struct NodePinMetadata {
    pub input_pins: Vec<streamkit_core::InputPin>,
    pub output_pins: Vec<streamkit_core::OutputPin>,
}

/// Pre-built OTel metric labels for a node, allocated once on creation and
/// reused on every stats/state update to avoid per-update `String` allocations.
#[derive(Clone)]
pub(super) struct NodeMetricLabels {
    /// `[node_id, node_kind]` — used by stats counters.
    stats: [KeyValue; 2],
    /// Standalone `node_id` label — combined with a varying `state` label.
    node_id_kv: KeyValue,
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
                    // handle_stats_update is synchronous (no .await needed)
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
        }
    }

    /// Checks if all nodes in the pipeline are Ready or Running.
    /// If all nodes are ready, sends Start signal to nodes in Ready state.
    /// This ensures that source nodes don't start producing packets until the entire
    /// pipeline is initialized, preventing packet loss.
    ///
    /// Takes `&self` not `&mut self` because it only reads pipeline state and sends messages
    pub(crate) fn check_and_activate_pipeline(&self) {
        use tokio::sync::mpsc::error::TrySendError;

        // Skip if we have no nodes
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

        // Find nodes in Ready state.
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
            return; // No nodes waiting to be activated
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

        // Send Start message to all source nodes that are still Ready.
        // Avoid stalling the control-plane task on backpressure: try_send fast-path,
        // and fall back to a spawned async send if the channel is full.
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

    /// Handles a node state update by storing it and broadcasting to subscribers
    ///
    /// Takes by reference to avoid unnecessary clones when broadcasting to subscribers
    fn handle_state_update(&mut self, update: &NodeStateUpdate) {
        // Ignore state updates for nodes that have been removed
        // This prevents race conditions where a node sends a final state update
        // after shutdown_node() has already removed it from node_states
        if !self.live_nodes.contains_key(&update.node_id) {
            tracing::trace!(
                node = %update.node_id,
                state = ?update.state,
                "Ignoring state update for removed node"
            );
            return;
        }

        tracing::debug!(
            node = %update.node_id,
            state = ?update.state,
            "Node state updated"
        );

        // Record state transition metric
        let state_name = Self::node_state_name(&update.state);
        let node_id_kv = self.node_metric_labels.get(&update.node_id).map_or_else(
            || KeyValue::new("node_id", update.node_id.clone()),
            |c| c.node_id_kv.clone(),
        );
        self.node_state_transitions_counter
            .add(1, &[node_id_kv.clone(), KeyValue::new("state", state_name)]);

        // Record state gauge as a proper "one-hot" state indicator per node:
        // - Set the previous state's series to 0
        // - Set the new/current state's series to 1
        //
        // This keeps dashboards correct when nodes transition away from Running.
        let prev_state = self.node_states.get(&update.node_id);
        if let Some(prev_state) = prev_state {
            let prev_state_name = Self::node_state_name(prev_state);
            if prev_state_name != state_name {
                self.node_state_gauge
                    .record(0, &[node_id_kv.clone(), KeyValue::new("state", prev_state_name)]);
            }
        }
        self.node_state_gauge.record(1, &[node_id_kv, KeyValue::new("state", state_name)]);

        // Store the current state
        Arc::make_mut(&mut self.node_states).insert(update.node_id.clone(), update.state.clone());

        // Check if all nodes are Ready or Running - if so, activate Ready nodes
        // This prevents packet loss by ensuring all nodes are initialized before data flows
        self.check_and_activate_pipeline();

        // Broadcast to all subscribers
        self.state_subscribers.retain(|subscriber| {
            // Keep subscribers on transient backpressure (Full); remove only when Closed.
            //
            // For state updates we also try to deliver eventually: dropping a state transition
            // (e.g. Running -> Recovering) can leave clients showing a stale "healthy" status.
            match subscriber.try_send(update.clone()) {
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
            }
        });
    }

    /// Handles a telemetry event by broadcasting to subscribers.
    ///
    /// Unlike state/stats, telemetry events are not stored - they're purely streaming.
    /// Takes by reference to avoid unnecessary clones when broadcasting to subscribers.
    fn handle_telemetry_event(&mut self, event: &TelemetryEvent) {
        // Broadcast to all subscribers, removing disconnected ones
        self.telemetry_subscribers.retain(|subscriber| {
            // Keep subscribers on transient backpressure (Full); remove only when Closed.
            match subscriber.try_send(event.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }

    /// Handles a node view data update by storing it and broadcasting to subscribers.
    ///
    /// View data is best-effort (like stats): dropped updates are acceptable.
    fn handle_view_data_update(&mut self, update: &NodeViewDataUpdate) {
        // Ignore view data updates for nodes that have been removed
        if !self.live_nodes.contains_key(&update.node_id) {
            tracing::trace!(
                node = %update.node_id,
                "Ignoring view data update for removed node"
            );
            return;
        }

        // Store latest value
        Arc::make_mut(&mut self.node_view_data).insert(update.node_id.clone(), update.data.clone());

        // Broadcast to all subscribers
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

    /// Handles a node statistics update by storing it and broadcasting to subscribers
    ///
    /// Not async because all operations are synchronous (no .await calls)
    /// Takes by reference to avoid unnecessary clones when broadcasting to subscribers
    fn handle_stats_update(&mut self, update: &NodeStatsUpdate) {
        // Ignore stats updates for nodes that have been removed
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

        let labels = match self.node_metric_labels.get(&update.node_id) {
            Some(cached) => &cached.stats,
            None => return,
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

    /// Helper function to initialize a node and its I/O actors (Pin Distributors).
    ///
    /// Channel senders are bundled in `NodeChannels` to keep the signature
    /// under the clippy::too_many_arguments threshold.
    async fn initialize_node(
        &mut self,
        node: Box<dyn streamkit_core::ProcessorNode>,
        node_id: &str,
        kind: &str,
        channels: &NodeChannels,
    ) -> Result<(), StreamKitError> {
        let mut node = node;

        // Tier 1: Initialization-time discovery (dynamic pins, probing external resources, etc.)
        let init_ctx =
            InitContext { node_id: node_id.to_string(), state_tx: channels.state.clone() };
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

            // Notify subscribers so the UI can merge the schema immediately
            // rather than waiting for a manual pipeline re-fetch.
            let update = RuntimeSchemaUpdate { node_id: node_id.to_string(), schema };
            self.runtime_schema_subscribers
                .retain(|subscriber| subscriber.send(update.clone()).is_ok());
        }

        let (control_tx, control_rx) = mpsc::channel(CONTROL_CAPACITY);

        // 0. Capture pin metadata for runtime type validation
        let input_pins = node.input_pins();
        let output_pins = node.output_pins();
        self.node_pin_metadata.insert(
            node_id.to_string(),
            NodePinMetadata { input_pins: input_pins.clone(), output_pins: output_pins.clone() },
        );

        // 1. Setup Inputs
        let mut node_inputs_map = HashMap::new();
        for pin in input_pins {
            let (tx, rx) = mpsc::channel(self.node_input_capacity);
            // Store the Sender so the engine can provide it to upstream PinDistributors.
            self.node_inputs.insert((node_id.to_string(), pin.name.clone()), tx);
            node_inputs_map.insert(pin.name, rx);
        }

        // 2. Setup Outputs (Spawn Pin Distributors)
        let mut node_outputs_map = HashMap::new();
        for pin in output_pins {
            // Create channels for the PinDistributor
            let (data_tx, data_rx) = mpsc::channel(self.pin_distributor_capacity);
            let (config_tx, config_rx) = mpsc::channel(CONTROL_CAPACITY);

            // Spawn the PinDistributorActor
            let distributor =
                PinDistributorActor::new(data_rx, config_rx, node_id.to_string(), pin.name.clone());
            tokio::spawn(distributor.run());

            // Store the configuration sender in the engine state
            self.pin_distributors.insert((node_id.to_string(), pin.name.clone()), config_tx);

            // Provide the data sender to the node itself
            node_outputs_map.insert(pin.name.clone(), data_tx);
        }

        // 3. Initialize State and Stats
        // Use broadcast_state_update so the gauge transition (e.g.
        // Creating → Initializing) zeroes the previous gauge and sets
        // the new one atomically — no window where no gauge reads 1.
        self.broadcast_state_update(node_id, NodeState::Initializing);
        Arc::make_mut(&mut self.node_stats).insert(node_id.to_string(), NodeStats::default());

        // 4. Setup pin management channel.
        // Always created so the engine can deliver `InputTypeResolved` to
        // every node.  Dynamic-pin nodes additionally receive
        // `AddedInputPin` / `RemoveInputPin` etc. through the same channel.
        let (pin_management_tx, pin_management_rx) = mpsc::channel(CONTROL_CAPACITY);
        self.pin_management_txs.insert(node_id.to_string(), pin_management_tx);
        if node.supports_dynamic_pins() {
            self.dynamic_pin_nodes.insert(node_id.to_string());
        }

        // 5. Create NodeContext
        let context = NodeContext {
            inputs: node_inputs_map,
            // Dynamic pipelines wire connections after nodes are spawned, so
            // input types are not known at construction time.
            input_types: HashMap::new(),
            control_rx,
            // We use OutputRouting::Direct, pointing the node directly to its Pin Distributors
            output_sender: OutputSender::new(
                node_id.to_string(),
                OutputRouting::Direct(node_outputs_map),
            ),
            batch_size: self.batch_size,
            state_tx: channels.state.clone(),
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
        };

        // 5. Spawn Node
        let task_handle = tokio::spawn(node.run(context).instrument(tracing::info_span!(
            "node_run",
            session.id = %self.session_id.as_deref().unwrap_or("<unknown>"),
            node.name = %node_id,
            node.kind = %kind
        )));
        self.live_nodes
            .insert(node_id.to_string(), graph_builder::LiveNode { control_tx, task_handle });
        self.nodes_active_gauge.record(self.live_nodes.len() as u64, &[]);
        Ok(())
    }

    /// Validates type compatibility between source and destination pins.
    ///
    /// For dynamic pipelines, this provides runtime type checking to prevent
    /// incompatible connections. Passthrough types are allowed and will be
    /// resolved at runtime based on actual packet types.
    pub(crate) fn validate_connection_types(
        &self,
        from_node: &str,
        from_pin: &str,
        to_node: &str,
        to_pin: &str,
    ) -> Result<(), String> {
        fn is_dynamic_pin_match(prefix: &str, pin: &str) -> bool {
            if pin == prefix {
                return true;
            }
            pin.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('_'))
        }

        fn match_dynamic_pin<'a>(
            pins: &'a [streamkit_core::InputPin],
            pin: &str,
        ) -> Option<&'a streamkit_core::InputPin> {
            pins.iter().find(|p| {
                matches!(&p.cardinality, PinCardinality::Dynamic { prefix } if is_dynamic_pin_match(prefix, pin))
            })
        }

        fn match_dynamic_output_pin<'a>(
            pins: &'a [streamkit_core::OutputPin],
            pin: &str,
        ) -> Option<&'a streamkit_core::OutputPin> {
            pins.iter().find(|p| {
                matches!(&p.cardinality, PinCardinality::Dynamic { prefix } if is_dynamic_pin_match(prefix, pin))
            })
        }

        // Get source node metadata
        let source_metadata = self
            .node_pin_metadata
            .get(from_node)
            .ok_or_else(|| format!("Source node '{from_node}' not found"))?;

        // Get destination node metadata
        let dest_metadata = self
            .node_pin_metadata
            .get(to_node)
            .ok_or_else(|| format!("Destination node '{to_node}' not found"))?;

        // Find source output pin (exact match or dynamic pin family template)
        let source_pin = source_metadata
            .output_pins
            .iter()
            .find(|p| p.name == from_pin)
            .or_else(|| match_dynamic_output_pin(&source_metadata.output_pins, from_pin));
        let Some(source_pin) = source_pin else {
            // If the source pin is not found but the node supports dynamic pins,
            // allow the connection — the output pin will be created on-demand in
            // connect_nodes via RequestAddOutputPin.
            //
            // NOTE: this skips destination-pin validation too.  When both nodes
            // support dynamic pins and neither pin exists yet, no compile-time
            // type checking occurs — mismatches will only surface at runtime
            // (or via the post-creation check in connect_nodes).
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

        // Find destination input pin (exact match or dynamic pin family template).
        //
        // For nodes that support dynamic pins, we allow connecting to pins that don't exist yet
        // (they'll be created on-demand in connect_nodes). If we can't find a template pin to
        // validate against, fall back to permissive validation for dynamic-pin nodes.
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

        // Special handling for Passthrough types in dynamic pipelines
        if matches!(source_pin.produces_type, streamkit_core::types::PacketType::Passthrough) {
            tracing::debug!(
                "Source pin {}.{} uses Passthrough - type will be resolved at runtime",
                from_node,
                from_pin
            );
            return Ok(());
        }

        // Check if destination accepts Any type
        if dest_pin
            .accepts_types
            .iter()
            .any(|t| matches!(t, streamkit_core::types::PacketType::Any))
        {
            return Ok(());
        }

        // Check if destination accepts Passthrough
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

        // Use the existing can_connect_any function for validation
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

        // 0. Validate type compatibility before making the connection
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

        // 1. Find the destination input Sender
        // If the pin doesn't exist and the node supports dynamic pins, create it first.
        // Track whether we dynamically created the input pin so we can roll it
        // back if step 2 (output pin creation) fails.
        //
        // NOTE: We defer sending AddedInputPin until after the source output
        // pin is resolved (step 2) so that AddedInputPin arrives before
        // InputTypeResolved — the node needs the channel ready before it
        // receives type info.  The deferred state is stored in
        // `pending_input_pin_activation`.
        let mut created_dynamic_input: Option<String> = None;
        let mut pending_input_pin_activation: Option<(
            streamkit_core::InputPin,
            mpsc::Receiver<streamkit_core::types::Packet>,
            Option<mpsc::Sender<streamkit_core::UpstreamHint>>,
        )> = None;
        // Hint receiver to deliver to the source node after connection is
        // established.  Created for both pre-existing and dynamic pins.
        // (Not initialised here — every branch below either assigns it or
        // returns early, so the compiler can verify exhaustiveness.)
        let mut pending_hint_rx: Option<mpsc::Receiver<streamkit_core::UpstreamHint>>;
        let dest_tx = if let Some(tx) = self.node_inputs.get(&(to_node.clone(), to_pin.clone())) {
            // Pre-existing pin — create a hint channel so the destination
            // can send advisory hints (e.g. preferred output size) upstream.
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
            // Node supports dynamic pins - create the pin on-demand
            tracing::info!(
                "Dynamically creating input pin '{}.{}' for connection",
                to_node,
                to_pin
            );

            // Request pin creation
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

            // Wait for the pin to be created (with timeout to avoid blocking
            // the engine indefinitely if the node is unresponsive).
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

            // Create the channel for this new pin
            let (tx, rx) = mpsc::channel(self.node_input_capacity);
            self.node_inputs.insert((to_node.clone(), pin.name.clone()), tx.clone());

            // Create a parallel hint channel (downstream → upstream) so the
            // destination node can send advisory hints back to the source.
            let (hint_tx, hint_rx) = mpsc::channel::<streamkit_core::UpstreamHint>(1);
            pending_hint_rx = Some(hint_rx);

            // Update our pin metadata so future validations can resolve this pin by name.
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

        // 2. Find the source Pin Distributor configuration Sender
        // If the pin doesn't exist and the node supports dynamic pins, create it first.
        // Also resolve the upstream `produces_type` so we can include it in
        // the deferred AddedInputPin message (step 2b).
        // Track whether we dynamically created the output pin so we can roll
        // it back if step 2b fails.
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
            // Node supports dynamic pins — create the output pin on-demand
            tracing::info!(
                "Dynamically creating output pin '{}.{}' for connection",
                from_node,
                from_pin
            );

            // Request pin creation from the node
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

            // Wait for the node to respond with the pin definition (with
            // timeout to avoid blocking the engine indefinitely).
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

            // Create channels for the PinDistributor
            let (data_tx, data_rx) = mpsc::channel(self.pin_distributor_capacity);
            let (cfg_tx, cfg_rx) = mpsc::channel(CONTROL_CAPACITY);

            // Spawn the PinDistributorActor
            let distributor =
                PinDistributorActor::new(data_rx, cfg_rx, from_node.clone(), pin.name.clone());
            tokio::spawn(distributor.run());

            // Store the configuration sender in the engine state
            self.pin_distributors.insert((from_node.clone(), pin.name.clone()), cfg_tx.clone());

            // Update pin metadata so future validations can resolve this pin by name
            let meta = self.node_pin_metadata.entry(from_node.clone()).or_insert_with(|| {
                NodePinMetadata { input_pins: Vec::new(), output_pins: Vec::new() }
            });
            if !meta.output_pins.iter().any(|p| p.name == pin.name) {
                meta.output_pins.push(pin.clone());
            }

            // Now that we have the concrete pin definition, validate type
            // compatibility against the destination.  This catches YAML typos
            // like `moq_peer.nonexistent/garbage` that were previously allowed
            // through the early-return in validate_connection_types.
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
                        // Clean up the distributor actor and metadata that were
                        // just created — leaving them would leak an orphaned
                        // task and stale metadata for the session.
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

            // Notify the node that the output pin is ready with its channel
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
                // Clean up the distributor and metadata — the node never
                // received AddedOutputPin so nothing will produce into this pin.
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

        // 2b. Send the deferred AddedInputPin now that the source pin is resolved.
        // This only fires when a dynamic input pin was created in step 1.
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
                // pin_management_txs should always contain an entry (created
                // in add_node for every node).  If missing, the node was
                // removed between step 1 and step 2b.
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

        // 2c. Deliver InputTypeResolved to the destination node.
        // This is the single, uniform mechanism for all nodes (both
        // dynamic-pin and pre-existing-pin) to learn the upstream type.
        //
        // If the source produces Passthrough, resolve it by tracing
        // backward through the connection graph to find a concrete type.
        //
        // NOTE: If step 3 (AddConnection) fails below, the node will have
        // received type info for a connection that never materialized.
        // This is low-severity — the worst case is a pin that never
        // receives data, and step 3 failure is rare (PinDistributor
        // would need to have stopped between creation and this point).
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

        // 2d. Deliver the hint receiver to the source node so it can
        // receive advisory hints (e.g. preferred output size) from the
        // downstream consumer.
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

        // 3. Send configuration message
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

        // Record the connection for Passthrough type resolution.
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

            // Trace backward: find any connection feeding this node's input
            // pins, then look up that upstream output's type.
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
                // No upstream connection found — can't resolve further.
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

    /// Helper function to disconnect nodes.
    async fn disconnect_nodes(
        &mut self,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    ) {
        tracing::info!("Disconnecting {}.{} -> {}.{}", from_node, from_pin, to_node, to_pin);

        // Remove from connection tracking.
        self.connections.remove(&(to_node.clone(), to_pin.clone()));

        // 1. Find the source Pin Distributor configuration Sender
        // Use let...else for cleaner early return pattern
        let Some(config_tx) = self.pin_distributors.get(&(from_node.clone(), from_pin.clone()))
        else {
            // If it doesn't exist, it's already disconnected or never existed.
            tracing::warn!(
                "Cannot disconnect: Source output '{}.{}' distributor not found.",
                from_node,
                from_pin
            );
            return;
        };

        // 2. Send configuration message
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

    /// Helper function to gracefully shut down a node and its associated actors.
    async fn shutdown_node(&mut self, node_id: &str) {
        if let Some(state) = self.node_states.get(node_id) {
            self.zero_state_gauge(node_id, state);
        }

        // 1. Stop the node task gracefully
        if let Some(live_node) = self.live_nodes.remove(node_id) {
            // First, try graceful shutdown by sending a control message
            if live_node.control_tx.send(NodeControlMessage::Shutdown).await.is_ok() {
                let mut task_handle = live_node.task_handle;
                // Wait for graceful shutdown with timeout
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
                // Control channel closed, node may have already exited
                tracing::debug!(node_id = %node_id, "Node control channel closed, assuming exited");
            }
        }

        // 2. Clean up inputs
        self.node_inputs.retain(|(name, _), _| name != node_id);

        // 3. Stop and clean up Pin Distributors
        let distributors_to_remove: Vec<(String, String)> =
            self.pin_distributors.keys().filter(|(name, _)| name == node_id).cloned().collect();

        for key in distributors_to_remove {
            if let Some(config_tx) = self.pin_distributors.remove(&key) {
                // Send shutdown signal. The actor will exit gracefully after draining.
                let _ = config_tx.send(PinConfigMsg::Shutdown).await;
            }
        }

        // 4. Clean up Control Plane state
        Arc::make_mut(&mut self.node_states).remove(node_id);
        Arc::make_mut(&mut self.node_stats).remove(node_id);
        Arc::make_mut(&mut self.node_view_data).remove(node_id);
        self.node_pin_metadata.remove(node_id);
        self.pin_management_txs.remove(node_id);
        self.dynamic_pin_nodes.remove(node_id);
        self.runtime_schemas.remove(node_id);
        self.connections.retain(|(to, _), (from, _)| to != node_id && from != node_id);
        self.node_kinds.remove(node_id);
        self.node_metric_labels.remove(node_id);
        self.nodes_active_gauge.record(self.live_nodes.len() as u64, &[]);
    }

    /// Handles a completed background node creation.
    ///
    /// On success: initializes the node, then flushes any pending connections
    /// whose endpoints are now both realized.
    /// On failure: transitions the node to `Failed`, drains pending connections
    /// referencing the failed node.
    async fn handle_node_created(&mut self, event: NodeCreatedEvent, channels: &NodeChannels) {
        let NodeCreatedEvent { node_id, kind, creation_id, result } = event;

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

                // initialize_node calls broadcast_state_update(Initializing)
                // which reads Creating as the previous state and zeroes its
                // gauge before setting Initializing to 1 — no gap.
                if let Err(e) = self.initialize_node(node, &node_id, &kind, channels).await {
                    tracing::error!(
                        node_id = %node_id,
                        kind = %kind,
                        error = %e,
                        "Failed to initialize node after async creation"
                    );

                    // Broadcast Failed (reads prev state before inserting).
                    self.broadcast_state_update(
                        &node_id,
                        NodeState::Failed { reason: e.to_string() },
                    );

                    // Clean up node_kinds (mirrors RemoveNode-while-Creating).
                    self.node_kinds.remove(&node_id);
                    self.node_metric_labels.remove(&node_id);

                    // Drain pending connections and tunes referencing this node.
                    self.pending_connections
                        .retain(|pc| pc.from_node != node_id && pc.to_node != node_id);
                    self.pending_tunes.retain(|pt| pt.node_id != node_id);
                    return;
                }

                // Flush pending connections where both endpoints are now realized.
                self.flush_pending_connections().await;

                // Replay any TuneNode messages that arrived while Creating.
                self.flush_pending_tunes(&node_id).await;
            },
            Err(e) => {
                tracing::error!(
                    node_id = %node_id,
                    kind = %kind,
                    error = %e,
                    "Background node creation failed"
                );

                // Broadcast Failed (reads prev state before inserting).
                self.broadcast_state_update(&node_id, NodeState::Failed { reason: e.to_string() });

                // Clean up node_kinds (mirrors RemoveNode-while-Creating).
                self.node_kinds.remove(&node_id);

                // Drain pending connections and tunes referencing this node.
                self.pending_connections
                    .retain(|pc| pc.from_node != node_id && pc.to_node != node_id);
                self.pending_tunes.retain(|pt| pt.node_id != node_id);
            },
        }
    }

    /// Zero-out the gauge for a specific state (one-hot pattern helper).
    fn zero_state_gauge(&self, node_id: &str, state: &NodeState) {
        let state_name = Self::node_state_name(state);
        let node_id_kv = self
            .node_metric_labels
            .get(node_id)
            .map_or_else(|| KeyValue::new("node_id", node_id.to_owned()), |c| c.node_id_kv.clone());
        self.node_state_gauge.record(0, &[node_id_kv, KeyValue::new("state", state_name)]);
    }

    /// Broadcast a state update to all subscribers (used when the actor itself
    /// synthesizes a state transition, e.g. `Creating → Failed`).
    ///
    /// Reads the previous state from `node_states` **before** inserting the
    /// new one, so the one-hot gauge zeroing is correct.
    fn broadcast_state_update(&mut self, node_id: &str, new_state: NodeState) {
        let state_name = Self::node_state_name(&new_state);
        let node_id_kv = self
            .node_metric_labels
            .get(node_id)
            .map_or_else(|| KeyValue::new("node_id", node_id.to_owned()), |c| c.node_id_kv.clone());
        self.node_state_transitions_counter
            .add(1, &[node_id_kv.clone(), KeyValue::new("state", state_name)]);

        // Zero-out the previous state's gauge series (one-hot pattern),
        // mirroring the logic in `handle_state_update`.
        if let Some(prev_state) = self.node_states.get(node_id) {
            let prev_state_name = Self::node_state_name(prev_state);
            if prev_state_name != state_name {
                self.node_state_gauge
                    .record(0, &[node_id_kv.clone(), KeyValue::new("state", prev_state_name)]);
            }
        }

        // Insert the new state AFTER reading the previous one.
        Arc::make_mut(&mut self.node_states).insert(node_id.to_owned(), new_state.clone());

        self.node_state_gauge.record(1, &[node_id_kv, KeyValue::new("state", state_name)]);

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
        // Drain the vec, keeping connections that still have unrealized endpoints.
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

    /// Handles a single control message sent to the engine.
    /// Returns true if the engine should continue running, false if it should shut down.
    #[allow(clippy::cognitive_complexity)]
    async fn handle_engine_control(&mut self, msg: EngineControlMessage) -> bool {
        match msg {
            EngineControlMessage::AddNode { node_id, kind, params } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "add_node")]);
                tracing::info!(name = %node_id, kind = %kind, "Adding node to graph (async)");

                // Reject duplicate node IDs — the node already exists in
                // node_states (either Creating or fully initialized).
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

                // Record kind immediately so the actor loop continues
                // processing the next message without blocking.
                self.node_kinds.insert(node_id.clone(), kind.clone());
                self.node_metric_labels.insert(
                    node_id.clone(),
                    NodeMetricLabels {
                        stats: [
                            KeyValue::new("node_id", node_id.clone()),
                            KeyValue::new("node_kind", kind.clone()),
                        ],
                        node_id_kv: KeyValue::new("node_id", node_id.clone()),
                    },
                );

                // Insert Creating state and broadcast to subscribers.
                // broadcast_state_update handles gauge + node_states insert.
                self.broadcast_state_update(&node_id, NodeState::Creating);

                // Spawn background creation: `create_node` may invoke FFI
                // that blocks for 10-20+ seconds (ONNX model loading).
                let registry = Arc::clone(&self.registry);
                let tx = self.node_created_tx.clone();
                let spawn_node_id = node_id;
                let spawn_kind = kind.clone();
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
                        guard.create_node(&spawn_kind, params.as_ref())
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
                            result,
                        })
                        .await;
                });
            },
            EngineControlMessage::RemoveNode { node_id } => {
                self.engine_operations_counter.add(1, &[KeyValue::new("operation", "remove_node")]);
                tracing::info!(name = %node_id, "Removing node from graph");

                if self.is_node_creating(&node_id) {
                    // Node is still being created in the background.
                    // Remove the active_creations entry so that when the
                    // background task completes, handle_node_created finds
                    // no matching entry and discards the result.
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
                    // Drain pending connections and tunes referencing this node.
                    self.pending_connections
                        .retain(|pc| pc.from_node != node_id && pc.to_node != node_id);
                    self.pending_tunes.retain(|pt| pt.node_id != node_id);
                } else {
                    // Normal shutdown for a fully initialized node.
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
                    // Both endpoints are realized — connect immediately.
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

                // Delegate disconnection logic for realized connections.
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

                // Step 0: Clean up nodes still in Creating state.
                // Clear all active_creations so any background results
                // that arrive after shutdown are discarded.
                self.active_creations.clear();
                self.pending_connections.clear();
                self.pending_tunes.clear();

                // Step 1: Close all input channels so nodes blocked on recv() will exit
                // This ensures nodes that don't check control_rx will still shut down
                self.node_inputs.clear();
                tracing::debug!("Closed all node input channels");

                // Step 2: Send shutdown to all Pin Distributors immediately (non-blocking)
                // Using try_send to avoid blocking if channels are full
                for (_, config_tx) in self.pin_distributors.drain() {
                    // Ignore errors - distributor might already be shutting down
                    // Use drop to explicitly ignore Result (cleaner than let _)
                    drop(config_tx.try_send(PinConfigMsg::Shutdown));
                }
                tracing::debug!("Sent shutdown to all pin distributors");

                // Step 3: Send shutdown messages to ALL nodes immediately (non-blocking broadcast)
                let mut shutdown_handles = Vec::new();
                for (node_id, live_node) in self.live_nodes.drain() {
                    // Use try_send for immediate, non-blocking broadcast
                    // If channel is full or closed, that's fine - node is busy or already shutting down
                    match live_node.control_tx.try_send(NodeControlMessage::Shutdown) {
                        // Use () instead of _ for unit pattern to be explicit
                        Ok(()) => {
                            tracing::debug!(node_id = %node_id, "Sent shutdown signal to node");
                        },
                        Err(_) => {
                            tracing::debug!(node_id = %node_id, "Node control channel full or closed");
                        },
                    }
                    // Store the handle regardless - we want to wait for the node
                    shutdown_handles.push((node_id, live_node.task_handle));
                }

                // Step 4: Wait for nodes to exit gracefully (with timeout), then force-abort stragglers
                // Graceful shutdown helps surface issues like nodes not checking control_rx
                let shutdown_futures = shutdown_handles
                    .into_iter()
                    .map(|(node_id, handle)| async move {
                        let mut handle = handle;
                        // Wait up to 2 seconds for graceful shutdown
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
                                // Timeout - node didn't exit gracefully
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

                // Wait for all nodes to complete or timeout
                futures::future::join_all(shutdown_futures).await;

                // Step 5: Clean up remaining state
                for (node_id, state) in self.node_states.as_ref() {
                    let node_id_kv = self.node_metric_labels.get(node_id.as_str()).map_or_else(
                        || KeyValue::new("node_id", node_id.clone()),
                        |c| c.node_id_kv.clone(),
                    );
                    self.node_state_gauge.record(
                        0,
                        &[node_id_kv, KeyValue::new("state", Self::node_state_name(state))],
                    );
                }
                Arc::make_mut(&mut self.node_states).clear();
                Arc::make_mut(&mut self.node_stats).clear();
                Arc::make_mut(&mut self.node_view_data).clear();
                self.nodes_active_gauge.record(0, &[]);

                tracing::info!("All nodes shut down successfully");
                return false; // Signal to shut down the engine
            },
        }
        true // Continue running
    }
}
