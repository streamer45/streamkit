// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::wit_types;
use bytes::Bytes;
use std::sync::Arc;
use streamkit_core::types::{
    AudioFormat as CoreAudioFormat, CustomEncoding, CustomPacketData, PacketType as CorePacketType,
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
            wit_types::PacketType::OpusAudio => Self::OpusAudio,
            wit_types::PacketType::Text => Self::Text,
            wit_types::PacketType::Binary => Self::Binary,
            wit_types::PacketType::Custom(type_id) => Self::Custom { type_id: type_id.clone() },
            wit_types::PacketType::Any => Self::Any,
        }
    }
}
