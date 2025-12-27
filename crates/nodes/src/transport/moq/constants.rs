// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared constants for MoQ transport nodes

use streamkit_core::types::PacketMetadata;

pub const DEFAULT_AUDIO_FRAME_DURATION_US: u64 = 20_000;

const fn duration_us_to_ms_ceil(duration_us: u64) -> u64 {
    // hang::Timestamp is millisecond granularity; round up so we never claim
    // a frame is shorter than it is (helps avoid drift/under-runs).
    duration_us.saturating_add(999) / 1000
}

pub fn packet_duration_us(metadata: Option<&PacketMetadata>) -> Option<u64> {
    metadata.and_then(|m| m.duration_us).filter(|d| *d > 0)
}

#[derive(Debug, Clone)]
pub struct MediaClock {
    initial_delay_ms: u64,
    media_time_ms: u64,
}

impl MediaClock {
    pub const fn new(initial_delay_ms: u64) -> Self {
        Self { initial_delay_ms, media_time_ms: 0 }
    }

    pub const fn timestamp_ms(&self) -> u64 {
        self.initial_delay_ms.saturating_add(self.media_time_ms)
    }

    pub const fn is_group_boundary(&self, group_duration_ms: u64) -> bool {
        group_duration_ms > 0 && self.media_time_ms.is_multiple_of(group_duration_ms)
    }

    pub fn advance_by_duration_us(&mut self, duration_us: Option<u64>) -> u64 {
        let duration_us = duration_us.unwrap_or(DEFAULT_AUDIO_FRAME_DURATION_US);
        let frame_duration_ms = duration_us_to_ms_ceil(duration_us).max(1);
        self.media_time_ms = self.media_time_ms.saturating_add(frame_duration_ms);
        frame_duration_ms
    }
}
