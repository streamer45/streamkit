// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared encoder node infrastructure.
//!
//! Video encoder nodes (VP9, AV1/rav1e, SVT-AV1) follow a nearly identical
//! `run()` architecture: initialise metrics, create channels, spawn a blocking
//! codec task, batch incoming video frames, and forward encoded results via
//! [`codec_forward_loop`](crate::codec_utils::codec_forward_loop).
//!
//! This module extracts the shared pattern into two traits:
//!
//! - [`EncoderNodeRunner`] — captures the full `run()` boilerplate that is
//!   identical across all encoder nodes.  Each encoder provides a
//!   [`spawn_codec_task`](EncoderNodeRunner::spawn_codec_task) method for the
//!   codec-specific blocking work.
//!
//! - [`StandardVideoEncoder`] — captures the single-thread
//!   `spawn_blocking(create → encode → flush)` loop used by VP9 and AV1.
//!   SVT-AV1 does **not** implement this trait because its blocking
//!   `get_packet` API requires a two-thread architecture to avoid deadlocks.

use bytes::Bytes;
use opentelemetry::global;
use std::borrow::Cow;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{Packet, PacketMetadata, PixelFormat, VideoFrame};
use streamkit_core::{
    get_codec_channel_capacity, packet_helpers, state_helpers, NodeContext, StreamKitError,
};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Encoded output packet shared across all video encoder nodes.
///
/// Replaces the three identical `EncodedPacket` structs that previously
/// existed in `vp9.rs`, `av1.rs`, and `svt_av1.rs`.
pub struct EncodedPacket {
    pub data: Bytes,
    pub metadata: Option<PacketMetadata>,
}

// ---------------------------------------------------------------------------
// Layer 1: Node-level trait (all encoder nodes)
// ---------------------------------------------------------------------------

/// Trait for video encoder nodes that use the channel + `spawn_blocking` +
/// [`codec_forward_loop`](crate::codec_utils::codec_forward_loop) architecture.
///
/// Implementors provide codec-specific constants (metric names, content type,
/// log label) and a [`spawn_codec_task`](Self::spawn_codec_task) method that
/// spawns the blocking encode work.  The shared [`run_encoder`] function
/// handles everything else: initialisation, metrics, channel setup, the
/// async input-batching task, the forward loop, and shutdown.
pub trait EncoderNodeRunner: Send + 'static {
    /// MIME-style content type for encoded output (e.g. `"video/vp9"`).
    const CONTENT_TYPE: &'static str;
    /// Human-readable node label for log messages (e.g. `"Vp9EncoderNode"`).
    const NODE_LABEL: &'static str;
    /// OTel counter name for packets processed
    /// (e.g. `"vp9_packets_processed"`).
    const PACKETS_COUNTER_NAME: &'static str;
    /// OTel histogram name for encode/send duration
    /// (e.g. `"vp9_encode_duration"`).
    const DURATION_HISTOGRAM_NAME: &'static str;

    /// Spawn the blocking codec task.
    ///
    /// VP9 and AV1 delegate to [`spawn_standard_encode_task`].
    /// SVT-AV1 provides its own two-thread implementation.
    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()>;
}

/// Run an encoder node using the shared boilerplate.
///
/// This replaces the ~140–175 lines of near-identical code in each encoder's
/// `ProcessorNode::run()` implementation.  The encoder-specific work is
/// confined to [`EncoderNodeRunner::spawn_codec_task`].
pub async fn run_encoder<E: EncoderNodeRunner>(
    encoder: E,
    mut context: NodeContext,
) -> Result<(), StreamKitError> {
    let node_name = context.output_sender.node_name().to_string();
    state_helpers::emit_initializing(&context.state_tx, &node_name);

    tracing::info!("{} starting", E::NODE_LABEL);
    let mut input_rx = context.take_input("in")?;

    // ── Metrics ──────────────────────────────────────────────────────────
    let meter = global::meter("skit_nodes");
    let packets_processed_counter = meter.u64_counter(E::PACKETS_COUNTER_NAME).build();
    let encode_duration_histogram = meter
        .f64_histogram(E::DURATION_HISTOGRAM_NAME)
        .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
        .build();

    // ── Channels ─────────────────────────────────────────────────────────
    let (encode_tx, encode_rx) =
        mpsc::channel::<(VideoFrame, Option<PacketMetadata>)>(get_codec_channel_capacity());
    let (result_tx, mut result_rx) =
        mpsc::channel::<Result<EncodedPacket, String>>(get_codec_channel_capacity());

    // ── Codec task ───────────────────────────────────────────────────────
    let encode_task = encoder.spawn_codec_task(encode_rx, result_tx, encode_duration_histogram);

    // ── State transition ─────────────────────────────────────────────────
    state_helpers::emit_running(&context.state_tx, &node_name);
    let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
    let batch_size = context.batch_size;

    // ── Input task ───────────────────────────────────────────────────────
    let encode_tx_clone = encode_tx.clone();
    let node_label = E::NODE_LABEL;
    let mut input_task = tokio::spawn(async move {
        loop {
            let Some(first_packet) = input_rx.recv().await else {
                break;
            };

            let packet_batch =
                packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

            for packet in packet_batch {
                if let Packet::Video(mut frame) = packet {
                    let metadata = frame.metadata.take();
                    if encode_tx_clone.send((frame, metadata)).await.is_err() {
                        tracing::error!("{node_label} encode task has shut down unexpectedly");
                        return;
                    }
                }
            }
        }
        tracing::info!("{node_label} input stream closed");
    });

    // ── Forward loop ─────────────────────────────────────────────────────
    crate::codec_utils::codec_forward_loop(
        &mut context,
        &mut result_rx,
        &mut input_task,
        encode_task,
        encode_tx,
        &packets_processed_counter,
        &mut stats_tracker,
        |encoded| Packet::Binary {
            data: encoded.data,
            content_type: Some(Cow::Borrowed(E::CONTENT_TYPE)),
            metadata: encoded.metadata,
        },
        E::NODE_LABEL,
    )
    .await;

    state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
    tracing::info!("{} finished", E::NODE_LABEL);
    Ok(())
}

// ---------------------------------------------------------------------------
// Layer 2: Codec-level trait (single-thread encoders: VP9, AV1)
// ---------------------------------------------------------------------------

/// A video encoder codec that follows the single-thread
/// `create → encode → flush` pattern inside `spawn_blocking`.
///
/// VP9 and AV1/rav1e implement this trait.  SVT-AV1 does **not** — its
/// blocking `get_packet` API requires a two-thread architecture (separate
/// send and receive OS threads) to avoid deadlocks in low-delay mode.
///
/// Use [`spawn_standard_encode_task`] to turn an implementation into a
/// `tokio::task::JoinHandle<()>` suitable for
/// [`EncoderNodeRunner::spawn_codec_task`].
pub trait StandardVideoEncoder: 'static {
    /// Codec configuration type (e.g. `Vp9EncoderConfig`).
    type Config: Send + 'static;

    /// Human-readable codec name for error/log messages (e.g. `"VP9"`).
    const CODEC_NAME: &'static str;

    /// Create a new encoder for the given frame dimensions.
    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String>
    where
        Self: Sized;

    /// Encode a single video frame, returning zero or more encoded packets.
    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String>;

    /// Flush any buffered packets from the encoder.
    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String>;

    /// Whether to flush the old encoder before replacing it when frame
    /// dimensions change.
    ///
    /// Returns `true` for encoders that may buffer/reorder frames (e.g.
    /// AV1/rav1e in non-low-latency mode).  Returns `false` (default) for
    /// encoders that produce output immediately (e.g. VP9 with `lag=0`).
    fn flush_on_dimension_change() -> bool {
        false
    }
}

/// Spawn the standard single-thread encode loop as a blocking task.
///
/// Handles:
/// - Lazy encoder creation on first frame (and re-creation on dimension change)
/// - RGBA8 input rejection with a descriptive error
/// - Optional flush of the old encoder on dimension change
///   ([`StandardVideoEncoder::flush_on_dimension_change`])
/// - Per-frame encode timing via the provided OTel histogram
/// - Final flush on input channel close
/// - Early exit when the result channel is closed
///
/// This is the standard implementation used by VP9 and AV1.  SVT-AV1 has
/// its own two-thread variant and does not use this function.
pub fn spawn_standard_encode_task<E: StandardVideoEncoder>(
    config: E::Config,
    mut encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
    result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
    duration_histogram: opentelemetry::metrics::Histogram<f64>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut encoder: Option<E> = None;
        let mut current_dimensions: Option<(u32, u32)> = None;

        while let Some((frame, metadata)) = encode_rx.blocking_recv() {
            // Exit early if the async side has been cancelled (e.g. tokio
            // runtime shutting down).  Without this check the blocking
            // thread keeps the runtime alive indefinitely.
            if result_tx.is_closed() {
                return;
            }

            if frame.pixel_format == PixelFormat::Rgba8 {
                let _ = result_tx.blocking_send(Err(format!(
                    "{} encoder requires NV12 or I420 input; \
                     insert a video::pixel_convert node upstream",
                    E::CODEC_NAME,
                )));
                continue;
            }

            let frame_dimensions = (frame.width, frame.height);
            if current_dimensions != Some(frame_dimensions) {
                // Optionally flush the old encoder so that any
                // buffered/reordered frames are emitted rather than
                // silently dropped.
                if E::flush_on_dimension_change() {
                    if let Some(old_encoder) = encoder.as_mut() {
                        match old_encoder.flush_encoder() {
                            Ok(packets) => {
                                for packet in packets {
                                    if result_tx.blocking_send(Ok(packet)).is_err() {
                                        return;
                                    }
                                }
                            },
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "failed to flush old {} encoder during dimension change",
                                    E::CODEC_NAME,
                                );
                            },
                        }
                    }
                }

                match E::new_encoder(frame.width, frame.height, &config) {
                    Ok(new_encoder) => {
                        encoder = Some(new_encoder);
                        current_dimensions = Some(frame_dimensions);
                    },
                    Err(err) => {
                        tracing::warn!(
                            width = frame.width,
                            height = frame.height,
                            "{} encoder re-creation failed, dropping frame: {err}",
                            E::CODEC_NAME,
                        );
                        let _ = result_tx.blocking_send(Err(err));
                        continue;
                    },
                }
            }

            let Some(enc) = encoder.as_mut() else {
                let _ = result_tx
                    .blocking_send(Err(format!("{} encoder not initialized", E::CODEC_NAME,)));
                continue;
            };

            let encode_start_time = Instant::now();
            let result = enc.encode(&frame, metadata);
            duration_histogram.record(encode_start_time.elapsed().as_secs_f64(), &[]);

            match result {
                Ok(packets) => {
                    for packet in packets {
                        if result_tx.blocking_send(Ok(packet)).is_err() {
                            return;
                        }
                    }
                },
                Err(err) => {
                    let _ = result_tx.blocking_send(Err(err));
                },
            }
        }

        // Input channel closed — flush the encoder.
        if let Some(enc) = encoder.as_mut() {
            if !result_tx.is_closed() {
                match enc.flush_encoder() {
                    Ok(packets) => {
                        for packet in packets {
                            if result_tx.blocking_send(Ok(packet)).is_err() {
                                return;
                            }
                        }
                    },
                    Err(err) => {
                        let _ = result_tx.blocking_send(Err(err));
                    },
                }
            }
        }
    })
}
