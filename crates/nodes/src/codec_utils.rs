// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use opentelemetry::KeyValue;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::Packet;
use streamkit_core::NodeContext;
use tokio::sync::mpsc;

/// Must drain `result_rx` concurrently with the codec task: the codec
/// uses `blocking_send` on a bounded channel, so awaiting the task
/// without draining would deadlock.
///
/// `flush_idle_timeout` arms a safety net for codecs that flush through a
/// blocking FFI boundary (e.g. SVT-AV1's two-thread design): if the codec
/// stops delivering results *and* never finishes for that long after input
/// close, the stuck blocking task is abandoned so the node can finalize
/// instead of hanging forever. The timer resets on every result, so a
/// healthy flush never trips it. `None` keeps the unbounded wait used by
/// every other codec.
// clippy::too_many_arguments: mirrors the shared codec-loop state handles;
// grouping them into a struct would only obscure the single call site.
#[allow(clippy::too_many_arguments)]
async fn drain_codec_results<T: Send + 'static, F: Fn(T) -> Packet + Send + Sync>(
    result_rx: &mut mpsc::Receiver<Result<T, String>>,
    mut codec_task: tokio::task::JoinHandle<()>,
    context: &mut NodeContext,
    counter: &opentelemetry::metrics::Counter<u64>,
    stats: &mut NodeStatsTracker,
    to_packet: &F,
    label: &str,
    flush_idle_timeout: Option<Duration>,
) {
    let mut codec_done = false;
    let mut drained = 0u64;
    loop {
        let idle_guard = async {
            match flush_idle_timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
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
            () = idle_guard, if !codec_done => {
                // The codec task is still running but has gone silent past the
                // idle budget — almost certainly deadlocked inside a blocking
                // FFI flush. Abandon it (the blocking OS thread cannot be
                // interrupted) so the node can finalize the output it already
                // produced instead of hanging until the request times out.
                tracing::error!(
                    "{label} codec task stalled for {flush_idle_timeout:?} during EOS flush; \
                     abandoning stuck task and finalizing (likely native encoder deadlock)"
                );
                codec_task.abort();
                break;
            }
        }
    }
    tracing::debug!("{label} drain complete: forwarded {drained} result(s)");
}

/// Result of [`bounded_thread_join`].
#[cfg(any(feature = "svt_av1", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadJoin {
    /// The worker signaled completion within the budget and was joined.
    Joined,
    /// The worker never signaled; it was left detached, so the caller must
    /// leak any resource the thread can still touch (see below).
    Abandoned,
}

/// Join a worker thread, bounded by `timeout`.
///
/// The blocking-FFI analogue of [`drain_codec_results`]'s idle watchdog: some
/// encoders (e.g. SVT-AV1) flush through a two-thread native boundary whose
/// receive thread can deadlock under rare scheduling, and a plain
/// [`std::thread::JoinHandle::join`] there is unbounded — it can hang the codec
/// task until the client request times out.
///
/// `done` is a channel the worker sends on immediately before returning; a
/// disconnect (e.g. a panic that drops the sender) also counts as finished. If
/// the signal arrives within `timeout` the handle is joined (an already-
/// finished, non-blocking wait). Otherwise the worker is presumed wedged in a
/// blocking call that cannot be interrupted, so it is abandoned: the
/// `JoinHandle` is dropped (detaching the OS thread) and the caller MUST leak
/// any resource the detached thread can still access, to avoid a
/// use-after-free.
#[cfg(any(feature = "svt_av1", test))]
pub(crate) fn bounded_thread_join(
    done: &std::sync::mpsc::Receiver<()>,
    handle: std::thread::JoinHandle<()>,
    timeout: Duration,
) -> ThreadJoin {
    match done.recv_timeout(timeout) {
        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let _ = handle.join();
            ThreadJoin::Joined
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => ThreadJoin::Abandoned,
    }
}

/// Forward codec results downstream, draining the codec task on input close.
///
/// `flush_idle_timeout` arms an idle watchdog on the post-input drain (see
/// [`drain_codec_results`]); pass `None` for the unbounded wait used by every
/// codec that flushes promptly, or `Some` for codecs whose flush goes through
/// a blocking FFI boundary that can deadlock (e.g. SVT-AV1).
// clippy::too_many_arguments: these are the codec loop's distinct state
// handles (channels, task handles, metrics); bundling them into a struct
// would only obscure the call sites.
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
    flush_idle_timeout: Option<Duration>,
) {
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
        drain_codec_results(
            result_rx,
            codec_task,
            context,
            counter,
            stats,
            &to_packet,
            label,
            flush_idle_timeout,
        )
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // tests should fail loudly on setup/assertion errors
mod tests {
    use super::*;
    use crate::test_utils::create_test_context;
    use std::collections::HashMap;
    use streamkit_core::stats::NodeStatsTracker;

    fn test_counter() -> opentelemetry::metrics::Counter<u64> {
        opentelemetry::global::meter("test").u64_counter("test_drain_packets").build()
    }

    fn to_binary(data: Vec<u8>) -> Packet {
        Packet::Binary { data: bytes::Bytes::from(data), content_type: None, metadata: None }
    }

    /// Regression: a codec task that deadlocks inside a blocking flush (never
    /// completes, never closes its result channel) must be abandoned once the
    /// idle budget elapses — not awaited forever — while still forwarding the
    /// packets it produced before stalling.
    ///
    /// Uses paused (virtual) time so the assertion is deterministic: the only
    /// timers are the 200ms watchdog and the 5s guard, and tokio auto-advances
    /// to the watchdog first. If the watchdog regressed, the guard would fire
    /// instead and fail the test rather than hang.
    #[tokio::test(start_paused = true)]
    async fn drain_abandons_stalled_codec_within_idle_budget() {
        let (mut ctx, mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        result_tx.send(Ok(vec![1, 2, 3])).await.unwrap();
        // Hold the sender so result_rx never closes — mimics the leaked
        // receive-thread sender of a deadlocked SVT-AV1 codec task.
        let _held = result_tx;

        let codec_task = tokio::task::spawn(std::future::pending::<()>());

        let start = tokio::time::Instant::now();
        tokio::time::timeout(
            Duration::from_secs(5),
            drain_codec_results(
                &mut result_rx,
                codec_task,
                &mut ctx,
                &counter,
                &mut stats,
                &to_binary,
                "test",
                Some(Duration::from_millis(200)),
            ),
        )
        .await
        .expect("drain must return once the idle watchdog fires");

        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(1),
            "watchdog should fire at the idle budget, not the 5s guard; took {elapsed:?}"
        );
        assert!(
            matches!(mock_out.try_recv().await, Some((_, _, Packet::Binary { .. }))),
            "packet produced before the stall must still be forwarded"
        );
    }

    /// Without a watchdog (`None`), the normal path must still terminate: the
    /// codec finishes and closes its channel, so the drain returns promptly.
    #[tokio::test]
    async fn drain_returns_when_codec_finishes_and_channel_closes() {
        let (mut ctx, _mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        result_tx.send(Ok(vec![9])).await.unwrap();
        drop(result_tx);

        let codec_task = tokio::task::spawn(async {});

        tokio::time::timeout(
            Duration::from_secs(5),
            drain_codec_results(
                &mut result_rx,
                codec_task,
                &mut ctx,
                &counter,
                &mut stats,
                &to_binary,
                "test",
                None,
            ),
        )
        .await
        .expect("drain must return when the codec finishes and closes the channel");
    }

    /// Regression for #540: a receive thread wedged in a blocking native flush
    /// (never signals completion) must be abandoned once the budget elapses,
    /// not joined forever. This is the synchronous counterpart to the async
    /// EOS-flush watchdog above, and guards the SVT-AV1 dimension-change /
    /// shutdown joins that `drain_codec_results` does not cover.
    #[test]
    fn bounded_thread_join_abandons_wedged_worker() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Never unparked — stands in for a thread wedged in a blocking
            // native call that the join can't interrupt.
            std::thread::park();
            let _ = done_tx.send(());
        });

        let start = std::time::Instant::now();
        let outcome = bounded_thread_join(&done_rx, handle, Duration::from_millis(100));
        let elapsed = start.elapsed();

        assert_eq!(outcome, ThreadJoin::Abandoned);
        assert!(
            elapsed < Duration::from_secs(5),
            "must not block on the wedged join; took {elapsed:?}"
        );
    }

    /// A worker that finishes promptly is joined, not abandoned.
    #[test]
    fn bounded_thread_join_joins_completed_worker() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = done_tx.send(());
        });

        assert_eq!(
            bounded_thread_join(&done_rx, handle, Duration::from_secs(5)),
            ThreadJoin::Joined
        );
    }
}
