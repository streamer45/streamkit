// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! engine: The core pipeline execution engine for Skit.
//! This crate is responsible for running pipeline graphs, both oneshot and dynamic.

use opentelemetry::global;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
pub use streamkit_api::Connection;
use streamkit_core::registry::NodeRegistry;
use tokio::sync::mpsc;

// --- Public Modules ---

pub mod constants;
pub mod graph_builder;
pub mod oneshot;

// Dynamic engine modules (gated by feature flag)
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

// Re-exports
#[cfg(feature = "dynamic")]
pub use dynamic_config::DynamicEngineConfig;
#[cfg(feature = "dynamic")]
pub use dynamic_handle::DynamicEngineHandle;
pub use oneshot::{OneshotEngineConfig, OneshotPipelineResult};

// Import constants and types (within dynamic module)
#[cfg(feature = "dynamic")]
use constants::{
    DEFAULT_CONTROL_CAPACITY, DEFAULT_ENGINE_CONTROL_CAPACITY, DEFAULT_ENGINE_QUERY_CAPACITY,
    DEFAULT_NODE_INPUT_CAPACITY, DEFAULT_PIN_DISTRIBUTOR_CAPACITY,
};
#[cfg(feature = "dynamic")]
use dynamic_actor::DynamicEngine;

// --- Engine Structs ---

/// The main Engine struct, which acts as a unified entry point.
/// It can be used to run stateless pipelines or to start the long-running dynamic actor.
pub struct Engine {
    pub registry: Arc<RwLock<NodeRegistry>>,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Creates a new engine with a populated node registry.
    pub fn new() -> Self {
        Self::build(
            true,
            None,
            None,
            #[cfg(feature = "script")]
            None,
            #[cfg(feature = "script")]
            std::collections::HashMap::new(),
        )
    }

    /// Creates a new engine with a custom plugin directory.
    pub fn with_plugin_dir(plugin_dir: Option<std::path::PathBuf>) -> Self {
        Self::build(
            true,
            plugin_dir,
            None,
            #[cfg(feature = "script")]
            None,
            #[cfg(feature = "script")]
            std::collections::HashMap::new(),
        )
    }

    /// Creates a new engine with only built-in nodes registered, skipping plugin loading.
    pub fn without_plugins() -> Self {
        Self::build(
            false,
            None,
            None,
            #[cfg(feature = "script")]
            None,
            #[cfg(feature = "script")]
            std::collections::HashMap::new(),
        )
    }

    /// Creates a new engine with resource management support.
    /// This is typically used by the server to enable shared resource caching (ML models, etc.).
    pub fn with_resource_manager(resource_manager: Arc<streamkit_core::ResourceManager>) -> Self {
        Self::build(
            false,
            None,
            Some(resource_manager),
            #[cfg(feature = "script")]
            None,
            #[cfg(feature = "script")]
            std::collections::HashMap::new(),
        )
    }

    /// Creates a new engine with resource management and script configuration.
    /// This is typically used by the server to pass global script allowlists and secrets.
    #[cfg(feature = "script")]
    pub fn with_resource_manager_and_script_config(
        resource_manager: Arc<streamkit_core::ResourceManager>,
        global_script_allowlist: Option<Vec<streamkit_nodes::core::script::AllowlistRule>>,
        secrets: std::collections::HashMap<String, streamkit_nodes::core::script::ScriptSecret>,
    ) -> Self {
        Self::build(false, None, Some(resource_manager), global_script_allowlist, secrets)
    }

    fn build(
        load_plugins: bool,
        plugin_dir: Option<std::path::PathBuf>,
        resource_manager: Option<Arc<streamkit_core::ResourceManager>>,
        #[cfg(feature = "script")] global_script_allowlist: Option<
            Vec<streamkit_nodes::core::script::AllowlistRule>,
        >,
        #[cfg(feature = "script")] secrets: std::collections::HashMap<
            String,
            streamkit_nodes::core::script::ScriptSecret,
        >,
    ) -> Self {
        let mut registry =
            resource_manager.map_or_else(NodeRegistry::new, NodeRegistry::with_resource_manager);

        // Register built-in nodes
        #[cfg(feature = "script")]
        streamkit_nodes::register_nodes(&mut registry, global_script_allowlist, secrets);

        #[cfg(not(feature = "script"))]
        streamkit_nodes::register_nodes(&mut registry, None, Default::default());

        if load_plugins {
            // Load WASM plugins if feature is enabled
            #[cfg(feature = "plugins")]
            Self::load_plugins(&mut registry, plugin_dir);
        }

        Self { registry: Arc::new(RwLock::new(registry)) }
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

    /// Starts the long-running dynamic actor in the background,
    /// returning a handle to send it control messages and query its state.
    ///
    /// # Panics
    ///
    /// Panics if the engine registry lock is poisoned (only possible if another thread
    /// panicked while holding the lock).
    #[cfg(feature = "dynamic")]
    pub fn start_dynamic_actor(&self, config: DynamicEngineConfig) -> DynamicEngineHandle {
        let (control_tx, control_rx) = mpsc::channel(DEFAULT_ENGINE_CONTROL_CAPACITY);
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

        // expect is documented in #[doc] Panics section above
        #[allow(clippy::expect_used)]
        let registry_snapshot = {
            let guard = self
                .registry
                .read()
                .expect("Engine registry poisoned while starting dynamic actor");
            guard.clone()
        };

        let meter = global::meter("skit_engine");
        let dynamic_engine = DynamicEngine {
            registry: registry_snapshot,
            control_rx,
            query_rx,
            live_nodes: HashMap::new(),
            node_inputs: HashMap::new(),
            pin_distributors: HashMap::new(),
            pin_management_txs: HashMap::new(),
            node_pin_metadata: HashMap::new(),
            batch_size: config.packet_batch_size,
            session_id: config.session_id,
            audio_pool: Arc::new(streamkit_core::FramePool::<f32>::audio_default()),
            node_input_capacity,
            pin_distributor_capacity,
            node_states: HashMap::new(),
            state_subscribers: Vec::new(),
            node_stats: HashMap::new(),
            stats_subscribers: Vec::new(),
            telemetry_subscribers: Vec::new(),
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
            node_packets_received_gauge: meter
                .u64_gauge("node.packets.received")
                .with_description("Total packets received by node")
                .build(),
            node_packets_sent_gauge: meter
                .u64_gauge("node.packets.sent")
                .with_description("Total packets sent by node")
                .build(),
            node_packets_discarded_gauge: meter
                .u64_gauge("node.packets.discarded")
                .with_description("Total packets discarded by node")
                .build(),
            node_packets_errored_gauge: meter
                .u64_gauge("node.packets.errored")
                .with_description("Total packet processing errors by node")
                .build(),
            node_state_gauge: meter
                .u64_gauge("node.state")
                .with_description("Node state (1=running, 0=stopped/failed)")
                .build(),
        };

        let engine_task = tokio::spawn(dynamic_engine.run());

        DynamicEngineHandle::new(control_tx, query_tx, engine_task)
    }
}

#[cfg(test)]
mod tests;
