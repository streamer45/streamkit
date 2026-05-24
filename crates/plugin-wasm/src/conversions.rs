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
// Tests rely on expect/unwrap to fail fast with readable assertion context.
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
    fn try_from_wit_binary_packet_produces_packet_with_no_metadata() {
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
        let wit_types::Packet::Text(s) = wit else {
            panic!("expected wit_types::Packet::Text, got {wit:?}");
        };
        assert_eq!(s, "hi there");
    }

    #[test]
    fn into_wit_binary_packet_drops_content_type_and_metadata() {
        let core = Packet::Binary {
            data: Bytes::from_static(&[1, 2, 3]),
            content_type: Some(std::borrow::Cow::Borrowed("audio/ogg")),
            metadata: None,
        };
        let wit = wit_types::Packet::from(core);
        let wit_types::Packet::Binary(data) = wit else {
            panic!("expected wit_types::Packet::Binary, got {wit:?}");
        };
        assert_eq!(data, vec![1, 2, 3]);
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
        // NOTE: this conversion triggers a process-wide once-gated `tracing::warn!`
        // (see conversions.rs `From<core::Packet> for wit_types::Packet`); any test
        // that asserts the warning fires must order itself before this one.
        use streamkit_core::types::{PixelFormat, VideoFrame};
        let frame = VideoFrame::new(1, 1, PixelFormat::Rgba8, vec![0x11, 0x22, 0x33, 0x44])
            .expect("valid frame");
        let wit = wit_types::Packet::from(Packet::Video(frame));
        let wit_types::Packet::Binary(bytes) = wit else {
            panic!("expected video to flatten to Binary, got {wit:?}");
        };
        assert_eq!(bytes, vec![0x11, 0x22, 0x33, 0x44]);
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
    fn packet_type_text_maps_directly() {
        assert!(matches!(CorePacketType::from(&wit_types::PacketType::Text), CorePacketType::Text));
    }

    #[test]
    fn packet_type_binary_maps_directly() {
        assert!(matches!(
            CorePacketType::from(&wit_types::PacketType::Binary),
            CorePacketType::Binary
        ));
    }

    #[test]
    fn packet_type_any_maps_directly() {
        assert!(matches!(CorePacketType::from(&wit_types::PacketType::Any), CorePacketType::Any));
    }

    #[test]
    fn packet_type_custom_preserves_type_id() {
        let core =
            CorePacketType::from(&wit_types::PacketType::Custom("plugin::native::x@1".into()));
        let CorePacketType::Custom { type_id } = core else {
            panic!("expected Custom variant, got {core:?}");
        };
        assert_eq!(type_id, "plugin::native::x@1");
    }

    // ── Round-trip edge cases ───────────────────────────────────────────
    //
    // These exercise the boundary conditions that pure-data unit tests above
    // can miss: large payloads, multi-byte UTF-8, and nested custom data.

    fn round_trip_packet(packet: Packet) -> Packet {
        let wit = wit_types::Packet::from(packet);
        Packet::try_from(wit)
            .expect("round-trip via WIT must succeed for non-Transcription packets")
    }

    #[test]
    fn round_trip_binary_packet_preserves_empty_payload() {
        let original = Packet::Binary { data: Bytes::new(), content_type: None, metadata: None };
        let Packet::Binary { data, content_type, metadata } = round_trip_packet(original) else {
            panic!("round-trip must yield Binary");
        };
        assert!(data.is_empty());
        assert!(content_type.is_none());
        assert!(metadata.is_none());
    }

    #[test]
    fn round_trip_binary_packet_preserves_1mib_payload_byte_for_byte() {
        // 1 MiB boundary — exercises the Vec<u8>↔Bytes copy paths at a size
        // large enough to catch any accidental truncation in the WIT bridge.
        let size = 1024 * 1024;
        let mut original = Vec::with_capacity(size);
        for i in 0..size {
            // i % 251 keeps a non-trivial repeating pattern (251 is prime).
            original.push(u8::try_from(i % 251).expect("pattern fits in u8"));
        }
        let packet = Packet::Binary {
            data: Bytes::from(original.clone()),
            content_type: None,
            metadata: None,
        };
        let Packet::Binary { data, .. } = round_trip_packet(packet) else {
            panic!("round-trip must yield Binary");
        };
        assert_eq!(data.len(), original.len());
        assert_eq!(data.as_ref(), original.as_slice());
    }

    #[test]
    fn round_trip_text_packet_preserves_non_ascii_utf8_including_4byte_emoji() {
        // Includes BMP (café), CJK (日本語), supplementary plane (🎵 = U+1F3B5
        // encoded as a UTF-16 surrogate pair, 4-byte UTF-8). If the WIT bridge
        // ever truncated or mishandled multi-byte sequences this would break.
        let original = "café 日本語 🎵 \u{0301}\u{0308}";
        let packet = Packet::Text(Arc::from(original));
        let Packet::Text(text) = round_trip_packet(packet) else {
            panic!("round-trip must yield Text");
        };
        assert_eq!(text.as_ref(), original);
    }

    #[test]
    fn round_trip_custom_packet_preserves_two_layer_nested_variant() {
        // Two layers deep: outer object → array → object → array → primitives.
        let nested = serde_json::json!({
            "envelope": {
                "events": [
                    {"kind": "open", "payload": {"score": 0.9, "tags": ["a", "b"]}},
                    {"kind": "close", "payload": {"score": -0.1, "tags": []}},
                ],
                "summary": {"count": 2, "labels": ["A", "B"]},
            },
        });
        let packet = Packet::Custom(Arc::new(CustomPacketData {
            type_id: "plugin::test/nested@1".to_string(),
            encoding: CustomEncoding::Json,
            data: nested.clone(),
            metadata: None,
        }));
        let Packet::Custom(custom) = round_trip_packet(packet) else {
            panic!("round-trip must yield Custom");
        };
        assert_eq!(custom.type_id, "plugin::test/nested@1");
        assert!(matches!(custom.encoding, CustomEncoding::Json));
        assert_eq!(custom.data, nested);
    }

    #[test]
    fn round_trip_audio_packet_preserves_samples_and_format_fields() {
        let frame = streamkit_core::types::AudioFrame::new(
            48_000,
            2,
            vec![-1.0, -0.5, 0.0, 0.5, 1.0, 0.123_456_7],
        );
        let original = Packet::Audio(frame);
        let Packet::Audio(out) = round_trip_packet(original) else {
            panic!("round-trip must yield Audio");
        };
        assert_eq!(out.sample_rate, 48_000);
        assert_eq!(out.channels, 2);
        assert_eq!(out.samples.as_slice(), &[-1.0, -0.5, 0.0, 0.5, 1.0, 0.123_456_7]);
    }

    // ── Property-style round-trip ────────────────────────────────────────
    //
    // Hand-rolled because `proptest` is not a workspace dependency. A small
    // deterministic LCG drives the input space — failure replays exactly via
    // the seed in the loop body.

    fn lcg_next(state: &mut u64) -> u64 {
        // Numerical Recipes constants — full-period 64-bit LCG.
        *state =
            state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    fn random_packet_type(rng: &mut u64) -> wit_types::PacketType {
        match lcg_next(rng) % 6 {
            0 => wit_types::PacketType::RawAudio(wit_types::AudioFormat {
                sample_rate: u32::try_from(lcg_next(rng) % 192_001).unwrap_or(0),
                channels: u16::try_from(lcg_next(rng) % 16 + 1).unwrap_or(1),
                sample_format: if lcg_next(rng).is_multiple_of(2) {
                    wit_types::SampleFormat::Float32
                } else {
                    wit_types::SampleFormat::S16Le
                },
            }),
            1 => wit_types::PacketType::OpusAudio,
            2 => wit_types::PacketType::Text,
            3 => wit_types::PacketType::Binary,
            4 => wit_types::PacketType::Any,
            _ => wit_types::PacketType::Custom(format!("plugin::test/pt@{}", lcg_next(rng) % 100)),
        }
    }

    fn random_packet(rng: &mut u64) -> Packet {
        match lcg_next(rng) % 4 {
            0 => {
                let len = (lcg_next(rng) % 16) as usize;
                let samples: Vec<f32> = (0..len)
                    .map(|i| {
                        // Test-only randomization; bounded by len < 16 so f32
                        // precision drift can't affect anything observable.
                        #[allow(
                            clippy::cast_precision_loss,
                            clippy::suboptimal_flops,
                            clippy::arithmetic_side_effects
                        )]
                        let v = {
                            let r = (lcg_next(rng) % 2001) as f32;
                            let j = i as f32;
                            r / 1000.0 - (j % 2.0)
                        };
                        v
                    })
                    .collect();
                let sample_rate = u32::try_from(lcg_next(rng) % 96_001 + 8_000).unwrap_or(48_000);
                let channels = u16::try_from(lcg_next(rng) % 8 + 1).unwrap_or(1);
                Packet::Audio(streamkit_core::types::AudioFrame::new(
                    sample_rate,
                    channels,
                    samples,
                ))
            },
            1 => {
                let len = (lcg_next(rng) % 64) as usize;
                let text: String =
                    (0..len).map(|_| char::from(b'a' + (lcg_next(rng) % 26) as u8)).collect();
                Packet::Text(Arc::from(text))
            },
            2 => {
                let len = (lcg_next(rng) % 256) as usize;
                let data: Vec<u8> = (0..len).map(|_| (lcg_next(rng) & 0xff) as u8).collect();
                Packet::Binary { data: Bytes::from(data), content_type: None, metadata: None }
            },
            _ => {
                let value = serde_json::json!({
                    "n": lcg_next(rng) % 1000,
                    "tag": format!("t{}", lcg_next(rng) % 50),
                    "arr": [lcg_next(rng) % 10, lcg_next(rng) % 10, lcg_next(rng) % 10],
                });
                Packet::Custom(Arc::new(CustomPacketData {
                    type_id: format!("plugin::test/p@{}", lcg_next(rng) % 100),
                    encoding: CustomEncoding::Json,
                    data: value,
                    metadata: None,
                }))
            },
        }
    }

    fn audio_format_round_trip(rng: &mut u64) -> bool {
        let wit = match random_packet_type(rng) {
            wit_types::PacketType::RawAudio(f) => f,
            _ => wit_types::AudioFormat {
                sample_rate: 48_000,
                channels: 2,
                sample_format: wit_types::SampleFormat::Float32,
            },
        };
        let core = CorePacketType::from(&wit_types::PacketType::RawAudio(wit));
        let CorePacketType::RawAudio(fmt) = core else { return false };
        fmt.sample_rate == wit.sample_rate
            && fmt.channels == wit.channels
            && matches!(
                (fmt.sample_format, wit.sample_format),
                (streamkit_core::types::SampleFormat::F32, wit_types::SampleFormat::Float32,)
                    | (streamkit_core::types::SampleFormat::S16Le, wit_types::SampleFormat::S16Le,)
            )
    }

    fn packets_equivalent(a: &Packet, b: &Packet) -> bool {
        match (a, b) {
            (Packet::Audio(x), Packet::Audio(y)) => {
                x.sample_rate == y.sample_rate
                    && x.channels == y.channels
                    && x.samples.as_slice() == y.samples.as_slice()
            },
            (Packet::Text(x), Packet::Text(y)) => x.as_ref() == y.as_ref(),
            (Packet::Binary { data: dx, .. }, Packet::Binary { data: dy, .. }) => {
                dx.as_ref() == dy.as_ref()
            },
            (Packet::Custom(x), Packet::Custom(y)) => {
                x.type_id == y.type_id
                    && x.data == y.data
                    && matches!(
                        (&x.encoding, &y.encoding),
                        (CustomEncoding::Json, CustomEncoding::Json)
                    )
            },
            _ => false,
        }
    }

    #[test]
    fn round_trip_random_packets_seeded_loop_preserves_equivalence() {
        // 256 iterations across all 4 packet variants — large enough to exercise
        // many sample-count / payload-length combinations while staying fast.
        let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..256 {
            let original = random_packet(&mut rng);
            let round = round_trip_packet(original.clone());
            assert!(
                packets_equivalent(&original, &round),
                "round-trip mismatch on iteration {i}: orig={original:?} round={round:?}"
            );
        }
    }

    #[test]
    fn round_trip_random_audio_formats_preserves_fields_for_all_variants() {
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_F00D;
        for i in 0..128 {
            assert!(
                audio_format_round_trip(&mut rng),
                "audio format round-trip mismatch on iteration {i}"
            );
        }
    }
}
