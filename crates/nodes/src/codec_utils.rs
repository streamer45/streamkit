// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use opentelemetry::KeyValue;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::Packet;
use streamkit_core::{state_helpers, NodeContext, NodeStateUpdate, StreamKitError};
use tokio::sync::mpsc;

/// Bounds a codec's entire lifetime by output idleness.
///
/// A codec task can wedge in an uninterruptible blocking FFI call (e.g.
/// SVT-AV1's two-thread `send_picture`/`get_packet` deadlock) mid-stream or
/// during a flush, going permanently silent. If results stall for this long
/// while the codec is *demonstrably* stuck — its input channel is full (it has
/// stopped consuming) or input has already closed (it should be flushing) —
/// [`codec_forward_loop`] abandons the task so the node finalizes instead of
/// hanging to the client request timeout. The "stuck" guard keeps a merely
/// idle or slow input (whose channel drains to empty) from tripping it. Must
/// stay larger than any in-task flush bound (SVT-AV1's
/// `RECEIVE_THREAD_JOIN_TIMEOUT`) so a flush that recovers within its own
/// budget is not pre-empted (enforced at compile time, see
/// `SvtAv1EncoderNode::RECEIVE_THREAD_JOIN_TIMEOUT`).
pub(crate) const CODEC_IDLE_TIMEOUT: Duration = Duration::from_mins(1);

/// How a [`codec_forward_loop`] run terminated.
///
/// Distinguishes a clean finish (input/codec/downstream closed, or shutdown)
/// from a degraded one that leaves the output truncated — an idle-watchdog
/// abandonment or a codec-task panic. A degraded run must surface as a node
/// failure rather than a successful `Stopped("input_closed")` so callers and
/// state subscribers can tell it from a complete one (see #539).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CodecLoopOutcome {
    /// The loop finalized cleanly: the codec finished, input or the downstream
    /// channel closed, or a shutdown was requested.
    Completed,
    /// The idle watchdog abandoned a wedged codec worker mid-stream or
    /// mid-flush. Any output already forwarded is truncated, and the worker's
    /// native handle was intentionally leaked (it may still be in a blocking
    /// FFI call), so the run is degraded, not successful.
    WatchdogAbandoned,
    /// The codec task panicked, so the loop ended before the stream was fully
    /// processed. Any output already forwarded is truncated, exactly like a
    /// watchdog abandonment, so the run is degraded, not successful.
    CodecPanicked,
}

/// Emit the terminal node state and return the run result for a codec node,
/// based on how its [`codec_forward_loop`] ended.
///
/// A clean finish emits `Stopped("input_closed")` and returns `Ok(())`. A
/// degraded finish (watchdog abandonment or codec panic) emits a terminal
/// `Failed` state and returns an `Err`, making the truncated output observable
/// to callers and state subscribers instead of masquerading as a successful
/// run (#539).
///
/// # Errors
///
/// Returns [`StreamKitError::Codec`] when `outcome` is
/// [`CodecLoopOutcome::WatchdogAbandoned`] or [`CodecLoopOutcome::CodecPanicked`],
/// i.e. the codec was abandoned or panicked and its output is truncated.
pub fn finalize_codec_run(
    outcome: CodecLoopOutcome,
    state_tx: &mpsc::Sender<NodeStateUpdate>,
    node_id: &str,
    label: &str,
) -> Result<(), StreamKitError> {
    let reason = match outcome {
        CodecLoopOutcome::Completed => {
            state_helpers::emit_stopped(state_tx, node_id, "input_closed");
            tracing::info!("{label} finished");
            return Ok(());
        },
        CodecLoopOutcome::WatchdogAbandoned => format!(
            "{label} abandoned a wedged codec worker after {CODEC_IDLE_TIMEOUT:?} of output \
             idleness; output is truncated"
        ),
        CodecLoopOutcome::CodecPanicked => {
            format!("{label} codec task panicked mid-stream; output is truncated")
        },
    };
    tracing::error!("{reason}");
    state_helpers::emit_failed(state_tx, node_id, reason.clone());
    Err(StreamKitError::Codec(reason))
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
/// Some encoders (e.g. SVT-AV1) flush through a two-thread native boundary that
/// can deadlock under rare scheduling, and a plain
/// [`std::thread::JoinHandle::join`] there is unbounded — it can hang the codec
/// task until the client request times out. This bounds the wait so the codec
/// task always finalizes.
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
            if handle.join().is_err() {
                tracing::error!("bounded thread join: worker panicked before signaling completion");
            }
            ThreadJoin::Joined
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => ThreadJoin::Abandoned,
    }
}

/// Forward codec results downstream until the codec finishes, input closes, or
/// the output channel goes away.
///
/// A single idle watchdog ([`CODEC_IDLE_TIMEOUT`]) bounds the codec's whole
/// lifetime: if it wedges in a blocking FFI call and stops producing results,
/// the task is abandoned so the node finalizes instead of hanging.
// clippy::too_many_arguments: these are the codec loop's distinct state
// handles (channels, task handles, metrics); bundling them into a struct
// would only obscure the call sites.
#[allow(clippy::too_many_arguments)]
pub async fn codec_forward_loop<T: Send + 'static, S: Send>(
    context: &mut NodeContext,
    result_rx: &mut mpsc::Receiver<Result<T, String>>,
    input_task: &mut tokio::task::JoinHandle<()>,
    mut codec_task: tokio::task::JoinHandle<()>,
    codec_tx: mpsc::Sender<S>,
    counter: &opentelemetry::metrics::Counter<u64>,
    stats: &mut NodeStatsTracker,
    to_packet: impl Fn(T) -> Packet + Send + Sync,
    label: &str,
) -> CodecLoopOutcome {
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

    let mut input_done = false;
    let mut codec_done = false;
    let mut outcome = CodecLoopOutcome::Completed;
    // Held in an `Option` so the idle watchdog can observe the codec's input
    // backpressure; taken (dropped) to signal end-of-input to the codec.
    let mut codec_tx = Some(codec_tx);

    let idle = tokio::time::sleep(CODEC_IDLE_TIMEOUT);
    tokio::pin!(idle);

    loop {
        tokio::select! {
            biased;
            // Forward results first so active output always wins over the idle
            // watchdog and keeps unblocking the codec's `blocking_send()`.
            maybe_result = result_rx.recv() => {
                let Some(result) = maybe_result else { break };
                idle.as_mut().reset(tokio::time::Instant::now() + CODEC_IDLE_TIMEOUT);
                match result {
                    Ok(item) => {
                        if forward_one(to_packet(item), context, counter, stats).await {
                            break;
                        }
                    }
                    Err(err) => handle_error(&err, counter, stats, label),
                }
            }
            Some(control_msg) = context.control_rx.recv() => {
                if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                    tracing::info!("{label} received shutdown signal");
                    break;
                }
            }
            _ = &mut *input_task, if !input_done => {
                tracing::debug!("{label} input task completed, flushing codec");
                input_done = true;
                codec_tx = None;
                // Give the EOS flush a full idle budget rather than whatever
                // residual time the timer happened to hold; otherwise a healthy
                // flush could be abandoned within seconds, breaking the
                // `CODEC_IDLE_TIMEOUT > RECEIVE_THREAD_JOIN_TIMEOUT` budget.
                idle.as_mut().reset(tokio::time::Instant::now() + CODEC_IDLE_TIMEOUT);
            }
            res = &mut codec_task, if !codec_done => {
                codec_done = true;
                if let Err(e) = res {
                    if e.is_panic() {
                        tracing::error!("{label} codec task panicked: {e:?}");
                        outcome = CodecLoopOutcome::CodecPanicked;
                        break;
                    }
                }
                // The codec task may have abandoned a receive thread during a
                // bounded flush (SVT-AV1 teardown): that detached thread is
                // wedged in a native call and never drops its `result_tx`
                // clone, so `recv()` would block forever. Closing forwards any
                // buffered results and then returns `None`.
                result_rx.close();
            }
            () = &mut idle, if !codec_done => {
                let codec_input_full = codec_tx.as_ref().is_some_and(|tx| tx.capacity() == 0);
                if input_done || codec_input_full {
                    tracing::error!(
                        "{label} produced no output for {CODEC_IDLE_TIMEOUT:?} while the codec \
                         was stuck (input {}); abandoning the wedged codec worker and \
                         finalizing instead of hanging (likely native encoder deadlock)",
                        if input_done { "closed" } else { "backed up" }
                    );
                    outcome = CodecLoopOutcome::WatchdogAbandoned;
                    break;
                }
                idle.as_mut().reset(tokio::time::Instant::now() + CODEC_IDLE_TIMEOUT);
            }
        }
    }

    input_task.abort();
    // Drop the input sender (if still held) so a healthy codec sees EOF, then
    // abandon a codec task that hasn't finished. A wedged blocking task cannot
    // be interrupted; `abort()` detaches it so the node finalizes, and the
    // task keeps ownership of any native handle it is mid-call on so nothing
    // is freed underneath it (the #539 leak tradeoff).
    drop(codec_tx.take());
    if !codec_done {
        codec_task.abort();
    }

    outcome
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

    /// The normal path must terminate: the codec finishes and closes its
    /// channel, so the loop finalizes promptly.
    #[tokio::test]
    async fn forward_loop_finalizes_when_codec_finishes_and_channel_closes() {
        let (mut ctx, _mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        result_tx.send(Ok(vec![9])).await.unwrap();
        drop(result_tx);

        let mut input_task = tokio::task::spawn(std::future::pending::<()>());
        let codec_task = tokio::task::spawn(async {});
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("loop must finalize when the codec finishes and closes the channel");

        assert_eq!(
            outcome,
            CodecLoopOutcome::Completed,
            "a clean finish must report Completed, not a watchdog abandonment"
        );
    }

    /// Regression for #540: an SVT-AV1 dimension change can abandon a wedged
    /// receive thread, leaking its `result_tx` clone, while the codec task
    /// still finishes cleanly. The loop must finalize anyway — forwarding
    /// buffered results, then closing the receiver on codec completion so the
    /// leaked sender can't keep `recv()` pending forever.
    #[tokio::test]
    async fn forward_loop_finalizes_despite_leaked_sender() {
        let (mut ctx, mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        result_tx.send(Ok(vec![4, 2])).await.unwrap();
        // The clone an abandoned receive thread would hold forever; the codec
        // task finishes without it ever being dropped.
        let _leaked = result_tx.clone();
        drop(result_tx);

        let mut input_task = tokio::task::spawn(std::future::pending::<()>());
        let codec_task = tokio::task::spawn(async {});
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("loop must finalize once the codec finishes, even with a leaked sender");

        assert_eq!(
            outcome,
            CodecLoopOutcome::Completed,
            "a leaked sender that the codec finishes around is still a clean finish"
        );
        assert!(
            matches!(mock_out.try_recv().await, Some((_, _, Packet::Binary { .. }))),
            "buffered result must still be forwarded before finalizing"
        );
    }

    /// Regression for #540: a codec wedged *mid-stream* (no output, input
    /// backed up, task never completes) must be abandoned by the idle watchdog
    /// so the node finalizes instead of hanging to the request timeout.
    #[tokio::test(start_paused = true)]
    async fn forward_loop_abandons_codec_wedged_midstream() {
        let (mut ctx, _mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        // The sender stays alive and never sends, so `recv()` never returns
        // `None` on its own — only the watchdog can end the loop.
        let (_result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);

        let mut input_task = tokio::task::spawn(std::future::pending::<()>());
        let codec_task = tokio::task::spawn(std::future::pending::<()>());

        // A full input channel is the "codec stopped consuming" signal the
        // watchdog keys on while input is still open.
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);
        codec_tx.try_send(vec![0]).unwrap();
        assert_eq!(codec_tx.capacity(), 0);

        let outcome = tokio::time::timeout(
            Duration::from_mins(10),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("idle watchdog must abandon the mid-stream-wedged codec instead of hanging");

        assert_eq!(
            outcome,
            CodecLoopOutcome::WatchdogAbandoned,
            "a watchdog-abandoned codec must surface as a degraded run, not a clean finish"
        );
    }

    /// The idle watchdog also bounds a flush that never completes: once input
    /// has closed, a codec that produces no further output (wedged in its EOS
    /// flush) is abandoned rather than awaited forever.
    #[tokio::test(start_paused = true)]
    async fn forward_loop_abandons_codec_wedged_after_input_close() {
        let (mut ctx, _mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (_result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);

        // Input completes immediately; the codec task is wedged in its flush
        // and never finishes.
        let mut input_task = tokio::task::spawn(async {});
        let codec_task = tokio::task::spawn(std::future::pending::<()>());
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);

        let outcome = tokio::time::timeout(
            Duration::from_mins(10),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("idle watchdog must finalize a wedged post-input-close flush");

        assert_eq!(
            outcome,
            CodecLoopOutcome::WatchdogAbandoned,
            "an EOS flush abandoned by the watchdog is a truncated, degraded run"
        );
    }

    /// Regression for #540: closing input must re-arm the idle watchdog to a
    /// full budget. Input here goes silent for nearly a whole idle period
    /// before closing, leaving the timer with ~1s residual; the EOS flush then
    /// emits a packet 2s after close. Without the re-arm the watchdog fires on
    /// the stale residual and abandons a healthy flush, dropping that packet;
    /// with it, the flush gets a full budget and the packet is forwarded.
    #[tokio::test(start_paused = true)]
    async fn forward_loop_rearms_idle_budget_on_input_close() {
        let (mut ctx, mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        let (result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        tokio::task::spawn(async move {
            tokio::time::sleep(CODEC_IDLE_TIMEOUT + Duration::from_secs(1)).await;
            let _ = result_tx.send(Ok(vec![7])).await;
        });

        // No output while input is open, then input closes ~1s before the
        // watchdog's first deadline (residual ≈ 1s).
        let mut input_task = tokio::task::spawn(async {
            tokio::time::sleep(CODEC_IDLE_TIMEOUT.saturating_sub(Duration::from_secs(1))).await;
        });
        let codec_task = tokio::task::spawn(std::future::pending::<()>());
        // Input channel stays non-full, so only the watchdog can end the loop
        // early — exactly the path the re-arm must lengthen.
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);

        let outcome = tokio::time::timeout(
            Duration::from_mins(10),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("loop must finalize");

        assert_eq!(
            outcome,
            CodecLoopOutcome::Completed,
            "a healthy flush that completes within its re-armed budget is a clean finish"
        );
        assert!(
            matches!(mock_out.try_recv().await, Some((_, _, Packet::Binary { .. }))),
            "a flush packet emitted after input close must be forwarded, not pre-empted \
             by the watchdog firing on a stale residual budget"
        );
    }

    /// Regression for #540: a thread wedged in a blocking native flush (never
    /// signals completion) must be abandoned once the budget elapses, not
    /// joined forever. This is the single bound for every SVT-AV1 flush
    /// (mid-stream dimension change, downstream-close, input-close).
    #[test]
    fn bounded_thread_join_abandons_wedged_worker() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Stands in for a thread wedged in a blocking native call the join
            // can't interrupt. `park()` can wake spuriously, so loop to stay
            // parked deterministically; holding `done_tx` keeps `done_rx` from
            // disconnecting, so the join can only end via the timeout.
            let _done_tx = done_tx;
            loop {
                std::thread::park();
            }
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

    /// #539: a clean finish emits `Stopped("input_closed")` and returns `Ok`,
    /// so a normal end of input is reported as success.
    #[test]
    fn finalize_maps_clean_finish_to_stopped_ok() {
        let (state_tx, mut state_rx) = mpsc::channel::<NodeStateUpdate>(4);

        let result = finalize_codec_run(CodecLoopOutcome::Completed, &state_tx, "node", "Label");

        assert!(result.is_ok(), "a clean finish must return Ok");
        let update = state_rx.try_recv().expect("a terminal state must be emitted");
        assert!(
            matches!(update.state, streamkit_core::NodeState::Stopped { .. }),
            "a clean finish must emit Stopped, got {:?}",
            update.state
        );
    }

    /// #539: a watchdog abandonment emits a terminal `Failed` state and returns
    /// `Err`, so a truncated encode is programmatically distinguishable from a
    /// complete one instead of masquerading as `Stopped("input_closed")` + `Ok`.
    #[test]
    fn finalize_maps_watchdog_abandonment_to_failed_err() {
        let (state_tx, mut state_rx) = mpsc::channel::<NodeStateUpdate>(4);

        let result =
            finalize_codec_run(CodecLoopOutcome::WatchdogAbandoned, &state_tx, "node", "Label");

        assert!(
            matches!(result, Err(StreamKitError::Codec(_))),
            "a watchdog-abandoned run must return a codec error, not Ok"
        );
        let update = state_rx.try_recv().expect("a terminal state must be emitted");
        assert!(
            matches!(update.state, streamkit_core::NodeState::Failed { .. }),
            "a truncated encode must surface as Failed, got {:?}",
            update.state
        );
    }

    /// #539: a codec-task panic truncates output exactly like a watchdog
    /// abandonment, so it must surface as `Failed` + `Err`, not a clean stop.
    #[test]
    fn finalize_maps_codec_panic_to_failed_err() {
        let (state_tx, mut state_rx) = mpsc::channel::<NodeStateUpdate>(4);

        let result =
            finalize_codec_run(CodecLoopOutcome::CodecPanicked, &state_tx, "node", "Label");

        assert!(
            matches!(result, Err(StreamKitError::Codec(_))),
            "a panicked codec run must return a codec error, not Ok"
        );
        let update = state_rx.try_recv().expect("a terminal state must be emitted");
        assert!(
            matches!(update.state, streamkit_core::NodeState::Failed { .. }),
            "a panicked codec run must surface as Failed, got {:?}",
            update.state
        );
    }

    /// #539: a panicking codec task must end the loop with `CodecPanicked`, not
    /// the default `Completed` — otherwise a panic-truncated stream would be
    /// finalized as a clean `Stopped("input_closed")` success.
    #[tokio::test]
    async fn forward_loop_reports_panic_as_degraded() {
        let (mut ctx, _mock_out, _state_rx) = create_test_context(HashMap::new(), 1);
        let mut stats = NodeStatsTracker::new("test".to_string(), ctx.stats_tx.clone());
        let counter = test_counter();

        // No results ever arrive; the codec task panics, which must end the loop.
        let (_result_tx, mut result_rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
        let mut input_task = tokio::task::spawn(std::future::pending::<()>());
        let codec_task = tokio::task::spawn(async { panic!("codec worker blew up") });
        let (codec_tx, _codec_rx) = mpsc::channel::<Vec<u8>>(1);

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            codec_forward_loop(
                &mut ctx,
                &mut result_rx,
                &mut input_task,
                codec_task,
                codec_tx,
                &counter,
                &mut stats,
                to_binary,
                "test",
            ),
        )
        .await
        .expect("loop must finalize when the codec task panics");

        assert_eq!(
            outcome,
            CodecLoopOutcome::CodecPanicked,
            "a panicked codec task must surface as a degraded run, not a clean finish"
        );
    }
}
