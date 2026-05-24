// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared performance constants for both dynamic and oneshot engines.
//!
//! Channel capacities are in packets. At 20ms/48kHz audio frames
//! (~7.5 KB each), capacity N ≈ N × 20ms of buffered audio.

/// Packets processed per node yield. Range: 8-64.
pub const DEFAULT_BATCH_SIZE: usize = 32;

/// Per-input-pin buffer (dynamic engine). 128 ≈ ~2.5s at 20ms frames.
pub const DEFAULT_NODE_INPUT_CAPACITY: usize = 128;

/// Buffer between node output and pin distributor.
/// Worst-case per hop ≈ this + `NODE_INPUT_CAPACITY`.
pub const DEFAULT_PIN_DISTRIBUTOR_CAPACITY: usize = 64;

/// Per-node control channel (UpdateParams, etc.).
pub const DEFAULT_CONTROL_CAPACITY: usize = 32;

/// Engine-level control channel (AddNode, Connect, etc.).
pub const DEFAULT_ENGINE_CONTROL_CAPACITY: usize = 128;

pub const DEFAULT_ENGINE_QUERY_CAPACITY: usize = 32;

/// Per-subscriber update channel (state/stats watchers).
pub const DEFAULT_SUBSCRIBER_CHANNEL_CAPACITY: usize = 128;

/// Oneshot media channel — larger than dynamic because batch throughput
/// matters more than tight backpressure here.
pub const DEFAULT_ONESHOT_MEDIA_CAPACITY: usize = 256;

pub const DEFAULT_ONESHOT_CONTROL_CAPACITY: usize = 32;

pub const DEFAULT_STATE_CHANNEL_CAPACITY: usize = 32;

/// HTTP I/O streaming. Smaller than media channels because
/// raw-byte chunks tend to be larger.
pub const DEFAULT_ONESHOT_IO_CAPACITY: usize = 16;

/// Codec async ↔ blocking thread handoff.
pub const DEFAULT_CODEC_CHANNEL_CAPACITY: usize = 32;

/// Container demuxer reader channel.
pub const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 8;

pub const DEFAULT_DEMUXER_BUFFER_SIZE: usize = 64 * 1024;

/// MoQ transport send/receive coordination.
pub const DEFAULT_MOQ_PEER_CHANNEL_CAPACITY: usize = 100;
