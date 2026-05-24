// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Unit tests for the engine crate.

#[cfg(feature = "dynamic")]
mod async_node_creation;
#[cfg(feature = "dynamic")]
mod connection_types;
#[cfg(feature = "dynamic")]
mod dynamic_actor_edges;
#[cfg(feature = "dynamic")]
mod dynamic_actor_handlers;
#[cfg(feature = "dynamic")]
mod dynamic_actor_runtime_schemas;
#[cfg(feature = "dynamic")]
mod dynamic_actor_update_filters;
#[cfg(feature = "dynamic")]
mod dynamic_handle;
#[cfg(feature = "dynamic")]
mod dynamic_initialize;
mod engine_construction;
mod graph_builder;
mod oneshot;
mod oneshot_linear;
#[cfg(feature = "dynamic")]
mod pin_distributor;
#[cfg(feature = "dynamic")]
mod pipeline_activation;
#[cfg(feature = "dynamic")]
mod upstream_hints;

/// Construct a minimal [`DynamicEngine`] for direct-construction tests,
/// avoiding field-list drift across callers.
#[cfg(feature = "dynamic")]
pub fn create_test_engine() -> crate::dynamic_actor::DynamicEngine {
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    let (control_tx, control_rx) = mpsc::channel(32);
    let (query_tx, query_rx) = mpsc::channel(32);
    let engine_control_tx = control_tx.clone();
    drop(control_tx);
    drop(query_tx);

    let (nc_tx, nc_rx) = mpsc::channel(32);

    let meter = opentelemetry::global::meter("test");
    crate::dynamic_actor::DynamicEngine {
        registry: std::sync::Arc::new(std::sync::RwLock::new(
            streamkit_core::registry::NodeRegistry::new(),
        )),
        control_rx,
        query_rx,
        live_nodes: HashMap::new(),
        node_inputs: HashMap::new(),
        pin_distributors: HashMap::new(),
        pin_management_txs: HashMap::new(),
        dynamic_pin_nodes: std::collections::HashSet::new(),
        node_pin_metadata: HashMap::new(),
        connections: HashMap::new(),
        node_kinds: HashMap::new(),
        node_metric_labels: HashMap::new(),
        batch_size: 32,
        session_id: None,
        audio_pool: std::sync::Arc::new(streamkit_core::FramePool::<f32>::audio_default()),
        video_pool: std::sync::Arc::new(streamkit_core::FramePool::<u8>::video_default()),
        node_input_capacity: 128,
        pin_distributor_capacity: 64,
        node_states: std::sync::Arc::new(HashMap::new()),
        state_subscribers: Vec::new(),
        node_stats: std::sync::Arc::new(HashMap::new()),
        stats_subscribers: Vec::new(),
        telemetry_subscribers: Vec::new(),
        node_view_data: std::sync::Arc::new(HashMap::new()),
        view_data_subscribers: Vec::new(),
        nodes_active_gauge: meter.u64_gauge("test.nodes").build(),
        node_state_transitions_counter: meter.u64_counter("test.transitions").build(),
        engine_operations_counter: meter.u64_counter("test.operations").build(),
        node_packets_received_counter: meter.u64_counter("test.received").build(),
        node_packets_sent_counter: meter.u64_counter("test.sent").build(),
        node_packets_discarded_counter: meter.u64_counter("test.discarded").build(),
        node_packets_errored_counter: meter.u64_counter("test.errored").build(),
        node_state_gauge: meter.u64_gauge("test.state").build(),
        runtime_schemas: HashMap::new(),
        runtime_schema_subscribers: Vec::new(),
        node_added_subscribers: Vec::new(),
        engine_control_tx,
        node_created_tx: nc_tx,
        node_created_rx: nc_rx,
        pending_connections: Vec::new(),
        pending_tunes: Vec::new(),
        next_creation_id: 0,
        active_creations: std::collections::HashMap::new(),
    }
}
