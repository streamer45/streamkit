// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared performance constants for both dynamic and oneshot engines.
//!
//! This module provides the canonical default values for all performance-related
//! configuration. Server config and engine configs should reference these constants
//! to ensure consistency across the codebase.
//!
//! # Channel Capacity Guidelines
//!
//! Channel capacities are measured in packets (not bytes). The actual memory footprint
//! depends on packet size (audio frames are typically 20ms at 48kHz = ~7.5KB per frame).
//!
//! - **Higher capacity** = more buffering, higher latency, smoother throughput
//! - **Lower capacity** = less buffering, lower latency, more backpressure
//!
//! For real-time audio streaming at 20ms frames:
//! - Capacity of N ≈ N × 20ms of buffered audio per channel

// === Batch Processing ===

/// Default batch size for packet processing in nodes.
///
/// Controls how many packets a node processes before yielding to check for
/// control messages. Lower values = more responsive to control messages,
/// higher values = better throughput due to reduced context switching.
///
/// Recommended range: 8-64
pub const DEFAULT_BATCH_SIZE: usize = 32;

// === Dynamic Engine Channel Capacities ===

/// Default buffer size for node input channels (dynamic engine).
///
/// Each input pin on a node has its own bounded channel with this capacity.
/// Higher values increase buffering and worst-case latency before upstream
/// backpressure kicks in.
///
/// At 20ms audio frames, capacity of 128 = up to ~2.5 seconds of queued audio.
///
/// Recommended:
/// - Low-latency streaming: 8-16 (~160-320ms)
/// - Balanced: 32 (~640ms)
/// - High-throughput batch: 128 (~2.5s)
pub const DEFAULT_NODE_INPUT_CAPACITY: usize = 128;

/// Default buffer size between node output and pin distributor (dynamic engine).
///
/// The pin distributor handles fan-out from a single output to multiple
/// downstream inputs. This adds another bounded queue in the hot path.
///
/// Worst-case queued packets per hop ≈ PIN_DISTRIBUTOR_CAPACITY + NODE_INPUT_CAPACITY
///
/// Recommended:
/// - Low-latency: 4
/// - Balanced: 16
/// - High-throughput: 64
pub const DEFAULT_PIN_DISTRIBUTOR_CAPACITY: usize = 64;

/// Default buffer size for per-node control message channels.
///
/// Used for sending UpdateParams and other control messages to individual nodes.
/// Typically doesn't need tuning unless you're sending very high-frequency
/// parameter updates.
pub const DEFAULT_CONTROL_CAPACITY: usize = 32;

/// Default buffer size for engine-level control message channel.
///
/// Used for AddNode, RemoveNode, Connect, Disconnect, TuneNode operations.
/// Larger than per-node control because graph operations may be batched.
pub const DEFAULT_ENGINE_CONTROL_CAPACITY: usize = 128;

/// Default buffer size for engine query channel.
///
/// Used for GetNodeStates, GetNodeStats, SubscribeState, SubscribeStats queries.
pub const DEFAULT_ENGINE_QUERY_CAPACITY: usize = 32;

/// Default buffer size for state/stats subscriber channels.
///
/// Each subscriber (e.g., WebSocket client watching node states) gets a channel
/// with this capacity for receiving updates. The dynamic engine also uses this as
/// the capacity for its internal state/stats update channels to avoid hard-coded
/// magic numbers.
pub const DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY: usize = 128;

// === Oneshot Engine Channel Capacities ===

/// Default buffer size for media channels in oneshot/stateless pipelines.
///
/// Oneshot pipelines use larger buffers by default because:
/// 1. No dynamic reconfiguration overhead
/// 2. Batch processing is the primary use case
/// 3. No need for tight backpressure coordination
pub const DEFAULT_ONESHOT_MEDIA_CAPACITY: usize = 256;

/// Default buffer size for control channels in oneshot pipelines.
pub const DEFAULT_ONESHOT_CONTROL_CAPACITY: usize = 32;

/// Default buffer size for state reporting channels.
pub const DEFAULT_STATE_CHANNEL_CAPACITY: usize = 32;

/// Default buffer size for I/O stream channels in oneshot pipelines.
///
/// Used for HTTP input/output streaming. Smaller than media channels because
/// these handle raw bytes that may be larger chunks.
pub const DEFAULT_ONESHOT_IO_CAPACITY: usize = 16;

// === Codec/Node Internal Buffers (Advanced) ===

/// Default capacity for codec async/blocking handoff channels.
///
/// Used by opus, flac, mp3 decoders/encoders for communication between
/// the async node task and the blocking codec thread.
pub const DEFAULT_CODEC_CHANNEL_CAPACITY: usize = 32;

/// Default capacity for streaming reader channels (container demuxers).
///
/// Smaller than codec channels because container frames may be larger
/// and we want to avoid excessive memory usage.
pub const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 8;

/// Default duplex buffer size for ogg demuxer (64KB).
pub const DEFAULT_DEMUXER_BUFFER_SIZE: usize = 64 * 1024;

/// Default capacity for MoQ transport peer channels.
///
/// Used for network send/receive coordination in MoQ transport nodes.
pub const DEFAULT_MOQ_PEER_CHANNEL_CAPACITY: usize = 100;
