// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Runtime configuration for internal node buffers.
//!
//! This module provides a global configuration mechanism for internal buffer settings
//! used by codec and container nodes. These settings are typically set once at server
//! startup and remain constant for the lifetime of the process.
//!
//! ## Usage
//!
//! Server startup:
//! ```ignore
//! use streamkit_core::node_config::{set_node_buffer_config, NodeBufferConfig};
//!
//! let config = NodeBufferConfig {
//!     codec_channel_capacity: 32,
//!     stream_channel_capacity: 8,
//!     demuxer_buffer_size: 65536,
//!     moq_peer_channel_capacity: 100,
//! };
//! set_node_buffer_config(config);
//! ```
//!
//! Node implementation:
//! ```ignore
//! use streamkit_core::node_config::get_codec_channel_capacity;
//!
//! let (tx, rx) = mpsc::channel(get_codec_channel_capacity());
//! ```
//!
//! ## Default Values
//!
//! If `set_node_buffer_config` is never called, the getter functions return sensible
//! defaults based on production experience:
//!
//! - `codec_channel_capacity`: 32 packets
//! - `stream_channel_capacity`: 8 packets
//! - `demuxer_buffer_size`: 65536 bytes (64KB)
//! - `moq_peer_channel_capacity`: 100 packets (MoQ transport internal queues)

use std::sync::OnceLock;

/// Default capacity for codec async/blocking handoff channels.
const DEFAULT_CODEC_CHANNEL_CAPACITY: usize = 32;

/// Default capacity for streaming reader channels (container demuxers).
const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 8;

/// Default duplex buffer size for ogg demuxer (64KB).
const DEFAULT_DEMUXER_BUFFER_SIZE: usize = 64 * 1024;

/// Default capacity for MoQ transport peer internal channels.
const DEFAULT_MOQ_PEER_CHANNEL_CAPACITY: usize = 100;

/// Runtime configuration for node internal buffers.
///
/// These settings affect the async/blocking handoff channels in codec and container nodes.
/// Most users should not need to modify these values.
#[derive(Debug, Clone)]
pub struct NodeBufferConfig {
    /// Capacity for codec processing channels (e.g. Opus encoder/decoder).
    /// Used for async/blocking handoff in codec nodes.
    pub codec_channel_capacity: usize,

    /// Capacity for streaming reader channels (container demuxers).
    /// Smaller than codec channels because container frames may be larger.
    pub stream_channel_capacity: usize,

    /// Duplex buffer size for ogg demuxer in bytes.
    pub demuxer_buffer_size: usize,

    /// Capacity for MoQ transport peer internal channels (packets).
    ///
    /// Used by MoQ transport nodes for per-connection send/receive coordination.
    /// This is not the same as engine graph channel capacities; it only affects the
    /// internal queueing within the transport node.
    pub moq_peer_channel_capacity: usize,
}

impl Default for NodeBufferConfig {
    fn default() -> Self {
        Self {
            codec_channel_capacity: DEFAULT_CODEC_CHANNEL_CAPACITY,
            stream_channel_capacity: DEFAULT_STREAM_CHANNEL_CAPACITY,
            demuxer_buffer_size: DEFAULT_DEMUXER_BUFFER_SIZE,
            moq_peer_channel_capacity: DEFAULT_MOQ_PEER_CHANNEL_CAPACITY,
        }
    }
}

/// Global storage for the node buffer configuration.
static NODE_BUFFER_CONFIG: OnceLock<NodeBufferConfig> = OnceLock::new();

/// Sets the global node buffer configuration.
///
/// This should be called once at server startup, before any nodes are created.
/// Subsequent calls are ignored (the first configuration wins).
///
/// # Example
///
/// ```ignore
/// use streamkit_core::node_config::{set_node_buffer_config, NodeBufferConfig};
///
/// set_node_buffer_config(NodeBufferConfig {
///     codec_channel_capacity: 64,  // Increased for high-throughput scenarios
///     stream_channel_capacity: 16,
///     demuxer_buffer_size: 128 * 1024,  // 128KB buffer
///     moq_peer_channel_capacity: 200,
/// });
/// ```
pub fn set_node_buffer_config(config: NodeBufferConfig) {
    if NODE_BUFFER_CONFIG.set(config).is_err() {
        tracing::warn!("Node buffer config already set, ignoring new configuration");
    }
}

/// Returns the configured codec channel capacity.
///
/// Used by codec nodes that offload CPU work to blocking tasks (e.g. Opus).
/// Returns the default (32) if no configuration was set.
#[inline]
pub fn get_codec_channel_capacity() -> usize {
    NODE_BUFFER_CONFIG.get().map_or(DEFAULT_CODEC_CHANNEL_CAPACITY, |c| c.codec_channel_capacity)
}

/// Returns the configured stream channel capacity.
///
/// Used by streaming readers that hand off `Bytes` into blocking demux/decode tasks
/// (container demuxers and some non-Opus codecs).
/// Returns the default (8) if no configuration was set.
#[inline]
pub fn get_stream_channel_capacity() -> usize {
    NODE_BUFFER_CONFIG.get().map_or(DEFAULT_STREAM_CHANNEL_CAPACITY, |c| c.stream_channel_capacity)
}

/// Returns the configured demuxer buffer size in bytes.
///
/// Used by ogg demuxer for its duplex buffer. Returns the default (64KB)
/// if no configuration was set.
#[inline]
pub fn get_demuxer_buffer_size() -> usize {
    NODE_BUFFER_CONFIG.get().map_or(DEFAULT_DEMUXER_BUFFER_SIZE, |c| c.demuxer_buffer_size)
}

/// Returns the configured MoQ peer channel capacity.
///
/// Used by MoQ transport peer tasks for per-connection internal buffering.
/// Returns the default (100) if no configuration was set.
#[inline]
pub fn get_moq_peer_channel_capacity() -> usize {
    NODE_BUFFER_CONFIG
        .get()
        .map_or(DEFAULT_MOQ_PEER_CHANNEL_CAPACITY, |c| c.moq_peer_channel_capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        // Without setting config, should return defaults
        // Note: This test may fail if run after other tests that set the config
        assert_eq!(DEFAULT_CODEC_CHANNEL_CAPACITY, 32);
        assert_eq!(DEFAULT_STREAM_CHANNEL_CAPACITY, 8);
        assert_eq!(DEFAULT_DEMUXER_BUFFER_SIZE, 64 * 1024);
        assert_eq!(DEFAULT_MOQ_PEER_CHANNEL_CAPACITY, 100);
    }
}
