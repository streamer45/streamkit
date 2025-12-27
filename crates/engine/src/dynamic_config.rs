// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration and constants for the dynamic engine.

use crate::constants::DEFAULT_BATCH_SIZE;

pub use crate::constants::DEFAULT_CONTROL_CAPACITY as CONTROL_CAPACITY;

/// Configuration for the dynamic engine actor.
#[derive(Debug, Clone)]
pub struct DynamicEngineConfig {
    /// Batch size for processing packets in nodes (default: 32)
    /// Lower values = more responsive to control messages, higher values = better throughput
    pub packet_batch_size: usize,
    /// Session ID for gateway registration (if applicable)
    pub session_id: Option<String>,
    /// Buffer size for node input channels (default: 128 packets)
    /// Higher = more buffering/latency, lower = more backpressure/responsiveness
    /// For low-latency streaming, consider 8-16 packets (~160-320ms at 20ms/frame)
    pub node_input_capacity: Option<usize>,
    /// Buffer size between node output and pin distributor (default: 64 packets)
    /// For low-latency streaming, consider 4-8 packets
    pub pin_distributor_capacity: Option<usize>,
}

impl Default for DynamicEngineConfig {
    fn default() -> Self {
        Self {
            packet_batch_size: DEFAULT_BATCH_SIZE,
            session_id: None,
            node_input_capacity: None, // Uses DEFAULT_NODE_INPUT_CAPACITY when None
            pin_distributor_capacity: None, // Uses DEFAULT_PIN_DISTRIBUTOR_CAPACITY when None
        }
    }
}
