// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared constants for MoQ transport nodes

use streamkit_core::types::PacketMetadata;

pub const DEFAULT_AUDIO_FRAME_DURATION_US: u64 = 20_000;

pub fn packet_duration_us(metadata: Option<&PacketMetadata>) -> Option<u64> {
    metadata.and_then(|m| m.duration_us).filter(|d| *d > 0)
}
