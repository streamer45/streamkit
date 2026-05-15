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

/// Frame budget overrun boundaries (1ms to 1s)
/// Used by: video_encoder.frame_overrun_seconds
///
/// Records how much encode time exceeds the frame duration budget.
/// Granularity around the 33ms mark (one frame at 30fps) helps distinguish
/// minor jitter from severe overruns that cause A/V desync.
pub const HISTOGRAM_BOUNDARIES_FRAME_OVERRUN: &[f64] =
    &[0.001, 0.005, 0.01, 0.02, 0.033, 0.05, 0.1, 0.5, 1.0];

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sorted_and_positive(name: &str, boundaries: &[f64]) {
        assert!(!boundaries.is_empty(), "{name} must not be empty");
        assert!(
            boundaries[0] > 0.0,
            "{name} first boundary must be positive, got {}",
            boundaries[0]
        );
        for window in boundaries.windows(2) {
            assert!(
                window[0] < window[1],
                "{name} boundaries must be strictly ascending: {} >= {}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn all_boundary_arrays_sorted_and_positive() {
        let arrays: &[(&str, &[f64])] = &[
            ("CODEC_PACKET", HISTOGRAM_BOUNDARIES_CODEC_PACKET),
            ("FILE_OPERATION", HISTOGRAM_BOUNDARIES_FILE_OPERATION),
            ("NODE_EXECUTION", HISTOGRAM_BOUNDARIES_NODE_EXECUTION),
            ("BACKPRESSURE", HISTOGRAM_BOUNDARIES_BACKPRESSURE),
            ("PACER_LATENESS", HISTOGRAM_BOUNDARIES_PACER_LATENESS),
            ("CLOCK_OFFSET_MS", HISTOGRAM_BOUNDARIES_CLOCK_OFFSET_MS),
            ("FRAME_GAP_MS", HISTOGRAM_BOUNDARIES_FRAME_GAP_MS),
            ("PIPELINE_DURATION", HISTOGRAM_BOUNDARIES_PIPELINE_DURATION),
            ("HTTP_DURATION", HISTOGRAM_BOUNDARIES_HTTP_DURATION),
            ("SESSION_DURATION", HISTOGRAM_BOUNDARIES_SESSION_DURATION),
            ("FRAME_OVERRUN", HISTOGRAM_BOUNDARIES_FRAME_OVERRUN),
        ];
        for (name, arr) in arrays {
            assert_sorted_and_positive(name, arr);
        }
    }

    #[test]
    fn codec_packet_covers_sub_millisecond_to_second() {
        let b = HISTOGRAM_BOUNDARIES_CODEC_PACKET;
        assert!(b[0] <= 0.0001, "should start at sub-millisecond range");
        assert!(*b.last().unwrap() >= 1.0, "should reach at least 1 second");
    }

    #[test]
    fn session_duration_covers_hours() {
        let b = HISTOGRAM_BOUNDARIES_SESSION_DURATION;
        assert!(*b.last().unwrap() >= 86400.0, "should cover up to 24 hours");
    }
}
