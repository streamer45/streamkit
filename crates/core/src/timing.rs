// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Timing helpers and canonical semantics for media packets.
//!
//! # Timing contract
//!
//! - `timestamp_us` is media presentation time in microseconds, relative to the
//!   stream's epoch (normally the first frame = 0). It is monotonic and
//!   non-decreasing per stream.
//! - `duration_us` is the playback duration for the payload in microseconds and
//!   should be set whenever it can be derived (decode, demux, resample).
//! - `sequence` is a monotonic counter scoped to the stream, useful for loss and
//!   ordering detection when absolute time is absent.
//! - Nodes should preserve `timestamp_us` and `duration_us` across transforms, or
//!   recompute them when they change payload timing (e.g., resampling). If timing
//!   is unknown, leave fields as `None` rather than inventing values.
//! - Engines and transports may introduce buffering but must not reorder packets
//!   with identical timestamps; late/drop policies should be explicit in nodes
//!   (see pacer/mixer).
//!
//! This module provides lightweight helpers for duration math and monotonicity
//! checks to keep node implementations consistent.

use crate::types::PacketMetadata;

/// Microseconds per second constant.
pub const MICROS_PER_SECOND: u64 = 1_000_000;
/// Milliseconds per second constant.
pub const MILLIS_PER_SECOND: u64 = 1_000;

/// Convert microseconds to milliseconds, rounding up (never under-reports duration).
#[must_use]
pub const fn duration_us_to_ms_ceil(duration_us: u64) -> u64 {
    duration_us.saturating_add(999) / 1000
}

/// Convert frames-per-channel to duration in microseconds.
///
/// Returns `None` if `sample_rate` is 0 to avoid divide-by-zero.
#[must_use]
pub fn frames_to_duration_us(frames_per_channel: u64, sample_rate: u32) -> Option<u64> {
    if sample_rate == 0 {
        return None;
    }
    Some(frames_per_channel.saturating_mul(MICROS_PER_SECOND) / u64::from(sample_rate))
}

/// Convert an interleaved sample count to duration in microseconds.
///
/// Returns `None` if `channels` or `sample_rate` is 0.
#[must_use]
pub fn samples_to_duration_us(samples: usize, channels: u16, sample_rate: u32) -> Option<u64> {
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let frames_per_channel = samples as u64 / u64::from(channels);
    frames_to_duration_us(frames_per_channel, sample_rate)
}

/// Advance a timestamp by a duration, if both are present.
///
/// - If `timestamp_us` is `Some` and `duration_us` is `Some`, returns
///   `timestamp_us + duration_us` (saturating).
/// - Otherwise, returns `timestamp_us` unchanged.
#[must_use]
pub fn advance_timestamp(timestamp_us: Option<u64>, duration_us: Option<u64>) -> Option<u64> {
    match (timestamp_us, duration_us) {
        (Some(ts), Some(dur)) => Some(ts.saturating_add(dur)),
        (ts, _) => ts,
    }
}

/// Returns true if `next` is monotonic (non-decreasing) with respect to `prev`.
///
/// When either side is `None`, the check is treated as passing.
#[must_use]
pub fn is_monotonic(prev: Option<u64>, next: Option<u64>) -> bool {
    match (prev, next) {
        (Some(p), Some(n)) => n >= p,
        _ => true,
    }
}

/// Extract the starting timestamp from metadata, or 0 if not present.
/// Useful when computing cumulative media time.
#[must_use]
pub fn starting_timestamp_or_zero(metadata: &Option<PacketMetadata>) -> u64 {
    metadata.as_ref().and_then(|m| m.timestamp_us).unwrap_or(0)
}

/// Infer a duration from consecutive timestamps; fall back to `default` when missing or non-positive.
#[must_use]
pub fn infer_duration_us(current_ts: u64, previous_ts: Option<u64>, default: u64) -> u64 {
    previous_ts.and_then(|prev| current_ts.checked_sub(prev)).filter(|d| *d > 0).unwrap_or(default)
}

/// Merge metadata from multiple inputs: min timestamp, max duration, max sequence.
#[must_use]
pub fn merge_metadata<'a, I: Iterator<Item = &'a PacketMetadata>>(
    iter: I,
) -> Option<PacketMetadata> {
    let mut ts = None;
    let mut dur = None;
    let mut seq = None;
    let mut keyframe = None;
    let mut keyframe_conflict = false;
    for m in iter {
        if let Some(t) = m.timestamp_us {
            ts = Some(ts.map_or(t, |prev: u64| prev.min(t)));
        }
        if let Some(d) = m.duration_us {
            dur = Some(dur.map_or(d, |prev: u64| prev.max(d)));
        }
        if let Some(s) = m.sequence {
            seq = Some(seq.map_or(s, |prev: u64| prev.max(s)));
        }
        if !keyframe_conflict {
            if let Some(k) = m.keyframe {
                match keyframe {
                    None => keyframe = Some(k),
                    Some(existing) if existing == k => {},
                    Some(_) => {
                        keyframe = None;
                        keyframe_conflict = true;
                    },
                }
            }
        }
    }
    if ts.is_some() || dur.is_some() || seq.is_some() || keyframe.is_some() {
        Some(PacketMetadata { timestamp_us: ts, duration_us: dur, sequence: seq, keyframe })
    } else {
        None
    }
}

/// A simple media clock that tracks media time in microseconds with an optional initial delay.
#[derive(Debug, Clone)]
pub struct MediaClock {
    initial_delay_us: u64,
    media_time_us: u64,
    seeded: bool,
}

impl MediaClock {
    /// Create a new clock with an initial delay (microseconds).
    pub const fn new(initial_delay_ms: u64) -> Self {
        Self { initial_delay_us: initial_delay_ms * 1000, media_time_us: 0, seeded: false }
    }

    /// Seed the clock from an absolute media timestamp. Idempotent (first seed wins).
    pub fn seed_from_timestamp_us(&mut self, timestamp_us: u64) {
        if !self.seeded {
            self.media_time_us = timestamp_us;
            self.seeded = true;
        }
    }

    /// Advance by a duration (or default) and return the duration used (ms rounded up).
    pub fn advance_by_duration_us(&mut self, duration_us: Option<u64>, default: u64) -> u64 {
        let dur = duration_us.unwrap_or(default);
        self.media_time_us = self.media_time_us.saturating_add(dur);
        duration_us_to_ms_ceil(dur)
    }

    /// Current media timestamp in microseconds (includes initial delay).
    #[must_use]
    pub const fn timestamp_us(&self) -> u64 {
        self.initial_delay_us.saturating_add(self.media_time_us)
    }

    /// Current media timestamp in milliseconds (rounded up).
    #[must_use]
    pub const fn timestamp_ms(&self) -> u64 {
        duration_us_to_ms_ceil(self.timestamp_us())
    }

    /// Returns true if at a group boundary (e.g., for MoQ keyframes).
    #[must_use]
    pub fn is_group_boundary_ms(&self, group_duration_ms: u64) -> bool {
        let t = self.timestamp_ms();
        group_duration_ms > 0 && t.is_multiple_of(group_duration_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frames_to_duration_us() {
        assert_eq!(frames_to_duration_us(960, 48_000), Some(20_000));
        assert_eq!(frames_to_duration_us(0, 48_000), Some(0));
        assert_eq!(frames_to_duration_us(1_920, 48_000), Some(40_000));
        assert_eq!(frames_to_duration_us(1, 0), None);
    }

    #[test]
    fn test_samples_to_duration_us() {
        assert_eq!(samples_to_duration_us(1_920, 2, 48_000), Some(20_000));
        assert_eq!(samples_to_duration_us(1_920, 0, 48_000), None);
        assert_eq!(samples_to_duration_us(1_920, 2, 0), None);
    }

    #[test]
    fn test_advance_timestamp() {
        assert_eq!(advance_timestamp(Some(1_000), Some(500)), Some(1_500));
        assert_eq!(advance_timestamp(Some(1_000), None), Some(1_000));
        assert_eq!(advance_timestamp(None, Some(500)), None);
    }

    #[test]
    fn test_is_monotonic() {
        assert!(is_monotonic(Some(1), Some(1)));
        assert!(is_monotonic(Some(1), Some(2)));
        assert!(!is_monotonic(Some(2), Some(1)));
        assert!(is_monotonic(None, Some(1)));
        assert!(is_monotonic(Some(1), None));
    }
}
