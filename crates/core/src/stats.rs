// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Node statistics tracking and reporting.
//!
//! This module provides types and utilities for collecting runtime statistics
//! from nodes during pipeline execution. Statistics are throttled to prevent
//! overload (typically every 2 seconds or 1000 packets).

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
/// These updates are throttled to prevent overload (typically every 2s or 1000 packets).
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
/// Automatically sends updates every 2 seconds or 1000 packets.
pub struct NodeStatsTracker {
    stats: NodeStats,
    start_time: std::time::Instant,
    last_send: std::time::Instant,
    has_sent_once: bool,
    node_id: String,
    stats_tx: Option<tokio::sync::mpsc::Sender<NodeStatsUpdate>>,
}

impl NodeStatsTracker {
    /// Throttling configuration
    const SEND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
    const SEND_PACKET_THRESHOLD: u64 = 1000;

    /// Create a new stats tracker for a node
    pub fn new(
        node_id: String,
        stats_tx: Option<tokio::sync::mpsc::Sender<NodeStatsUpdate>>,
    ) -> Self {
        let now = std::time::Instant::now();
        Self {
            stats: NodeStats::default(),
            start_time: now,
            last_send: now,
            has_sent_once: false,
            node_id,
            stats_tx,
        }
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

    /// Record multiple sent packets (for batched stats reporting)
    #[inline]
    pub const fn sent_n(&mut self, count: u64) {
        self.stats.sent += count;
    }

    /// Record a discarded packet
    #[inline]
    pub const fn discarded(&mut self) {
        self.stats.discarded += 1;
    }

    /// Record multiple discarded packets (for batched stats reporting)
    #[inline]
    pub const fn discarded_n(&mut self, count: u64) {
        self.stats.discarded += count;
    }

    /// Record an error
    #[inline]
    pub const fn errored(&mut self) {
        self.stats.errored += 1;
    }

    /// Record multiple errors (for batched stats reporting)
    #[inline]
    pub const fn errored_n(&mut self, count: u64) {
        self.stats.errored += count;
    }

    /// Automatically send stats if threshold is met (every 2s or 1000 packets).
    /// Call this after processing a batch of packets.
    pub fn maybe_send(&mut self) {
        // Many nodes only increment one side of the counters (e.g. pure sources only `sent`,
        // pure sinks only `received`). Use the max to keep the threshold behavior consistent
        // across node shapes and avoid the `0.is_multiple_of(..)` pitfall.
        let packet_count = self.stats.received.max(self.stats.sent).max(self.stats.discarded);

        // Always emit the first non-empty snapshot promptly so monitoring can "lock on"
        // even if the node later blocks under backpressure.
        let should_send = (!self.has_sent_once && packet_count > 0)
            || self.last_send.elapsed() >= Self::SEND_INTERVAL
            || (packet_count > 0 && packet_count.is_multiple_of(Self::SEND_PACKET_THRESHOLD));

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
            self.has_sent_once = true;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn maybe_send_does_not_fire_when_empty() {
        let (tx, mut rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let mut tracker = NodeStatsTracker::new("node".to_string(), Some(tx));

        tracker.maybe_send();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn maybe_send_emits_on_first_activity_and_on_threshold() {
        let (tx, mut rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let mut tracker = NodeStatsTracker::new("node".to_string(), Some(tx));

        tracker.sent();
        tracker.maybe_send();
        let first = rx.try_recv().unwrap();
        assert_eq!(first.stats.sent, 1);

        for _ in 1..NodeStatsTracker::SEND_PACKET_THRESHOLD {
            tracker.sent();
            tracker.maybe_send();
        }

        let threshold = rx.try_recv().unwrap();
        assert_eq!(threshold.node_id, "node");
        assert_eq!(threshold.stats.sent, NodeStatsTracker::SEND_PACKET_THRESHOLD);
    }
}
