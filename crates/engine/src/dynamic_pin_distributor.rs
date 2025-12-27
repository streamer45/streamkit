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
    /// Pre-built metric labels - allocated once in new(), reused on every packet
    metric_labels: [opentelemetry::KeyValue; 2],
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
            .build();

        // Pre-build metric labels once - avoids allocation on every packet
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
            metric_labels,
        }
    }

    /// The main loop for the Pin Distributor. Handles configuration and packet fan-out with backpressure.
    #[allow(clippy::cognitive_complexity)]
    pub(super) async fn run(mut self) {
        tracing::debug!("PinDistributorActor started for {}.{}", self.node_id, self.pin_name);

        loop {
            tokio::select! {
                // Prioritize configuration messages
                biased;

                // Handle configuration updates
                Some(msg) = self.config_rx.recv() => {
                    if !self.handle_config(msg) {
                        // Shutdown requested. Break the loop to start draining.
                        tracing::debug!(
                            "{}.{}: PinDistributor received Shutdown. Draining.",
                            self.node_id,
                            self.pin_name
                        );
                        break;
                    }
                },

                // Handle incoming packets from the node
                Some(packet) = self.data_rx.recv() => {
                    self.distribute_packet(packet).await;
                },
                else => {
                    // Data channel closed (node finished) and config channel closed.
                    tracing::debug!(
                        "{}.{}: PinDistributor inputs closed. Shutting down.",
                        self.node_id,
                        self.pin_name
                    );
                    return;
                },
            }
        }

        // Shutdown requested - exit immediately without draining.
        // During shutdown, we prioritize fast termination over packet delivery.
        // Close channels to signal upstream that we're done.
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
    #[allow(clippy::cognitive_complexity)] // Fan-out with mode handling requires multiple paths
    async fn distribute_packet(&mut self, packet: Packet) {
        use futures::stream::{FuturesUnordered, StreamExt};
        use tokio::sync::mpsc::error::TrySendError;

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
}
