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

/// Drain remaining codec results while concurrently awaiting the codec task.
///
/// We must keep reading from `result_rx` while the codec task is still
/// running because the codec task uses `blocking_send()` on a bounded
/// channel (capacity 32).  If we awaited the codec task first (without
/// draining), the channel would fill up and the codec task would block
/// forever — a deadlock.
///
/// Once the codec task finishes, `result_tx` is dropped, so
/// `result_rx.recv()` will eventually return `None` and we exit the
/// drain loop naturally with all results forwarded.
async fn drain_codec_results<T: Send + 'static, F: Fn(T) -> Packet + Send + Sync>(
    result_rx: &mut mpsc::Receiver<Result<T, String>>,
    mut codec_task: tokio::task::JoinHandle<()>,
    context: &mut NodeContext,
    counter: &opentelemetry::metrics::Counter<u64>,
    stats: &mut NodeStatsTracker,
    to_packet: &F,
    label: &str,
) {
    let mut codec_done = false;
    let mut drained = 0u64;
    loop {
        tokio::select! {
            biased;
            // Drain results first (biased) to keep the channel
            // flowing and unblock the codec task's blocking_send().
            maybe_result = result_rx.recv() => {
                match maybe_result {
                    Some(Ok(item)) => {
                        drained += 1;
                        counter.add(1, &[KeyValue::new("status", "ok")]);
                        stats.received();
                        if context.output_sender.send("out", to_packet(item)).await.is_err() {
                            tracing::debug!("Output channel closed during drain");
                            break;
                        }
                        stats.sent();
                        stats.maybe_send();
                    }
                    Some(Err(err)) => {
                        counter.add(1, &[KeyValue::new("status", "error")]);
                        stats.received();
                        stats.errored();
                        stats.maybe_send();
                        tracing::warn!("{label} codec error: {err}");
                    }
                    None => break,  // channel closed — codec task dropped result_tx
                }
            }
            res = &mut codec_task, if !codec_done => {
                codec_done = true;
                if let Err(e) = res {
                    if e.is_panic() {
                        tracing::error!("{label} codec task panicked: {e:?}");
                        break;
                    }
                }
                // Codec task finished — result_tx is dropped.
                // Continue draining any buffered results until recv() returns None.
            }
        }
    }
    tracing::debug!("{label} drain complete: forwarded {drained} result(s)");
}

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
    to_packet: impl Fn(T) -> Packet + Send + Sync,
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
        tracing::debug!("{label} waiting for codec task to finish before drain");
        drain_codec_results(result_rx, codec_task, context, counter, stats, &to_packet, label)
            .await;
    } else {
        // Abort before awaiting: the codec task may be blocked on
        // `result_tx.blocking_send()` with a full channel since nobody
        // is draining `result_rx` anymore (output closed or channel
        // dropped).  Without the abort this would deadlock.
        codec_task.abort();
        let _ = codec_task.await;
    }
}
