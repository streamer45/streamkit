// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared constants for MoQ transport nodes

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
