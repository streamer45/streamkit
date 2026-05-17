// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::wit_types;
use bytes::Bytes;
use std::sync::Arc;
use streamkit_core::types::{
    AudioCodec, AudioFormat as CoreAudioFormat, CustomEncoding, CustomPacketData,
    EncodedAudioFormat, PacketType as CorePacketType,
};

impl TryFrom<wit_types::Packet> for streamkit_core::types::Packet {
    type Error = String;

    fn try_from(packet: wit_types::Packet) -> Result<Self, Self::Error> {
        match packet {
            wit_types::Packet::Audio(audio) => {
                Ok(Self::Audio(streamkit_core::types::AudioFrame::new(
                    audio.sample_rate,
                    audio.channels,
                    audio.samples,
                )))
            },
            wit_types::Packet::Text(text) => Ok(Self::Text(text.into())),
            wit_types::Packet::Binary(data) => Ok(Self::Binary {
                data: Bytes::from(data),
                content_type: None, // WASM plugins don't have content-type metadata
                metadata: None,
            }),
            wit_types::Packet::Custom(custom) => {
                let encoding = match custom.encoding {
                    wit_types::CustomEncoding::Json => CustomEncoding::Json,
                };
                let data: serde_json::Value = serde_json::from_str(&custom.data)
                    .map_err(|e| format!("Invalid custom JSON: {e}"))?;
                Ok(Self::Custom(Arc::new(CustomPacketData {
                    type_id: custom.type_id,
                    encoding,
                    data,
                    metadata: None,
                })))
            },
        }
    }
}

impl From<streamkit_core::types::Packet> for wit_types::Packet {
    fn from(packet: streamkit_core::types::Packet) -> Self {
        match packet {
            streamkit_core::types::Packet::Audio(audio) => {
                // Use Self to avoid repetition of wit_types::Packet type name
                // Convert Arc<PooledSamples> to Vec<f32> for WASM boundary
                Self::Audio(wit_types::AudioFrame {
                    sample_rate: audio.sample_rate,
                    channels: audio.channels,
                    samples: audio.samples.to_vec(),
                })
            },
            streamkit_core::types::Packet::Text(text) => Self::Text(text.to_string()),
            streamkit_core::types::Packet::Transcription(trans_data) => {
                // Serialize transcription to binary for WASM (JSON format)
                let json = serde_json::to_vec(&trans_data).unwrap_or_default();
                Self::Binary(json)
            },
            streamkit_core::types::Packet::Custom(custom) => {
                let encoding = match custom.encoding {
                    CustomEncoding::Json => wit_types::CustomEncoding::Json,
                };
                let data =
                    serde_json::to_string(&custom.data).unwrap_or_else(|_| "null".to_string());
                Self::Custom(wit_types::CustomPacket {
                    type_id: custom.type_id.clone(),
                    encoding,
                    data,
                })
            },
            // TODO: extend WIT interface for structured video frame support.
            // Currently video frames are converted to opaque Binary packets, discarding
            // all metadata (width, height, pixel_format, layout, keyframe). Plugins
            // receiving these packets have no way to reconstruct the frame without
            // out-of-band knowledge. This will be addressed when the WIT interface gains
            // native video types.
            streamkit_core::types::Packet::Video(frame) => {
                use std::sync::Once;
                static WARN: Once = Once::new();
                WARN.call_once(|| {
                    tracing::warn!(
                        "Video packet converted to Binary for WASM plugin: frame metadata \
                         (width, height, pixel_format, layout, keyframe) is lost"
                    );
                });
                Self::Binary(frame.data.to_vec())
            },
            streamkit_core::types::Packet::Binary { data, .. } => Self::Binary(data.to_vec()),
        }
    }
}

impl From<&wit_types::PacketType> for CorePacketType {
    fn from(packet_type: &wit_types::PacketType) -> Self {
        match packet_type {
            // Use Self to avoid repetition of CorePacketType type name
            wit_types::PacketType::RawAudio(fmt) => Self::RawAudio(CoreAudioFormat {
                sample_rate: fmt.sample_rate,
                channels: fmt.channels,
                sample_format: match fmt.sample_format {
                    wit_types::SampleFormat::Float32 => streamkit_core::types::SampleFormat::F32,
                    wit_types::SampleFormat::S16Le => streamkit_core::types::SampleFormat::S16Le,
                },
            }),
            wit_types::PacketType::OpusAudio => Self::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
            wit_types::PacketType::Text => Self::Text,
            wit_types::PacketType::Binary => Self::Binary,
            wit_types::PacketType::Custom(type_id) => Self::Custom { type_id: type_id.clone() },
            wit_types::PacketType::Any => Self::Any,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use streamkit_core::types::{Packet, SampleFormat as CoreSampleFormat, TranscriptionData};

    fn opus_codec_private_none(pt: &CorePacketType) -> bool {
        matches!(
            pt,
            CorePacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            })
        )
    }

    #[test]
    fn try_from_wit_audio_packet_preserves_frame_fields() {
        let wit_packet = wit_types::Packet::Audio(wit_types::AudioFrame {
            sample_rate: 48_000,
            channels: 2,
            samples: vec![0.1_f32, -0.2, 0.3, -0.4],
        });

        let core = Packet::try_from(wit_packet).expect("conversion succeeds");
        let Packet::Audio(frame) = core else {
            panic!("expected Packet::Audio variant");
        };
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples.as_slice(), &[0.1_f32, -0.2, 0.3, -0.4]);
    }

    #[test]
    fn try_from_wit_text_packet_produces_arc_str() {
        let core =
            Packet::try_from(wit_types::Packet::Text("hello".to_string())).expect("text converts");
        let Packet::Text(text) = core else {
            panic!("expected Packet::Text variant");
        };
        assert_eq!(text.as_ref(), "hello");
    }

    #[test]
    fn try_from_wit_binary_packet_drops_metadata() {
        let core = Packet::try_from(wit_types::Packet::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]))
            .expect("binary converts");
        let Packet::Binary { data, content_type, metadata } = core else {
            panic!("expected Packet::Binary variant");
        };
        assert_eq!(data.as_ref(), &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(content_type.is_none());
        assert!(metadata.is_none());
    }

    #[test]
    fn try_from_wit_custom_packet_parses_json() {
        let wit_packet = wit_types::Packet::Custom(wit_types::CustomPacket {
            type_id: "plugin::test/event@1".to_string(),
            encoding: wit_types::CustomEncoding::Json,
            data: r#"{"score":0.42,"label":"ok"}"#.to_string(),
        });

        let core = Packet::try_from(wit_packet).expect("custom converts");
        let Packet::Custom(custom) = core else {
            panic!("expected Packet::Custom variant");
        };
        assert_eq!(custom.type_id, "plugin::test/event@1");
        assert!(matches!(custom.encoding, CustomEncoding::Json));
        assert_eq!(custom.data, serde_json::json!({"score": 0.42, "label": "ok"}));
        assert!(custom.metadata.is_none());
    }

    #[test]
    fn try_from_wit_custom_packet_rejects_invalid_json() {
        let wit_packet = wit_types::Packet::Custom(wit_types::CustomPacket {
            type_id: "plugin::test/event@1".to_string(),
            encoding: wit_types::CustomEncoding::Json,
            data: "not json {".to_string(),
        });

        let err = Packet::try_from(wit_packet).expect_err("invalid JSON must error");
        assert!(
            err.starts_with("Invalid custom JSON:"),
            "expected error to start with 'Invalid custom JSON:', got: {err}"
        );
    }

    #[test]
    fn into_wit_audio_packet_clones_samples_into_vec() {
        let frame = streamkit_core::types::AudioFrame::new(16_000, 1, vec![0.5_f32, -0.5]);
        let wit = wit_types::Packet::from(Packet::Audio(frame));
        let wit_types::Packet::Audio(audio) = wit else {
            panic!("expected wit_types::Packet::Audio variant");
        };
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.samples, vec![0.5_f32, -0.5]);
    }

    #[test]
    fn into_wit_text_packet_renders_string() {
        let wit = wit_types::Packet::from(Packet::Text(Arc::from("hi there")));
        match wit {
            wit_types::Packet::Text(s) => assert_eq!(s, "hi there"),
            other => panic!("expected wit_types::Packet::Text, got {other:?}"),
        }
    }

    #[test]
    fn into_wit_binary_packet_drops_content_type_and_metadata() {
        let core = Packet::Binary {
            data: Bytes::from_static(&[1, 2, 3]),
            content_type: Some(std::borrow::Cow::Borrowed("audio/ogg")),
            metadata: None,
        };
        let wit = wit_types::Packet::from(core);
        match wit {
            wit_types::Packet::Binary(data) => assert_eq!(data, vec![1, 2, 3]),
            other => panic!("expected wit_types::Packet::Binary, got {other:?}"),
        }
    }

    #[test]
    fn into_wit_custom_packet_serializes_json_value() {
        let core = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "plugin::test/x@1".to_string(),
            encoding: CustomEncoding::Json,
            data: serde_json::json!({"a": 1}),
            metadata: None,
        }));
        let wit = wit_types::Packet::from(core);
        let wit_types::Packet::Custom(custom) = wit else {
            panic!("expected wit_types::Packet::Custom");
        };
        assert_eq!(custom.type_id, "plugin::test/x@1");
        assert!(matches!(custom.encoding, wit_types::CustomEncoding::Json));
        let parsed: serde_json::Value =
            serde_json::from_str(&custom.data).expect("JSON round-trip parses");
        assert_eq!(parsed, serde_json::json!({"a": 1}));
    }

    #[test]
    fn into_wit_transcription_packet_serializes_to_binary_json() {
        let trans = TranscriptionData {
            text: "hello world".to_string(),
            segments: vec![],
            language: Some("en".to_string()),
            metadata: None,
        };
        let wit = wit_types::Packet::from(Packet::Transcription(Arc::new(trans)));
        let wit_types::Packet::Binary(bytes) = wit else {
            panic!("expected transcription to be flattened to Binary");
        };
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes).expect("transcription JSON parses");
        assert_eq!(parsed["text"], "hello world");
        assert_eq!(parsed["language"], "en");
    }

    #[test]
    fn into_wit_video_packet_flattens_to_binary_dropping_metadata() {
        use streamkit_core::types::{PixelFormat, VideoFrame};
        // Smallest valid Rgba8 frame: 1x1 = 4 bytes
        let frame = VideoFrame::new(1, 1, PixelFormat::Rgba8, vec![0x11, 0x22, 0x33, 0x44])
            .expect("valid frame");
        let wit = wit_types::Packet::from(Packet::Video(frame));
        match wit {
            wit_types::Packet::Binary(bytes) => {
                assert_eq!(bytes, vec![0x11, 0x22, 0x33, 0x44]);
            },
            other => panic!("expected video to flatten to Binary, got {other:?}"),
        }
    }

    #[test]
    fn packet_type_raw_audio_float32_converts() {
        let wit = wit_types::PacketType::RawAudio(wit_types::AudioFormat {
            sample_rate: 44_100,
            channels: 2,
            sample_format: wit_types::SampleFormat::Float32,
        });
        let core = CorePacketType::from(&wit);
        let CorePacketType::RawAudio(fmt) = core else {
            panic!("expected RawAudio variant");
        };
        assert_eq!(fmt.sample_rate, 44_100);
        assert_eq!(fmt.channels, 2);
        assert_eq!(fmt.sample_format, CoreSampleFormat::F32);
    }

    #[test]
    fn packet_type_raw_audio_s16le_converts() {
        let wit = wit_types::PacketType::RawAudio(wit_types::AudioFormat {
            sample_rate: 8_000,
            channels: 1,
            sample_format: wit_types::SampleFormat::S16Le,
        });
        let core = CorePacketType::from(&wit);
        let CorePacketType::RawAudio(fmt) = core else {
            panic!("expected RawAudio variant");
        };
        assert_eq!(fmt.sample_format, CoreSampleFormat::S16Le);
    }

    #[test]
    fn packet_type_opus_audio_maps_to_encoded_opus_without_codec_private() {
        let core = CorePacketType::from(&wit_types::PacketType::OpusAudio);
        assert!(
            opus_codec_private_none(&core),
            "expected EncodedAudio(Opus) with codec_private=None, got {core:?}"
        );
    }

    #[test]
    fn packet_type_text_binary_any_map_directly() {
        assert!(matches!(CorePacketType::from(&wit_types::PacketType::Text), CorePacketType::Text));
        assert!(matches!(
            CorePacketType::from(&wit_types::PacketType::Binary),
            CorePacketType::Binary
        ));
        assert!(matches!(CorePacketType::from(&wit_types::PacketType::Any), CorePacketType::Any));
    }

    #[test]
    fn packet_type_custom_preserves_type_id() {
        let core =
            CorePacketType::from(&wit_types::PacketType::Custom("plugin::native::x@1".into()));
        match core {
            CorePacketType::Custom { type_id } => assert_eq!(type_id, "plugin::native::x@1"),
            other => panic!("expected Custom variant, got {other:?}"),
        }
    }
}
