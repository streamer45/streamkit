// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! StreamKit Core - Fundamental traits and data structures for real-time media processing.
//!
//! This crate defines the core abstractions for building StreamKit pipelines:
//!
//! ## Core Modules
//!
//! - [`types`]: Core data types (Packet, AudioFrame, PacketType, etc.)
//! - [`node`]: ProcessorNode trait and execution context
//! - [`registry`]: Node factory and discovery system
//! - [`pins`]: Pin system for graph validation and type checking
//! - [`state`]: Node state machine and lifecycle tracking
//! - [`stats`]: Node statistics collection and reporting
//! - [`telemetry`]: Telemetry event emission for observability
//! - [`control`]: Control messages for node and engine management
//! - [`error`]: Error types and handling
//! - [`resource_manager`]: Shared resource management (ML models, GPU contexts)
//! - [`packet_meta`]: Packet type metadata and compatibility checking
//! - [`moq_gateway`]: MoQ WebTransport routing infrastructure
//! - [`helpers`]: Utility functions for configuration and packet processing
//!
//! ## Quick Start
//!
//! ```ignore
//! use streamkit_core::node::{ProcessorNode, NodeContext};
//! use streamkit_core::types::{Packet, AudioFrame};
//! use streamkit_core::pins::{InputPin, OutputPin};
//! use streamkit_core::registry::NodeRegistry;
//!
//! // Define a custom node
//! struct GainNode { gain: f32 }
//!
//! #[async_trait]
//! impl ProcessorNode for GainNode {
//!     fn input_pins(&self) -> Vec<InputPin> { /* ... */ }
//!     fn output_pins(&self) -> Vec<OutputPin> { /* ... */ }
//!     async fn run(self: Box<Self>, ctx: NodeContext) { /* ... */ }
//! }
//!
//! // Register with the factory
//! let mut registry = NodeRegistry::new();
//! registry.register_static(/* ... */);
//! ```

pub use async_trait::async_trait;

pub mod constraints;
pub mod control;
pub mod error;
pub mod frame_pool;
pub mod helpers;
pub mod hints;
pub mod metrics;
pub mod moq_gateway;
pub mod mse_gateway;
pub mod node;
pub mod node_config;
pub mod packet_meta;
pub mod pins;
pub mod registry;
pub mod resource_manager;
pub mod state;
pub mod stats;
pub mod telemetry;
pub mod timing;
pub mod types;
pub mod view_data;

pub use constraints::{GlobalNodeConstraints, NodeConstraint};
pub use error::StreamKitError;
pub use frame_pool::{
    AudioFramePool, FramePool, PooledFrameData, PooledSamples, PooledVideoData, VideoFramePool,
};
pub use helpers::{config_helpers, packet_helpers, path_helpers};
pub use hints::UpstreamHint;
pub use node::{
    InitContext, NodeContext, OutputSendError, OutputSender, PipelineMode, ProcessorNode,
    RoutedPacketMessage,
};
pub use node_config::{
    get_codec_channel_capacity, get_demuxer_buffer_size, get_moq_peer_channel_capacity,
    get_stream_channel_capacity, set_node_buffer_config, NodeBufferConfig,
};
pub use pins::{InputPin, OutputPin, PinCardinality};
pub use registry::{NodeDefinition, NodeRegistry};
pub use resource_manager::{Resource, ResourceError, ResourceKey, ResourceManager, ResourcePolicy};
pub use state::state_helpers;
pub use state::{NodeState, NodeStateUpdate, StopReason};
pub use stats::{NodeStats, NodeStatsUpdate};
pub use telemetry::telemetry_helpers;
pub use telemetry::{TelemetryConfig, TelemetryEmitter, TelemetryEvent};
pub use timing::*;
pub use view_data::view_data_helpers;
pub use view_data::NodeViewDataUpdate;
