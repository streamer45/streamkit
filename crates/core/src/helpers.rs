// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Utility functions for node configuration and packet processing.
//!
//! This module provides helper functions that simplify common tasks:
//! - [`config_helpers`]: Parse node configuration from YAML
//! - [`packet_helpers`]: Batch packet processing utilities

use crate::error::StreamKitError;
use crate::types::Packet;

/// Helper functions for parsing node configuration from JSON values.
pub mod config_helpers {
    use super::StreamKitError;
    use serde::Deserialize;

    /// Parses configuration from an optional JSON value, using defaults if not provided.
    /// This is the preferred approach for nodes with sensible defaults.
    ///
    /// # Errors
    ///
    /// This function always returns `Ok` in practice, as it uses `Default` when parsing fails.
    /// The `Result` return type is maintained for API consistency with other config helpers.
    pub fn parse_config_optional<T>(params: Option<&serde_json::Value>) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de> + Default,
    {
        Ok(serde_json::from_value(params.unwrap_or(&serde_json::Value::Null).clone())
            .unwrap_or_default())
    }

    /// Parses configuration from an optional JSON value, returning an error if not provided.
    /// Use this for nodes that require explicit configuration.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Configuration` if `params` is `None` or if deserialization fails.
    pub fn parse_config_required<T>(params: Option<&serde_json::Value>) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = params
            .ok_or_else(|| StreamKitError::Configuration("Configuration required".to_string()))?
            .clone();
        serde_json::from_value(value)
            .map_err(|e| StreamKitError::Configuration(format!("Failed to parse config: {e}")))
    }

    /// Parses configuration with detailed error messages.
    /// Use this when you want to provide context about what failed to parse.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Configuration` if `params` is `None` or if deserialization fails.
    pub fn parse_config_with_context<T>(
        params: Option<&serde_json::Value>,
        context: &str,
    ) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de>,
    {
        params.map_or_else(
            || Err(StreamKitError::Configuration(format!("{context} configuration required"))),
            |p| {
                serde_json::from_value(p.clone()).map_err(|e| {
                    StreamKitError::Configuration(format!("Failed to parse {context}: {e}"))
                })
            },
        )
    }
}

/// Helper functions for common packet processing patterns.
pub mod packet_helpers {
    use super::Packet;
    use smallvec::SmallVec;
    use tokio::sync::mpsc;

    /// Default batch size for stack-allocated SmallVec.
    ///
    /// 32 packets fits typical batch processing while avoiding heap allocation.
    /// Each Packet is ~40 bytes (enum discriminant + largest variant), so 32 packets = ~1.3KB on stack.
    pub const DEFAULT_BATCH_CAPACITY: usize = 32;

    /// A batch of packets that uses stack allocation for small batches.
    /// Falls back to heap allocation only if more than DEFAULT_BATCH_CAPACITY packets are collected.
    pub type PacketBatch = SmallVec<[Packet; DEFAULT_BATCH_CAPACITY]>;

    /// Greedily collects a batch of packets from a receiver.
    /// Starts with the given first packet, then attempts to drain up to `batch_size - 1`
    /// additional packets without blocking.
    ///
    /// This is useful for processing packets in batches to amortize processing overhead.
    ///
    /// # Performance
    ///
    /// Uses SmallVec to avoid heap allocation for batches up to 32 packets.
    /// For most real-time audio processing, batches are small (1-8 packets), so this
    /// avoids allocation in the common case.
    pub fn batch_packets_greedy(
        first_packet: Packet,
        rx: &mut mpsc::Receiver<Packet>,
        batch_size: usize,
    ) -> PacketBatch {
        let mut batch = PacketBatch::new();
        batch.push(first_packet);

        for _ in 0..batch_size.saturating_sub(1) {
            match rx.try_recv() {
                Ok(packet) => batch.push(packet),
                Err(_) => break,
            }
        }
        batch
    }
}
