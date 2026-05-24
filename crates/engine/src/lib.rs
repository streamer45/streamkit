// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pipeline execution engine — oneshot and dynamic modes.

use opentelemetry::global;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
pub use streamkit_api::Connection;
use streamkit_core::constraints::GlobalNodeConstraints;
use streamkit_core::registry::NodeRegistry;
use tokio::sync::mpsc;

pub mod constants;
pub mod graph_builder;
pub mod oneshot;

#[cfg(feature = "dynamic")]
mod dynamic_actor;
#[cfg(feature = "dynamic")]
mod dynamic_config;
#[cfg(feature = "dynamic")]
mod dynamic_handle;
#[cfg(feature = "dynamic")]
mod dynamic_messages;
#[cfg(feature = "dynamic")]
mod dynamic_pin_distributor;

#[cfg(feature = "dynamic")]
pub use dynamic_config::DynamicEngineConfig;
#[cfg(feature = "dynamic")]
pub use dynamic_handle::DynamicEngineHandle;
#[cfg(feature = "dynamic")]
pub use dynamic_messages::RuntimeSchemaUpdate;
pub use oneshot::{OneshotEngineConfig, OneshotInput, OneshotPipelineResult};

#[cfg(feature = "dynamic")]
use constants::{
    DEFAULT_CONTROL_CAPACITY, DEFAULT_ENGINE_CONTROL_CAPACITY, DEFAULT_ENGINE_QUERY_CAPACITY,
    DEFAULT_NODE_INPUT_CAPACITY, DEFAULT_PIN_DISTRIBUTOR_CAPACITY,
};
#[cfg(feature = "dynamic")]
use dynamic_actor::DynamicEngine;

/// Unified entry point for running stateless or dynamic pipelines.
pub struct Engine {
    pub registry: Arc<RwLock<NodeRegistry>>,
    pub(crate) audio_pool: Arc<streamkit_core::AudioFramePool>,
    pub(crate) video_pool: Arc<streamkit_core::VideoFramePool>,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::build(true, None, None, &GlobalNodeConstraints::new())
    }

    pub fn with_plugin_dir(plugin_dir: Option<std::path::PathBuf>) -> Self {
        Self::build(true, plugin_dir, None, &GlobalNodeConstraints::new())
    }

    pub fn without_plugins() -> Self {
        Self::build(false, None, None, &GlobalNodeConstraints::new())
    }

    pub fn with_resource_manager(resource_manager: Arc<streamkit_core::ResourceManager>) -> Self {
        Self::build(false, None, Some(resource_manager), &GlobalNodeConstraints::new())
    }

    pub fn with_resource_manager_and_constraints(
        resource_manager: Arc<streamkit_core::ResourceManager>,
        constraints: &GlobalNodeConstraints,
    ) -> Self {
        Self::build(false, None, Some(resource_manager), constraints)
    }

    fn build(
        load_plugins: bool,
        plugin_dir: Option<std::path::PathBuf>,
        resource_manager: Option<Arc<streamkit_core::ResourceManager>>,
        constraints: &GlobalNodeConstraints,
    ) -> Self {
        let mut registry =
            resource_manager.map_or_else(NodeRegistry::new, NodeRegistry::with_resource_manager);

        streamkit_nodes::register_nodes(&mut registry, constraints);

        if load_plugins {
            #[cfg(feature = "plugins")]
            Self::load_plugins(&mut registry, plugin_dir);
        }

        Self {
            registry: Arc::new(RwLock::new(registry)),
            audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
            video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
        }
    }

    #[cfg(feature = "plugins")]
    fn load_plugins(registry: &mut NodeRegistry, plugin_dir: Option<std::path::PathBuf>) {
        use std::path::PathBuf;

        let dir = plugin_dir.unwrap_or_else(|| PathBuf::from("./plugins"));

        let config = streamkit_plugin_wasm::PluginRuntimeConfig::default();
        let runtime = match streamkit_plugin_wasm::PluginRuntime::new(config) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create plugin runtime");
                return;
            },
        };

        let plugins = runtime.load_plugins_from_directory(&dir);

        if plugins.is_empty() {
            tracing::info!(?dir, "No plugins found in directory");
        } else {
            tracing::info!(count = plugins.len(), ?dir, "Loading WASM plugins");
            streamkit_plugin_wasm::register_plugins(registry, plugins);
        }
    }

    #[cfg(feature = "dynamic")]
    pub fn start_dynamic_actor(&self, config: DynamicEngineConfig) -> DynamicEngineHandle {
        let (control_tx, control_rx) = mpsc::channel(DEFAULT_ENGINE_CONTROL_CAPACITY);
        let engine_control_tx = control_tx.clone();
        let (query_tx, query_rx) = mpsc::channel(DEFAULT_ENGINE_QUERY_CAPACITY);

        let node_input_capacity = config.node_input_capacity.unwrap_or(DEFAULT_NODE_INPUT_CAPACITY);
        let pin_distributor_capacity =
            config.pin_distributor_capacity.unwrap_or(DEFAULT_PIN_DISTRIBUTOR_CAPACITY);

        tracing::info!(
            session_id = config.session_id.as_deref(),
            packet_batch_size = config.packet_batch_size,
            node_input_capacity,
            node_input_capacity_source =
                if config.node_input_capacity.is_some() { "config" } else { "default" },
            pin_distributor_capacity,
            pin_distributor_capacity_source =
                if config.pin_distributor_capacity.is_some() { "config" } else { "default" },
            engine_control_capacity = DEFAULT_ENGINE_CONTROL_CAPACITY,
            engine_query_capacity = DEFAULT_ENGINE_QUERY_CAPACITY,
            per_pin_control_capacity = DEFAULT_CONTROL_CAPACITY,
            "Starting Dynamic Engine actor"
        );

        let (nc_tx, nc_rx) = mpsc::channel(64);

        let meter = global::meter("skit_engine");
        let dynamic_engine = DynamicEngine {
            registry: Arc::clone(&self.registry),
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
            batch_size: config.packet_batch_size,
            session_id: config.session_id,
            audio_pool: self.audio_pool.clone(),
            video_pool: self.video_pool.clone(),
            node_input_capacity,
            pin_distributor_capacity,
            node_states: Arc::new(HashMap::new()),
            state_subscribers: Vec::new(),
            node_stats: Arc::new(HashMap::new()),
            stats_subscribers: Vec::new(),
            telemetry_subscribers: Vec::new(),
            node_view_data: Arc::new(HashMap::new()),
            view_data_subscribers: Vec::new(),
            runtime_schemas: HashMap::new(),
            runtime_schema_subscribers: Vec::new(),
            node_added_subscribers: Vec::new(),
            nodes_active_gauge: meter
                .u64_gauge("engine.nodes.active")
                .with_description("Number of active nodes in the pipeline")
                .build(),
            node_state_transitions_counter: meter
                .u64_counter("engine.node.state_transitions")
                .with_description("Node state transitions")
                .build(),
            engine_operations_counter: meter
                .u64_counter("engine.operations")
                .with_description("Engine control operations")
                .build(),
            node_packets_received_counter: meter
                .u64_counter("node.packets.received")
                .with_description("Total packets received by node")
                .build(),
            node_packets_sent_counter: meter
                .u64_counter("node.packets.sent")
                .with_description("Total packets sent by node")
                .build(),
            node_packets_discarded_counter: meter
                .u64_counter("node.packets.discarded")
                .with_description("Total packets discarded by node")
                .build(),
            node_packets_errored_counter: meter
                .u64_counter("node.packets.errored")
                .with_description("Total packet processing errors by node")
                .build(),
            node_state_gauge: meter
                .u64_gauge("node.state")
                .with_description("Node state (1=running, 0=stopped/failed)")
                .build(),
            engine_control_tx,
            node_created_tx: nc_tx,
            node_created_rx: nc_rx,
            pending_connections: Vec::new(),
            pending_tunes: Vec::new(),
            next_creation_id: 0,
            active_creations: std::collections::HashMap::new(),
        };

        let engine_task = tokio::spawn(dynamic_engine.run());

        DynamicEngineHandle::new(control_tx, query_tx, engine_task)
    }
}

#[cfg(test)]
mod tests;
