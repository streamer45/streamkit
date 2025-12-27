// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Node statistics tracking and reporting.
//!
//! This module provides types and utilities for collecting runtime statistics
//! from nodes during pipeline execution. Statistics are throttled to prevent
//! overload (typically every 10 seconds or 1000 packets).

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use ts_rs::TS;

/// Runtime statistics for a node, tracking packet processing metrics.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeStats {
    /// Total packets received on all input pins
    pub received: u64,
    /// Total packets successfully sent on all output pins
    pub sent: u64,
    /// Total packets discarded (e.g., due to backpressure, invalid data)
    pub discarded: u64,
    /// Total processing errors that didn't crash the node
    pub errored: u64,
    /// Duration in seconds since the node started processing (for rate calculation)
    pub duration_secs: f64,
}

impl Default for NodeStats {
    fn default() -> Self {
        Self { received: 0, sent: 0, discarded: 0, errored: 0, duration_secs: 0.0 }
    }
}

/// A statistics update message sent by a node to report its current metrics.
/// These updates are throttled to prevent overload (typically every 10s or 1000 packets).
#[derive(Debug, Clone)]
pub struct NodeStatsUpdate {
    /// The unique identifier of the node reporting the stats
    pub node_id: String,
    /// The current statistics snapshot
    pub stats: NodeStats,
    /// When this snapshot was taken
    pub timestamp: SystemTime,
}

/// Helper for tracking and throttling node statistics updates.
/// Automatically sends updates every 10 seconds or 1000 packets.
pub struct NodeStatsTracker {
    stats: NodeStats,
    start_time: std::time::Instant,
    last_send: std::time::Instant,
    node_id: String,
    stats_tx: Option<tokio::sync::mpsc::Sender<NodeStatsUpdate>>,
}

impl NodeStatsTracker {
    /// Throttling configuration
    const SEND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
    const SEND_PACKET_THRESHOLD: u64 = 1000;

    /// Create a new stats tracker for a node
    pub fn new(
        node_id: String,
        stats_tx: Option<tokio::sync::mpsc::Sender<NodeStatsUpdate>>,
    ) -> Self {
        let now = std::time::Instant::now();
        Self { stats: NodeStats::default(), start_time: now, last_send: now, node_id, stats_tx }
    }

    /// Record a received packet
    #[inline]
    pub const fn received(&mut self) {
        self.stats.received += 1;
    }

    /// Record multiple received packets (for batched stats reporting)
    #[inline]
    pub const fn received_n(&mut self, count: u64) {
        self.stats.received += count;
    }

    /// Record a sent packet
    #[inline]
    pub const fn sent(&mut self) {
        self.stats.sent += 1;
    }

    /// Record a discarded packet
    #[inline]
    pub const fn discarded(&mut self) {
        self.stats.discarded += 1;
    }

    /// Record an error
    #[inline]
    pub const fn errored(&mut self) {
        self.stats.errored += 1;
    }

    /// Automatically send stats if threshold is met (every 10s or 1000 packets).
    /// Call this after processing a batch of packets.
    pub fn maybe_send(&mut self) {
        let should_send = self.last_send.elapsed() >= Self::SEND_INTERVAL
            || self.stats.received.is_multiple_of(Self::SEND_PACKET_THRESHOLD);

        if should_send {
            self.force_send();
        }
    }

    /// Force send stats immediately (useful for final updates)
    pub fn force_send(&mut self) {
        if let Some(ref stats_tx) = self.stats_tx {
            // Update duration before sending
            self.stats.duration_secs = self.start_time.elapsed().as_secs_f64();

            let _ = stats_tx.try_send(NodeStatsUpdate {
                node_id: self.node_id.clone(),
                stats: self.stats.clone(),
                timestamp: SystemTime::now(),
            });
            self.last_send = std::time::Instant::now();
        }
    }
}
