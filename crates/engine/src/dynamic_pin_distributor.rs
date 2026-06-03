// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pin distributor actor for the data plane.
//!
//! The PinDistributorActor is responsible for distributing packets from a single
//! output pin to multiple downstream input pins. Supports two connection modes:
//!
//! - **Reliable**: Synchronized backpressure - waits for slow consumers
//! - **BestEffort**: Avoids backpressure; keeps the newest packet when downstream is congested.
//!   When `try_send` reports `Full`, the packet is parked in `OutputConnection::pending_best_effort`
//!   and a `reserve_owned()` future is scheduled in `pending_flushes`. As soon as the downstream
//!   channel frees a slot, the actor's `select!` flush arm wakes, takes the *current* pending
//!   packet (always the newest observed since the last successful send), and delivers it via the
//!   reserved permit. If a fresher packet has since landed via a successful `try_send`, the permit
//!   is dropped (releasing the slot) so we never deliver a stale packet on top of a newer one.

use crate::dynamic_messages::{ConnectionId, ConnectionMode, PinConfigMsg};
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::HashMap;
use std::time::Instant;
use streamkit_core::types::Packet;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::OwnedPermit;

const EWMA_ALPHA: f64 = 0.1;

/// Result yielded by an in-flight best-effort reservation: which connection
/// it belongs to, which incarnation (`generation`) of that connection
/// scheduled the reservation, and either the reserved permit or a
/// closed-channel error.
type FlushResult = (ConnectionId, u64, Result<OwnedPermit<Packet>, SendError<()>>);

/// Information about a downstream connection.
struct OutputConnection {
    tx: mpsc::Sender<Packet>,
    mode: ConnectionMode,
    /// Most recent best-effort packet observed while the downstream channel
    /// was full. Overwritten on each `Full` (older packet dropped) and
    /// cleared on any successful send (newer packet supersedes the parked
    /// one).
    pending_best_effort: Option<Packet>,
    /// True while a `reserve_owned()` future for this connection is queued
    /// in `PinDistributorActor::pending_flushes`. At most one in-flight
    /// reservation per output is enough because the actor is single-task:
    /// when the flush wakes, it always delivers the latest pending packet.
    flush_in_flight: bool,
    /// Monotonically increasing token assigned at `AddConnection` time.
    /// Each scheduled flush captures the generation of the connection that
    /// scheduled it, so a stale permit from a previous incarnation of the
    /// same `ConnectionId` (removed and later re-added) can be detected and
    /// dropped instead of misrouting a fresh packet onto the old channel.
    generation: u64,
}

/// Actor responsible for distributing packets from a single output pin (Data Plane).
pub struct PinDistributorActor {
    data_rx: mpsc::Receiver<streamkit_core::types::Packet>,
    config_rx: mpsc::Receiver<PinConfigMsg>,
    outputs: HashMap<ConnectionId, OutputConnection>,
    /// In-flight `reserve_owned()` futures for best-effort outputs whose
    /// downstream channel was full when a packet arrived. Each future
    /// resolves once the channel has capacity again; the `run()` flush arm
    /// then drains the connection's `pending_best_effort` slot through the
    /// reserved permit.
    pending_flushes: FuturesUnordered<BoxFuture<'static, FlushResult>>,
    next_generation: u64,
    node_id: String,
    pin_name: String,
    packets_distributed_counter: opentelemetry::metrics::Counter<u64>,
    packets_dropped_counter: opentelemetry::metrics::Counter<u64>,
    best_effort_drops_counter: opentelemetry::metrics::Counter<u64>,
    outputs_active_gauge: opentelemetry::metrics::Gauge<u64>,
    send_wait_histogram: opentelemetry::metrics::Histogram<f64>,
    queue_depth_gauge: opentelemetry::metrics::Gauge<u64>,
    queue_depth_bytes_gauge: opentelemetry::metrics::Gauge<u64>,
    queue_depth_seconds_gauge: opentelemetry::metrics::Gauge<f64>,
    /// `[node_id, pin_name]` plus the node's bounded pipeline attributes.
    metric_labels: Vec<opentelemetry::KeyValue>,
    avg_packet_size_bytes: f64,
    avg_packet_duration_s: f64,
}

/// Estimate the serialized JSON byte length of a `serde_json::Value` without
/// allocating a temporary `String`.
//
// `pub(crate)` is technically redundant because the containing module is
// private, but stating the intended visibility documents that this helper
// is deliberately consumed from the sibling `tests/` module rather than
// from outside the crate.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn json_byte_len(value: &serde_json::Value) -> usize {
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
    pub(super) fn new(
        data_rx: mpsc::Receiver<streamkit_core::types::Packet>,
        config_rx: mpsc::Receiver<PinConfigMsg>,
        node_id: String,
        pin_name: String,
        attributes: Vec<opentelemetry::KeyValue>,
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

        let mut metric_labels = vec![
            KeyValue::new("node_id", node_id.clone()),
            KeyValue::new("pin_name", pin_name.clone()),
        ];
        metric_labels.extend(attributes);
        outputs_active_gauge.record(0, &metric_labels);

        Self {
            data_rx,
            config_rx,
            outputs: HashMap::new(),
            pending_flushes: FuturesUnordered::new(),
            next_generation: 0,
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

        // Per-input liveness flags so closed receivers do not keep the loop
        // spinning on `Ready(None)` (which would never satisfy `Some(_) = ...`
        // and could deadlock when `pending_flushes` still has a long-lived
        // reservation against a downstream that never frees capacity).
        let mut config_open = true;
        let mut data_open = true;

        loop {
            tokio::select! {
                biased;

                msg = self.config_rx.recv(), if config_open => {
                    if let Some(m) = msg {
                        if !self.handle_config(m) {
                            tracing::debug!(
                                "{}.{}: PinDistributor received Shutdown. Draining.",
                                self.node_id,
                                self.pin_name
                            );
                            break;
                        }
                    } else {
                        config_open = false;
                        if !data_open {
                            tracing::debug!(
                                "{}.{}: PinDistributor inputs closed. Shutting down.",
                                self.node_id,
                                self.pin_name
                            );
                            break;
                        }
                    }
                },

                packet = self.data_rx.recv(), if data_open => {
                    if let Some(p) = packet {
                        self.distribute_packet(p).await;
                    } else {
                        data_open = false;
                        if !config_open {
                            tracing::debug!(
                                "{}.{}: PinDistributor inputs closed. Shutting down.",
                                self.node_id,
                                self.pin_name
                            );
                            break;
                        }
                    }
                },

                Some((id, generation, permit_result)) = self.pending_flushes.next(),
                    if !self.pending_flushes.is_empty() => {
                    self.handle_flush_completion(&id, generation, permit_result);
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

    /// Returns `false` on shutdown.
    fn handle_config(&mut self, msg: PinConfigMsg) -> bool {
        match msg {
            PinConfigMsg::AddConnection { id, tx, mode } => {
                let generation = self.next_generation;
                self.next_generation = self.next_generation.wrapping_add(1);
                self.outputs.insert(
                    id,
                    OutputConnection {
                        tx,
                        mode,
                        pending_best_effort: None,
                        flush_in_flight: false,
                        generation,
                    },
                );
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

    #[allow(
        clippy::cognitive_complexity,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )] // Fan-out with mode handling requires multiple paths and metric estimation casts
    async fn distribute_packet(&mut self, packet: Packet) {
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
            self.packets_dropped_counter.add(1, &self.metric_labels);
            return;
        }

        // Single-output fast path: avoids cloning the packet.
        if self.outputs.len() == 1 {
            if matches!(
                self.outputs.values().next().map(|c| &c.mode),
                Some(ConnectionMode::BestEffort)
            ) {
                let Some((id, conn)) = self.outputs.iter_mut().next() else {
                    tracing::error!(
                        "{}.{}: Outputs unexpectedly empty despite len() == 1",
                        self.node_id,
                        self.pin_name
                    );
                    return;
                };

                match conn.tx.try_send(packet) {
                    Ok(()) => {
                        if conn.pending_best_effort.take().is_some() {
                            self.best_effort_drops_counter.add(1, &self.metric_labels);
                        }
                        self.packets_distributed_counter.add(1, &self.metric_labels);
                    },
                    Err(TrySendError::Full(packet)) => {
                        if conn.pending_best_effort.is_some() {
                            self.best_effort_drops_counter.add(1, &self.metric_labels);
                        }
                        conn.pending_best_effort = Some(packet);
                        if !conn.flush_in_flight {
                            conn.flush_in_flight = true;
                            let tx = conn.tx.clone();
                            let id_owned = id.clone();
                            let generation = conn.generation;
                            self.pending_flushes.push(Box::pin(async move {
                                let result = tx.reserve_owned().await;
                                (id_owned, generation, result)
                            }));
                        }
                    },
                    Err(TrySendError::Closed(_packet)) => {
                        let id = id.clone();
                        tracing::warn!(
                            "{}.{}: Downstream connection {} closed.",
                            self.node_id,
                            self.pin_name,
                            id
                        );
                        self.remove_output(&id);
                    },
                }
            } else {
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
                            self.remove_output(&id);
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
                        self.remove_output(&id);
                    },
                }
            }
            return;
        }

        let mut successes = 0u64;
        let mut best_effort_drops = 0u64;
        let mut to_remove: Vec<ConnectionId> = Vec::new();
        let mut to_schedule_flush: Vec<(ConnectionId, u64, mpsc::Sender<Packet>)> = Vec::new();
        let mut pending = FuturesUnordered::new();

        for (id, conn) in &mut self.outputs {
            match conn.mode {
                ConnectionMode::BestEffort => match conn.tx.try_send(packet.clone()) {
                    Ok(()) => {
                        if conn.pending_best_effort.take().is_some() {
                            best_effort_drops += 1;
                        }
                        successes += 1;
                    },
                    Err(TrySendError::Full(packet_clone)) => {
                        if conn.pending_best_effort.is_some() {
                            best_effort_drops += 1;
                        }
                        conn.pending_best_effort = Some(packet_clone);
                        if !conn.flush_in_flight {
                            conn.flush_in_flight = true;
                            to_schedule_flush.push((id.clone(), conn.generation, conn.tx.clone()));
                        }
                    },
                    Err(TrySendError::Closed(_packet_clone)) => {
                        to_remove.push(id.clone());
                    },
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

        while let Some((id, waited_secs, result)) = pending.next().await {
            self.send_wait_histogram.record(waited_secs, &self.metric_labels);
            if result.is_err() {
                to_remove.push(id);
            } else {
                successes += 1;
            }
        }

        for id in &to_remove {
            tracing::warn!(
                "{}.{}: Downstream connection {} closed during fan-out.",
                self.node_id,
                self.pin_name,
                id
            );
            self.remove_output(id);
        }

        if successes > 0 {
            self.packets_distributed_counter.add(successes, &self.metric_labels);
        }
        if best_effort_drops > 0 {
            self.best_effort_drops_counter.add(best_effort_drops, &self.metric_labels);
        }

        for (id, generation, tx) in to_schedule_flush {
            self.pending_flushes.push(Box::pin(async move {
                let result = tx.reserve_owned().await;
                (id, generation, result)
            }));
        }
    }

    fn remove_output(&mut self, id: &ConnectionId) {
        self.outputs.remove(id);
        self.outputs_active_gauge.record(self.outputs.len() as u64, &self.metric_labels);
    }

    /// Deliver the newest pending best-effort packet through the permit,
    /// or release the permit if a fresher packet already went through.
    fn handle_flush_completion(
        &mut self,
        id: &ConnectionId,
        generation: u64,
        permit_result: Result<OwnedPermit<Packet>, SendError<()>>,
    ) {
        let Some(conn) = self.outputs.get_mut(id) else {
            // Connection was removed (RemoveConnection / Closed) while the
            // reservation was in flight. Dropping `permit_result` releases
            // any reserved slot back to the channel (the permit is bound to
            // the now-defunct Sender clone).
            return;
        };
        if conn.generation != generation {
            // Stale: this reservation belongs to a previous incarnation of
            // the same ConnectionId that was removed and re-added. The
            // permit is bound to the OLD channel; touching `conn` here
            // (delivering its pending packet through the old permit, or
            // resetting its flush_in_flight which guards the NEW reservation)
            // would misroute data. Drop the permit silently.
            return;
        }
        conn.flush_in_flight = false;
        if let Ok(permit) = permit_result {
            if let Some(packet) = conn.pending_best_effort.take() {
                permit.send(packet);
                self.packets_distributed_counter.add(1, &self.metric_labels);
            }
            // No pending packet: a newer try_send cleared it. Dropping
            // `permit` here releases the reserved slot back to the channel.
        } else {
            tracing::warn!(
                "{}.{}: BestEffort downstream {} closed while flush pending; removing.",
                self.node_id,
                self.pin_name,
                id
            );
            self.remove_output(id);
        }
    }

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
