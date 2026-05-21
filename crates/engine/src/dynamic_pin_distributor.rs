// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pin distributor actor for the data plane.
//!
//! The PinDistributorActor is responsible for distributing packets from a single
//! output pin to multiple downstream input pins. Supports two connection modes:
//!
//! - **Reliable**: Synchronized backpressure - waits for slow consumers
//! - **BestEffort**: Avoids backpressure; keeps the newest packet when downstream is congested

use crate::dynamic_messages::{ConnectionId, ConnectionMode, PinConfigMsg};
use std::collections::HashMap;
use std::time::Instant;
use streamkit_core::types::Packet;
use tokio::sync::mpsc;

const EWMA_ALPHA: f64 = 0.1;

/// Information about a downstream connection.
struct OutputConnection {
    tx: mpsc::Sender<Packet>,
    mode: ConnectionMode,
    pending_best_effort: Option<Packet>,
}

/// Actor responsible for distributing packets from a single output pin (Data Plane).
pub struct PinDistributorActor {
    /// Input from the node (data path)
    data_rx: mpsc::Receiver<streamkit_core::types::Packet>,
    /// Input from the control plane
    config_rx: mpsc::Receiver<PinConfigMsg>,
    /// Map of active downstream connections with their modes
    outputs: HashMap<ConnectionId, OutputConnection>,
    /// Metadata for logging
    node_id: String,
    pin_name: String,
    /// Telemetry: packets successfully distributed
    packets_distributed_counter: opentelemetry::metrics::Counter<u64>,
    /// Telemetry: packets dropped (no outputs configured)
    packets_dropped_counter: opentelemetry::metrics::Counter<u64>,
    /// Telemetry: packets dropped due to best-effort backpressure
    best_effort_drops_counter: opentelemetry::metrics::Counter<u64>,
    /// Telemetry: number of active outputs
    outputs_active_gauge: opentelemetry::metrics::Gauge<u64>,
    /// Telemetry: time spent blocked on downstream backpressure (send().await)
    send_wait_histogram: opentelemetry::metrics::Histogram<f64>,
    /// Telemetry: depth of the distributor's incoming queue
    queue_depth_gauge: opentelemetry::metrics::Gauge<u64>,
    /// Telemetry: estimated backlog in bytes
    queue_depth_bytes_gauge: opentelemetry::metrics::Gauge<u64>,
    /// Telemetry: estimated backlog in media seconds (based on observed durations)
    queue_depth_seconds_gauge: opentelemetry::metrics::Gauge<f64>,
    /// Pre-built metric labels - allocated once in new(), reused on every packet
    metric_labels: [opentelemetry::KeyValue; 2],
    /// EWMA of packet size in bytes for this pin
    avg_packet_size_bytes: f64,
    /// EWMA of packet duration in seconds for this pin (when available)
    avg_packet_duration_s: f64,
}

/// Estimate the serialized JSON byte length of a `serde_json::Value` without
/// allocating a temporary `String`.
pub fn json_byte_len(value: &serde_json::Value) -> usize {
    struct CountWriter(usize);
    impl std::io::Write for CountWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = CountWriter(0);
    // `Value` serialization to a non-failing writer is infallible: the only
    // error path in serde_json is the writer's `io::Error`, and `CountWriter`
    // never fails.  We surface any (impossible) error via `debug_assert` in
    // dev builds and silently accept the (accurate) partial count in release.
    let result = serde_json::to_writer(&mut w, value);
    debug_assert!(result.is_ok(), "Value serialization should be infallible");
    w.0
}

impl PinDistributorActor {
    /// Creates a new pin distributor actor.
    pub(super) fn new(
        data_rx: mpsc::Receiver<streamkit_core::types::Packet>,
        config_rx: mpsc::Receiver<PinConfigMsg>,
        node_id: String,
        pin_name: String,
    ) -> Self {
        use opentelemetry::KeyValue;

        let meter = opentelemetry::global::meter("skit_engine");
        let packets_distributed_counter = meter
            .u64_counter("pin_distributor.packets_distributed")
            .with_description("Number of packets successfully distributed by pin distributors")
            .build();
        let packets_dropped_counter = meter
            .u64_counter("pin_distributor.packets_dropped")
            .with_description("Number of packets dropped (no outputs configured)")
            .build();
        let best_effort_drops_counter = meter
            .u64_counter("pin_distributor.best_effort_drops")
            .with_description(
                "Number of packets dropped/overwritten on best-effort connections due to backpressure",
            )
            .build();
        let outputs_active_gauge = meter
            .u64_gauge("pin_distributor.outputs_active")
            .with_description("Number of active downstream outputs for a pin")
            .build();
        let send_wait_histogram = meter
            .f64_histogram("pin_distributor.send_wait_seconds")
            .with_description("Time spent waiting for downstream capacity (backpressure)")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_BACKPRESSURE.to_vec())
            .build();
        let queue_depth_gauge = meter
            .u64_gauge("pin_distributor.queue_depth")
            .with_description("Current backlog of packets waiting to be distributed")
            .build();
        let queue_depth_bytes_gauge = meter
            .u64_gauge("pin_distributor.queue_depth_bytes")
            .with_description("Estimated backlog (bytes) at pin distributor input")
            .build();
        let queue_depth_seconds_gauge = meter
            .f64_gauge("pin_distributor.queue_depth_seconds")
            .with_description(
                "Estimated backlog (seconds) at pin distributor input (from packet timing)",
            )
            .build();

        let metric_labels = [
            KeyValue::new("node_id", node_id.clone()),
            KeyValue::new("pin_name", pin_name.clone()),
        ];
        outputs_active_gauge.record(0, &metric_labels);

        Self {
            data_rx,
            config_rx,
            outputs: HashMap::new(),
            node_id,
            pin_name,
            packets_distributed_counter,
            packets_dropped_counter,
            best_effort_drops_counter,
            outputs_active_gauge,
            send_wait_histogram,
            queue_depth_gauge,
            queue_depth_bytes_gauge,
            queue_depth_seconds_gauge,
            metric_labels,
            avg_packet_size_bytes: 0.0,
            avg_packet_duration_s: 0.0,
        }
    }

    #[allow(clippy::cognitive_complexity)]
    pub(super) async fn run(mut self) {
        tracing::debug!("PinDistributorActor started for {}.{}", self.node_id, self.pin_name);

        loop {
            tokio::select! {
                biased;

                Some(msg) = self.config_rx.recv() => {
                    if !self.handle_config(msg) {
                        tracing::debug!(
                            "{}.{}: PinDistributor received Shutdown. Draining.",
                            self.node_id,
                            self.pin_name
                        );
                        break;
                    }
                },

                Some(packet) = self.data_rx.recv() => {
                    self.distribute_packet(packet).await;
                },
                else => {
                    tracing::debug!(
                        "{}.{}: PinDistributor inputs closed. Shutting down.",
                        self.node_id,
                        self.pin_name
                    );
                    return;
                },
            }
        }

        self.config_rx.close();
        self.data_rx.close();

        tracing::debug!(
            "PinDistributorActor finished for {}.{} (immediate shutdown, {} packets may be dropped)",
            self.node_id,
            self.pin_name,
            self.data_rx.len()
        );
    }

    /// Handles configuration messages. Returns false if shutdown is requested.
    fn handle_config(&mut self, msg: PinConfigMsg) -> bool {
        match msg {
            PinConfigMsg::AddConnection { id, tx, mode } => {
                self.outputs.insert(id, OutputConnection { tx, mode, pending_best_effort: None });
            },
            PinConfigMsg::RemoveConnection { id } => {
                self.outputs.remove(&id);
            },
            PinConfigMsg::Shutdown => {
                return false;
            },
        }
        self.outputs_active_gauge.record(self.outputs.len() as u64, &self.metric_labels);
        true
    }

    /// Distributes a single packet to all outputs.
    ///
    /// For `Reliable` connections: synchronized backpressure - waits for slow consumers.
    /// For `BestEffort` connections: drops packets when buffer is full (no waiting).
    #[allow(
        clippy::cognitive_complexity,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )] // Fan-out with mode handling requires multiple paths and metric estimation casts
    async fn distribute_packet(&mut self, packet: Packet) {
        use futures::stream::{FuturesUnordered, StreamExt};
        use tokio::sync::mpsc::error::TrySendError;

        let queue_len = self.data_rx.len() as u64;
        self.queue_depth_gauge.record(queue_len, &self.metric_labels);

        let (pkt_bytes, pkt_duration_s) = Self::packet_stats(&packet);
        self.avg_packet_size_bytes = if self.avg_packet_size_bytes == 0.0 {
            pkt_bytes
        } else {
            self.avg_packet_size_bytes.mul_add(1.0 - EWMA_ALPHA, pkt_bytes * EWMA_ALPHA)
        };
        if let Some(dur) = pkt_duration_s {
            self.avg_packet_duration_s = if self.avg_packet_duration_s == 0.0 {
                dur
            } else {
                self.avg_packet_duration_s.mul_add(1.0 - EWMA_ALPHA, dur * EWMA_ALPHA)
            };
        }
        let est_bytes = (self.avg_packet_size_bytes * queue_len as f64) as u64;
        if est_bytes > 0 {
            self.queue_depth_bytes_gauge.record(est_bytes, &self.metric_labels);
        }
        if self.avg_packet_duration_s > 0.0 {
            let est_seconds = self.avg_packet_duration_s * queue_len as f64;
            self.queue_depth_seconds_gauge.record(est_seconds, &self.metric_labels);
        }

        if self.outputs.is_empty() {
            // No outputs configured - drop packet and record metric
            // Use pre-built labels - no allocation on hot path
            self.packets_dropped_counter.add(1, &self.metric_labels);
            return;
        }

        // Optimization: Handle the common case of a single destination without cloning.
        if self.outputs.len() == 1 {
            // Best-effort needs a small per-output buffer (pending_best_effort), but does not await.
            if matches!(
                self.outputs.values().next().map(|c| &c.mode),
                Some(ConnectionMode::BestEffort)
            ) {
                // Use let-else pattern for safety instead of unwrap
                let Some((id, conn)) = self.outputs.iter_mut().next() else {
                    tracing::error!(
                        "{}.{}: Outputs unexpectedly empty despite len() == 1",
                        self.node_id,
                        self.pin_name
                    );
                    return;
                };

                // Optimization: try_send first, only store on Full (avoids store-then-take in common case)
                match conn.tx.try_send(packet) {
                    Ok(()) => {
                        self.packets_distributed_counter.add(1, &self.metric_labels);
                    },
                    Err(TrySendError::Full(packet)) => {
                        // Channel full - store packet for later (drop-old semantics)
                        if conn.pending_best_effort.is_some() {
                            self.best_effort_drops_counter.add(1, &self.metric_labels);
                        }
                        conn.pending_best_effort = Some(packet);
                    },
                    Err(TrySendError::Closed(_packet)) => {
                        let id = id.clone();
                        tracing::warn!(
                            "{}.{}: Downstream connection {} closed.",
                            self.node_id,
                            self.pin_name,
                            id
                        );
                        self.outputs.remove(&id);
                    },
                }
            } else {
                // Reliable: preserve synchronized backpressure semantics.
                let Some((id, conn)) = self.outputs.iter().next() else {
                    tracing::error!(
                        "{}.{}: Outputs unexpectedly empty despite len() == 1",
                        self.node_id,
                        self.pin_name
                    );
                    return;
                };

                let id = id.clone();
                let tx = conn.tx.clone();

                match tx.try_send(packet) {
                    Ok(()) => {
                        self.packets_distributed_counter.add(1, &self.metric_labels);
                    },
                    Err(TrySendError::Full(packet)) => {
                        let start = Instant::now();
                        let result = tx.send(packet).await;
                        self.send_wait_histogram
                            .record(start.elapsed().as_secs_f64(), &self.metric_labels);
                        if result.is_err() {
                            tracing::warn!(
                                "{}.{}: Downstream connection {} closed.",
                                self.node_id,
                                self.pin_name,
                                id
                            );
                            self.outputs.remove(&id);
                        } else {
                            self.packets_distributed_counter.add(1, &self.metric_labels);
                        }
                    },
                    Err(TrySendError::Closed(_packet)) => {
                        tracing::warn!(
                            "{}.{}: Downstream connection {} closed.",
                            self.node_id,
                            self.pin_name,
                            id
                        );
                        self.outputs.remove(&id);
                    },
                }
            }
            return;
        }

        // Fan-out to multiple outputs.
        //
        // Strategy:
        // - For Reliable connections: fall back to `send().await` if channel is full.
        // - For BestEffort connections: keep newest packet in a 1-slot buffer and try_send it.
        let mut successes = 0u64;
        let mut best_effort_drops = 0u64;
        let mut to_remove: Vec<ConnectionId> = Vec::new();
        // Let Rust infer future type - avoids Box::pin allocation per future
        let mut pending = FuturesUnordered::new();

        for (id, conn) in &mut self.outputs {
            match conn.mode {
                ConnectionMode::BestEffort => {
                    // Optimization: try_send first, only store on Full (avoids store-then-take in common case)
                    match conn.tx.try_send(packet.clone()) {
                        Ok(()) => {
                            successes += 1;
                        },
                        Err(TrySendError::Full(packet_clone)) => {
                            // Channel full - store packet for later (drop-old semantics)
                            if conn.pending_best_effort.is_some() {
                                best_effort_drops += 1;
                            }
                            conn.pending_best_effort = Some(packet_clone);
                        },
                        Err(TrySendError::Closed(_packet_clone)) => {
                            to_remove.push(id.clone());
                        },
                    }
                },
                ConnectionMode::Reliable => {
                    let packet_clone = packet.clone();
                    match conn.tx.try_send(packet_clone) {
                        Ok(()) => {
                            successes += 1;
                        },
                        Err(TrySendError::Full(packet_clone)) => {
                            let id = id.clone();
                            let tx = conn.tx.clone();
                            // Push async block directly - no Box::pin allocation
                            pending.push(async move {
                                let start = Instant::now();
                                let result = tx.send(packet_clone).await;
                                (id, start.elapsed().as_secs_f64(), result)
                            });
                        },
                        Err(TrySendError::Closed(_packet_clone)) => {
                            to_remove.push(id.clone());
                        },
                    }
                },
            }
        }

        // Wait for all pending reliable sends to complete
        while let Some((id, waited_secs, result)) = pending.next().await {
            self.send_wait_histogram.record(waited_secs, &self.metric_labels);
            if result.is_err() {
                to_remove.push(id);
            } else {
                successes += 1;
            }
        }

        // Remove closed connections
        for id in to_remove {
            tracing::warn!(
                "{}.{}: Downstream connection {} closed during fan-out.",
                self.node_id,
                self.pin_name,
                id
            );
            self.outputs.remove(&id);
        }

        // Record metrics
        if successes > 0 {
            self.packets_distributed_counter.add(successes, &self.metric_labels);
        }
        if best_effort_drops > 0 {
            self.best_effort_drops_counter.add(best_effort_drops, &self.metric_labels);
        }
    }

    /// Extract approximate size in bytes and optional duration (seconds) for a packet.
    #[allow(clippy::cast_precision_loss)]
    fn packet_stats(packet: &Packet) -> (f64, Option<f64>) {
        match packet {
            Packet::Audio(frame) => {
                let bytes = (frame.samples.len() * std::mem::size_of::<f32>()) as f64;
                let dur_s = frame.duration_us().map(|us| us as f64 / 1_000_000.0);
                (bytes, dur_s)
            },
            Packet::Video(frame) => {
                let bytes = frame.data.len() as f64;
                let dur_s = frame
                    .metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .map(|us| us as f64 / 1_000_000.0);
                (bytes, dur_s)
            },
            Packet::Binary { data, metadata, .. } => {
                let bytes = data.len() as f64;
                let dur_s =
                    metadata.as_ref().and_then(|m| m.duration_us).map(|us| us as f64 / 1_000_000.0);
                (bytes, dur_s)
            },
            Packet::Text(t) => (t.len() as f64, None),
            Packet::Transcription(t) => (
                t.text.len() as f64,
                t.metadata.as_ref().and_then(|m| m.duration_us).map(|us| us as f64 / 1_000_000.0),
            ),
            Packet::Custom(c) => (
                json_byte_len(&c.data) as f64,
                c.metadata.as_ref().and_then(|m| m.duration_us).map(|us| us as f64 / 1_000_000.0),
            ),
        }
    }
}
