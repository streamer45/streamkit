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

// Re-export async_trait for use in node implementations
pub use async_trait::async_trait;

// Module declarations
pub mod control;
pub mod error;
pub mod frame_pool;
pub mod helpers;
pub mod moq_gateway;
pub mod node;
pub mod node_config;
pub mod packet_meta;
pub mod pins;
pub mod registry;
pub mod resource_manager;
pub mod state;
pub mod stats;
pub mod telemetry;
pub mod types;

// Convenience re-exports for commonly used types
// These are the most frequently used types in node implementations

// Error handling
pub use error::StreamKitError;

// Core node abstractions
pub use node::{
    InitContext, NodeContext, OutputSendError, OutputSender, ProcessorNode, RoutedPacketMessage,
};

// Registry and factory
pub use registry::{NodeDefinition, NodeRegistry};

// Resource management
pub use resource_manager::{Resource, ResourceError, ResourceKey, ResourceManager, ResourcePolicy};

// State tracking
pub use state::{NodeState, NodeStateUpdate, StopReason};

// Statistics
pub use stats::{NodeStats, NodeStatsUpdate};

// Telemetry
pub use telemetry::{TelemetryConfig, TelemetryEmitter, TelemetryEvent};

// Pin definitions
pub use pins::{InputPin, OutputPin, PinCardinality};

// Helper modules (for convenience, maintaining backward compatibility)
pub use helpers::{config_helpers, packet_helpers};
pub use state::state_helpers;
pub use telemetry::telemetry_helpers;

// Frame pooling (optional hot-path optimization)
pub use frame_pool::{AudioFramePool, FramePool, PooledFrameData, PooledSamples};

// Node buffer configuration
pub use node_config::{
    get_codec_channel_capacity, get_demuxer_buffer_size, get_moq_peer_channel_capacity,
    get_stream_channel_capacity, set_node_buffer_config, NodeBufferConfig,
};
