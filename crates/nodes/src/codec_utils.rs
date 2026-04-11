// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared async select-loop for codec-style nodes.
//!
//! Many nodes follow the same pattern: a blocking task produces
//! `Result<T, String>` items, an input task feeds packets into the blocking
//! task, and an async select-loop forwards results to the output sender while
//! handling shutdown and input completion.  [`codec_forward_loop`] captures
//! this pattern so individual nodes don't need to duplicate it.

use opentelemetry::KeyValue;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::Packet;
use streamkit_core::NodeContext;
use tokio::sync::mpsc;

/// Shared select-loop that forwards codec results to the output sender.
///
/// Handles three concurrent events:
/// 1. Results arriving from the blocking codec task.
/// 2. Shutdown control messages.
/// 3. Input task completion (triggers drain of remaining results).
///
/// `to_packet` converts a codec-specific result `T` into a [`Packet`].
#[allow(clippy::too_many_arguments)]
pub async fn codec_forward_loop<T: Send + 'static, S: Send>(
    context: &mut NodeContext,
    result_rx: &mut mpsc::Receiver<Result<T, String>>,
    input_task: &mut tokio::task::JoinHandle<()>,
    codec_task: tokio::task::JoinHandle<()>,
    codec_tx: mpsc::Sender<S>,
    counter: &opentelemetry::metrics::Counter<u64>,
    stats: &mut NodeStatsTracker,
    to_packet: impl Fn(T) -> Packet,
    label: &str,
) {
    /// Forwards a single successful codec result to the output sender.
    /// Returns `true` if the output channel is closed (caller should break).
    async fn forward_one(
        packet: Packet,
        context: &mut NodeContext,
        counter: &opentelemetry::metrics::Counter<u64>,
        stats: &mut NodeStatsTracker,
    ) -> bool {
        counter.add(1, &[KeyValue::new("status", "ok")]);
        stats.received();
        if context.output_sender.send("out", packet).await.is_err() {
            tracing::debug!("Output channel closed, stopping node");
            return true;
        }
        stats.sent();
        stats.maybe_send();
        false
    }

    /// Handles a codec error result by updating counters and logging.
    fn handle_error(
        err: &str,
        counter: &opentelemetry::metrics::Counter<u64>,
        stats: &mut NodeStatsTracker,
        label: &str,
    ) {
        counter.add(1, &[KeyValue::new("status", "error")]);
        stats.received();
        stats.errored();
        stats.maybe_send();
        tracing::warn!("{label} codec error: {err}");
    }

    let mut drain_pending = false;

    loop {
        tokio::select! {
            maybe_result = result_rx.recv() => {
                match maybe_result {
                    Some(Ok(item)) => {
                        if forward_one(to_packet(item), context, counter, stats).await {
                            break;
                        }
                    }
                    Some(Err(err)) => handle_error(&err, counter, stats, label),
                    None => break,
                }
            }
            Some(control_msg) = context.control_rx.recv() => {
                if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                    tracing::info!("{label} received shutdown signal");
                    // NOTE: Dropping codec_tx first signals the codec thread to
                    // exit/flush, then aborting ensures it doesn't linger.
                    // Because we break out here, flushed results are never sent
                    // downstream.  Data loss on explicit shutdown is acceptable.
                    input_task.abort();
                    drop(codec_tx);
                    codec_task.abort();
                    break;
                }
            }
            _ = &mut *input_task => {
                tracing::debug!("{label} input task completed, starting drain");
                drop(codec_tx);
                drain_pending = true;
                break;
            }
        }
    }

    if drain_pending {
        // Wait for the codec task to finish producing all results before
        // draining.  Without this, the drain loop interleaves with the
        // (potentially slow) blocking encode task, and downstream nodes
        // may close their channels before all results are forwarded —
        // causing zero-byte output on fast pipelines.
        tracing::debug!("{label} waiting for codec task to finish before drain");
        match codec_task.await {
            Err(e) if e.is_panic() => {
                tracing::error!("{label} codec task panicked: {e:?}");
            },
            _ => {
                let mut drained = 0u64;
                while let Some(maybe_result) = result_rx.recv().await {
                    match maybe_result {
                        Ok(item) => {
                            drained += 1;
                            if forward_one(to_packet(item), context, counter, stats).await {
                                break;
                            }
                        },
                        Err(err) => handle_error(&err, counter, stats, label),
                    }
                }
                tracing::debug!("{label} drain complete: forwarded {drained} result(s)");
            },
        }
    } else {
        codec_task.abort();
        match codec_task.await {
            Err(e) if e.is_panic() => {
                tracing::error!("{label} codec task panicked: {e:?}");
            },
            _ => {},
        }
    }
}
