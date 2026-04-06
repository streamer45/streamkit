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

pub const DEFAULT_AUDIO_FRAME_DURATION_US: u64 = 20_000;

pub fn packet_duration_us(metadata: Option<&PacketMetadata>) -> Option<u64> {
    metadata.and_then(|m| m.duration_us).filter(|d| *d > 0)
}

/// Return the accepted media types for dynamic MoQ pins (Opus audio + VP9/AV1/H264 video).
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
    config_codec: Option<&str>,
    input_types: &std::collections::HashMap<String, PacketType>,
) -> VideoCodec {
    config_codec
        .and_then(parse_video_codec_config)
        .or_else(|| {
            input_types.iter().find_map(|(_, pt)| match pt {
                PacketType::EncodedVideo(fmt) => Some(fmt.codec),
                _ => None,
            })
        })
        .unwrap_or(VideoCodec::Vp9)
}

/// Parse a `video_codec` config string (e.g. `"vp9"`, `"av1"`) into the
/// corresponding [`VideoCodec`].  Returns `None` for unrecognised values so
/// the caller can fall back to auto-detection.
///
/// Shared by `moq_peer` and `moq_push` to avoid duplicating the parsing logic.
pub fn parse_video_codec_config(s: &str) -> Option<VideoCodec> {
    match s.to_ascii_lowercase().as_str() {
        "vp9" => Some(VideoCodec::Vp9),
        "av1" => Some(VideoCodec::Av1),
        "h264" => Some(VideoCodec::H264),
        _ => {
            tracing::warn!(video_codec = %s, "unrecognised video_codec config value — ignoring");
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_video_codec_config_vp9() {
        assert_eq!(parse_video_codec_config("vp9"), Some(VideoCodec::Vp9));
    }

    #[test]
    fn parse_video_codec_config_av1() {
        assert_eq!(parse_video_codec_config("av1"), Some(VideoCodec::Av1));
    }

    #[test]
    fn parse_video_codec_config_case_insensitive() {
        assert_eq!(parse_video_codec_config("AV1"), Some(VideoCodec::Av1));
        assert_eq!(parse_video_codec_config("VP9"), Some(VideoCodec::Vp9));
        assert_eq!(parse_video_codec_config("Av1"), Some(VideoCodec::Av1));
        assert_eq!(parse_video_codec_config("H264"), Some(VideoCodec::H264));
    }

    #[test]
    fn parse_video_codec_config_h264() {
        assert_eq!(parse_video_codec_config("h264"), Some(VideoCodec::H264));
    }

    #[test]
    fn parse_video_codec_config_unknown_returns_none() {
        assert_eq!(parse_video_codec_config(""), None);
        assert_eq!(parse_video_codec_config("unknown"), None);
    }

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
        assert_eq!(resolve_video_codec(Some("av1"), &input_types), VideoCodec::Av1);
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
}
