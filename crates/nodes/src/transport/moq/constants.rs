// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared constants for MoQ transport nodes

use crate::video::{
    AV1_BIT_DEPTH, AV1_LEVEL, AV1_PROFILE, AV1_TIER, H264_CONSTRAINTS, H264_LEVEL, H264_PROFILE,
    VP9_BIT_DEPTH, VP9_LEVEL, VP9_PROFILE,
};
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, PacketMetadata, PacketType, VideoCodec,
};

pub fn packet_duration_us(metadata: Option<&PacketMetadata>) -> Option<u64> {
    metadata.and_then(|m| m.duration_us).filter(|d| *d > 0)
}

/// Return the accepted media types for dynamic MoQ pins (Opus/AAC audio + VP9/AV1/H264 video).
///
/// This is shared across `moq_peer` and `moq_push` to avoid duplicating the
/// type construction in every `RequestAddInputPin` / `RequestAddOutputPin` handler.
pub fn moq_accepted_media_types() -> Vec<PacketType> {
    vec![
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        }),
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Aac,
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
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::H264,
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
        VideoCodec::Vp9 => hang::catalog::VideoCodec::VP9(hang::catalog::VP9 {
            profile: VP9_PROFILE,
            level: VP9_LEVEL,
            bit_depth: VP9_BIT_DEPTH,
            ..hang::catalog::VP9::default()
        }),
        // OpenH264 produces Constrained Baseline (profile 0x42, constraints
        // 0xC0) — inline SPS/PPS in bitstream (avc3 style, Annex B NALUs).
        VideoCodec::H264 => hang::catalog::VideoCodec::H264(hang::catalog::H264 {
            profile: H264_PROFILE,
            constraints: H264_CONSTRAINTS,
            level: H264_LEVEL,
            inline: true,
        }),
        // Unsupported codec — fall back to VP9 catalog entry and log a warning.
        _ => {
            tracing::warn!(?codec, "unsupported VideoCodec for MoQ catalog, defaulting to VP9");
            hang::catalog::VideoCodec::VP9(hang::catalog::VP9 {
                profile: VP9_PROFILE,
                level: VP9_LEVEL,
                bit_depth: VP9_BIT_DEPTH,
                ..hang::catalog::VP9::default()
            })
        },
    }
}

/// Resolve the video codec from config → input_types → default (VP9).
///
/// Priority order:
/// 1. Explicit `video_codec` config param (required for dynamic pipelines)
/// 2. Auto-detected from `input_types` (static pipelines)
/// 3. Default: VP9
///
/// Shared by `moq_peer` and `moq_push` to avoid duplicating the resolution chain.
pub fn resolve_video_codec(
    config_codec: Option<VideoCodec>,
    input_types: &std::collections::HashMap<String, PacketType>,
) -> VideoCodec {
    config_codec
        .or_else(|| {
            input_types.iter().find_map(|(_, pt)| match pt {
                PacketType::EncodedVideo(fmt) => Some(fmt.codec),
                _ => None,
            })
        })
        .unwrap_or(VideoCodec::Vp9)
}

/// Build the [`hang::catalog::AudioCodec`] entry for a given [`AudioCodec`].
///
/// Centralises the AAC/Opus catalog construction, mirroring
/// [`catalog_video_codec`] for the video side.
pub fn catalog_audio_codec(codec: AudioCodec) -> hang::catalog::AudioCodec {
    match codec {
        AudioCodec::Opus => hang::catalog::AudioCodec::Opus,
        AudioCodec::Aac => hang::catalog::AudioCodec::AAC(hang::catalog::AAC { profile: 2 }),
        // Future-proof: fall back to Opus and warn.
        _ => {
            tracing::warn!(?codec, "unsupported AudioCodec for MoQ catalog, defaulting to Opus");
            hang::catalog::AudioCodec::Opus
        },
    }
}

/// Map a [`hang::catalog::AudioCodec`] back to our [`AudioCodec`].
///
/// Returns `None` for catalog codecs we don't support yet.
pub const fn audio_codec_from_catalog(catalog_codec: &hang::catalog::AudioCodec) -> Option<AudioCodec> {
    match catalog_codec {
        hang::catalog::AudioCodec::Opus => Some(AudioCodec::Opus),
        // Only accept AAC-LC (profile 2) — matches what catalog_audio_codec() emits.
        hang::catalog::AudioCodec::AAC(aac) if aac.profile == 2 => Some(AudioCodec::Aac),
        _ => None,
    }
}

/// Resolve the audio codec from config → input_types → default (Opus).
///
/// Priority order:
/// 1. Explicit `audio_codec` config param (required for dynamic pipelines)
/// 2. Auto-detected from `input_types` (static pipelines)
/// 3. Default: Opus
///
/// Shared by `moq_peer` and `moq_push` to avoid duplicating the resolution chain.
pub fn resolve_audio_codec(
    config_codec: Option<AudioCodec>,
    input_types: &std::collections::HashMap<String, PacketType>,
) -> AudioCodec {
    config_codec
        .or_else(|| {
            input_types.iter().find_map(|(_, pt)| match pt {
                PacketType::EncodedAudio(fmt) => Some(fmt.codec),
                _ => None,
            })
        })
        .unwrap_or(AudioCodec::Opus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_video_codec_av1_produces_av1() {
        let result = catalog_video_codec(VideoCodec::Av1);
        assert!(
            matches!(result, hang::catalog::VideoCodec::AV1(_)),
            "expected AV1 catalog codec, got {result:?}"
        );
    }

    #[test]
    fn catalog_video_codec_vp9_produces_vp9() {
        let result = catalog_video_codec(VideoCodec::Vp9);
        assert!(
            matches!(result, hang::catalog::VideoCodec::VP9(_)),
            "expected VP9 catalog codec, got {result:?}"
        );
    }

    #[test]
    fn catalog_video_codec_h264_produces_h264() {
        let result = catalog_video_codec(VideoCodec::H264);
        assert!(
            matches!(result, hang::catalog::VideoCodec::H264(_)),
            "expected H264 catalog codec, got {result:?}"
        );
    }

    #[test]
    fn resolve_video_codec_prefers_config() {
        let mut input_types = std::collections::HashMap::new();
        input_types.insert(
            "video".to_string(),
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Vp9,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        );
        // Config says AV1 — should win over input_types VP9.
        assert_eq!(resolve_video_codec(Some(VideoCodec::Av1), &input_types), VideoCodec::Av1);
    }

    #[test]
    fn resolve_video_codec_falls_back_to_input_types() {
        let mut input_types = std::collections::HashMap::new();
        input_types.insert(
            "video".to_string(),
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        );
        assert_eq!(resolve_video_codec(None, &input_types), VideoCodec::Av1);
    }

    #[test]
    fn resolve_video_codec_defaults_to_vp9() {
        let input_types = std::collections::HashMap::new();
        assert_eq!(resolve_video_codec(None, &input_types), VideoCodec::Vp9);
    }

    #[test]
    fn audio_codec_from_catalog_opus() {
        assert_eq!(
            audio_codec_from_catalog(&hang::catalog::AudioCodec::Opus),
            Some(AudioCodec::Opus)
        );
    }

    #[test]
    fn audio_codec_from_catalog_aac() {
        assert_eq!(
            audio_codec_from_catalog(&hang::catalog::AudioCodec::AAC(hang::catalog::AAC {
                profile: 2,
            })),
            Some(AudioCodec::Aac)
        );
    }

    #[test]
    fn resolve_audio_codec_prefers_config() {
        let input_types = std::collections::HashMap::new();
        assert_eq!(resolve_audio_codec(Some(AudioCodec::Aac), &input_types), AudioCodec::Aac);
    }

    #[test]
    fn resolve_audio_codec_falls_back_to_input_types() {
        let mut input_types = std::collections::HashMap::new();
        input_types.insert(
            "audio".to_string(),
            PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Aac,
                codec_private: None,
            }),
        );
        assert_eq!(resolve_audio_codec(None, &input_types), AudioCodec::Aac);
    }

    #[test]
    fn resolve_audio_codec_defaults_to_opus() {
        let input_types = std::collections::HashMap::new();
        assert_eq!(resolve_audio_codec(None, &input_types), AudioCodec::Opus);
    }
}
