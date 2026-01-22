// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared metrics configuration and histogram boundaries.
//!
//! Defines standard histogram bucket boundaries for OpenTelemetry metrics
//! to ensure accurate percentile calculations across the codebase.

/// Sub-millisecond boundaries for per-packet codec operations (10μs to 1s)
/// Used by: opus encode/decode
pub const HISTOGRAM_BOUNDARIES_CODEC_PACKET: &[f64] =
    &[0.00001, 0.00005, 0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0];

/// Millisecond-to-minute boundaries for per-file operations (1ms to 60s)
/// Used by: mp3/flac/wav decode/demux
pub const HISTOGRAM_BOUNDARIES_FILE_OPERATION: &[f64] =
    &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0];

/// Node execution boundaries (10μs to 60s)
/// Used by: node.execution.duration
pub const HISTOGRAM_BOUNDARIES_NODE_EXECUTION: &[f64] =
    &[0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0, 10.0, 60.0];

/// Backpressure wait time boundaries (1μs to 10s)
/// Used by: pin_distributor.send_wait_seconds
pub const HISTOGRAM_BOUNDARIES_BACKPRESSURE: &[f64] =
    &[0.000001, 0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0, 10.0];

/// Pacer lateness boundaries (1ms to 10s)
/// Used by: pacer.lateness_seconds
pub const HISTOGRAM_BOUNDARIES_PACER_LATENESS: &[f64] =
    &[0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];

/// Clock offset boundaries in milliseconds (0.1ms to 10s)
/// Used by: moq.push.clock_offset_ms
pub const HISTOGRAM_BOUNDARIES_CLOCK_OFFSET_MS: &[f64] =
    &[0.1, 1.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 5000.0, 10000.0];

/// Frame gap boundaries in milliseconds (1ms to 1s, with common frame rates)
/// Used by: moq.peer.inter_frame_ms
pub const HISTOGRAM_BOUNDARIES_FRAME_GAP_MS: &[f64] =
    &[1.0, 5.0, 10.0, 16.0, 20.0, 33.0, 50.0, 100.0, 200.0, 500.0, 1000.0];

/// Pipeline duration boundaries (10ms to 5 minutes)
/// Used by: oneshot_pipeline.duration
pub const HISTOGRAM_BOUNDARIES_PIPELINE_DURATION: &[f64] =
    &[0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];

/// HTTP request duration boundaries (1ms to 60s)
/// Used by: http.server.duration
pub const HISTOGRAM_BOUNDARIES_HTTP_DURATION: &[f64] =
    &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0];

/// Session lifetime boundaries (1s to 24 hours)
/// Used by: session.duration
pub const HISTOGRAM_BOUNDARIES_SESSION_DURATION: &[f64] =
    &[1.0, 10.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 7200.0, 21600.0, 86400.0];
