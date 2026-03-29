// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared constants for MoQ transport nodes

use crate::video::{AV1_BIT_DEPTH, AV1_LEVEL, AV1_PROFILE, AV1_TIER, VP9_BIT_DEPTH, VP9_LEVEL, VP9_PROFILE};
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, PacketMetadata, PacketType, VideoCodec,
};

pub const DEFAULT_AUDIO_FRAME_DURATION_US: u64 = 20_000;

pub fn packet_duration_us(metadata: Option<&PacketMetadata>) -> Option<u64> {
    metadata.and_then(|m| m.duration_us).filter(|d| *d > 0)
}

/// Return the accepted media types for dynamic MoQ pins (Opus audio + VP9/AV1 video).
///
/// This is shared across `moq_peer` and `moq_push` to avoid duplicating the
/// type construction in every `RequestAddInputPin` / `RequestAddOutputPin` handler.
pub fn moq_accepted_media_types() -> Vec<PacketType> {
    vec![
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Av1,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }),
    ]
}

/// Build the [`hang::catalog::VideoCodec`] entry for a given [`VideoCodec`].
///
/// Centralises the AV1/VP9 catalog construction that was previously duplicated
/// in `push.rs` (static + dynamic) and `peer/mod.rs`.
pub fn catalog_video_codec(codec: VideoCodec) -> hang::catalog::VideoCodec {
    match codec {
        VideoCodec::Av1 => hang::catalog::VideoCodec::AV1(hang::catalog::AV1 {
            profile: AV1_PROFILE,
            level: AV1_LEVEL,
            tier: AV1_TIER,
            bitdepth: AV1_BIT_DEPTH,
            ..hang::catalog::AV1::default()
        }),
        // VP9 is the default / fallback.
        _ => hang::catalog::VideoCodec::VP9(hang::catalog::VP9 {
            profile: VP9_PROFILE,
            level: VP9_LEVEL,
            bit_depth: VP9_BIT_DEPTH,
            ..hang::catalog::VP9::default()
        }),
    }
}
