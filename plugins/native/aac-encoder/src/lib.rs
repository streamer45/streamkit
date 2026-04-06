// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! AAC-LC audio encoder native plugin using FDK AAC (Fraunhofer).
//!
//! Accepts 48 kHz mono or stereo f32 PCM audio on pin `"in"`, encodes to AAC-LC
//! (ISO 14496-3 profile 2), and emits raw AAC frames on pin `"out"` as
//! `Packet::Binary` with `content_type = "audio/aac"` and per-packet
//! timing metadata.
//!
//! The encoder always produces stereo output at 48 kHz.  Mono input is
//! automatically upmixed (duplicated to both channels).  The AAC frame size
//! is 1024 samples per channel.

use serde::Deserialize;
use streamkit_plugin_sdk_native::prelude::*;
use streamkit_plugin_sdk_native::streamkit_core::types::{
    AudioFormat, PacketMetadata, SampleFormat,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const AAC_SAMPLE_RATE: u32 = 48_000;
const AAC_CHANNELS: u16 = 2;
const AAC_FRAME_SAMPLES: usize = 1024;
/// Duration of one AAC frame in microseconds: 1024 / 48000 × 1_000_000.
const AAC_FRAME_DURATION_US: u64 = 21_333;
const AAC_CONTENT_TYPE: &str = "audio/aac";

const DEFAULT_BITRATE: usize = 128_000;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AacEncoderConfig {
    /// Target bitrate in bits per second (default: 128 000).
    #[serde(default = "default_bitrate")]
    bitrate: usize,
}

const fn default_bitrate() -> usize {
    DEFAULT_BITRATE
}

impl Default for AacEncoderConfig {
    fn default() -> Self {
        Self { bitrate: DEFAULT_BITRATE }
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

pub struct AacEncoderNode {
    encoder: shiguredo_fdk_aac::Encoder,
    /// Residual f32 samples that didn't fill a complete 1024×2 frame.
    residual: Vec<f32>,
    /// Running sequence counter (also used to compute drift-free timestamps).
    sequence: u64,
    logger: Logger,
}

impl AacEncoderNode {
    /// Convert an f32 sample (−1.0 … 1.0) to i16.
    #[inline]
    fn f32_to_i16(s: f32) -> i16 {
        let clamped = s.clamp(-1.0, 1.0);
        // Scale to i16 range.  Using i16::MAX as f32 is exact enough for
        // audio — the rounding error is < 1 LSB.
        #[allow(clippy::cast_possible_truncation)]
        let v = (clamped * f32::from(i16::MAX)) as i16;
        v
    }

    /// Drain the residual buffer, encoding complete 1024-sample (stereo)
    /// frames and sending each one downstream.
    fn encode_residual(&mut self, output: &OutputSender) -> Result<(), String> {
        let frame_len = AAC_FRAME_SAMPLES * usize::from(AAC_CHANNELS);

        while self.residual.len() >= frame_len {
            let chunk: Vec<i16> = self.residual.drain(..frame_len).map(Self::f32_to_i16).collect();

            if let Some(frame) =
                self.encoder.encode(&chunk).map_err(|e| format!("AAC encode error: {e}"))?
            {
                self.emit_frame(&frame.data, output)?;
            }
        }

        Ok(())
    }

    /// Send one encoded AAC frame downstream with timing metadata.
    fn emit_frame(&mut self, data: &[u8], output: &OutputSender) -> Result<(), String> {
        // Compute timestamp from frame count to avoid accumulating truncation
        // drift.  1024 samples / 48 000 Hz = 21.333… µs per frame; using
        // integer arithmetic: sequence * 1024 * 1_000_000 / 48_000.
        let timestamp_us =
            (self.sequence as u128 * 1_024 * 1_000_000 / 48_000) as u64;

        let packet = Packet::Binary {
            data: bytes::Bytes::copy_from_slice(data),
            content_type: Some(std::borrow::Cow::Borrowed(AAC_CONTENT_TYPE)),
            metadata: Some(PacketMetadata {
                timestamp_us: Some(timestamp_us),
                duration_us: Some(AAC_FRAME_DURATION_US),
                sequence: Some(self.sequence),
                keyframe: None,
            }),
        };
        output.send("out", &packet)?;

        self.sequence += 1;
        Ok(())
    }
}

impl NativeProcessorNode for AacEncoderNode {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("aac_encoder")
            .description(
                "AAC-LC audio encoder (48 kHz, mono or stereo) using FDK AAC.  \
                 Accepts f32 PCM on pin \"in\", outputs raw AAC frames on pin \"out\".",
            )
            .input(
                "in",
                &[
                    PacketType::RawAudio(AudioFormat {
                        sample_rate: AAC_SAMPLE_RATE,
                        channels: 2,
                        sample_format: SampleFormat::F32,
                    }),
                    PacketType::RawAudio(AudioFormat {
                        sample_rate: AAC_SAMPLE_RATE,
                        channels: 1,
                        sample_format: SampleFormat::F32,
                    }),
                ],
            )
            // NOTE: The output type is `PacketType::Binary` rather than
            // `PacketType::EncodedAudio(Aac)` because the native plugin C ABI
            // does not yet have a discriminant for `EncodedAudio`.  The
            // `BinaryWithMeta` transport preserves `content_type` and metadata
            // so downstream nodes that inspect these fields (e.g. MP4 muxer)
            // can still identify the codec.  MoQ transport nodes, however,
            // expect `EncodedAudio` and will reject `Binary` — this is a known
            // limitation until `EncodedAudio` support is added to the C ABI.
            .output("out", PacketType::Binary)
            .param_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "bitrate": {
                        "type": "integer",
                        "description": "Target bitrate in bits per second",
                        "default": DEFAULT_BITRATE,
                        "minimum": 16_000,
                        "maximum": 576_000
                    }
                }
            }))
            .category("audio")
            .category("codec")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        let config: AacEncoderConfig = match params {
            Some(v) => serde_json::from_value(v).map_err(|e| format!("Invalid params: {e}"))?,
            None => AacEncoderConfig::default(),
        };

        plugin_info!(logger, "Creating AAC encoder: bitrate={}", config.bitrate);

        let encoder_config = shiguredo_fdk_aac::EncoderConfig { target_bitrate: config.bitrate };

        let encoder =
            shiguredo_fdk_aac::Encoder::new(encoder_config).map_err(|e| format!("{e}"))?;

        plugin_info!(logger, "AAC encoder created successfully");

        Ok(Self {
            encoder,
            residual: Vec::with_capacity(AAC_FRAME_SAMPLES * usize::from(AAC_CHANNELS) * 2),
            sequence: 0,
            logger,
        })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        let frame = match packet {
            Packet::Audio(f) => f,
            other => {
                plugin_warn!(self.logger, "Ignoring non-audio packet: {other:?}");
                return Ok(());
            },
        };

        if frame.sample_rate != AAC_SAMPLE_RATE {
            return Err(format!(
                "AAC encoder requires {}Hz, got {}Hz",
                AAC_SAMPLE_RATE, frame.sample_rate
            ));
        }

        // The FDK AAC encoder requires stereo (interleaved L/R).  If the
        // input is mono, duplicate each sample to both channels.
        match frame.channels {
            1 => {
                let samples: &[f32] = &frame.samples;
                self.residual.reserve(samples.len() * 2);
                for &s in samples {
                    self.residual.push(s);
                    self.residual.push(s);
                }
            },
            2 => {
                self.residual.extend_from_slice(&frame.samples);
            },
            other => {
                return Err(format!("AAC encoder supports 1 or 2 channels, got {other}"));
            },
        }
        self.encode_residual(output)
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), String> {
        plugin_info!(
            self.logger,
            "Flushing AAC encoder ({} residual samples)",
            self.residual.len()
        );

        // Pad the residual to a full frame and encode.
        let frame_len = AAC_FRAME_SAMPLES * usize::from(AAC_CHANNELS);
        if !self.residual.is_empty() {
            self.residual.resize(frame_len, 0.0);
            let chunk: Vec<i16> = self.residual.drain(..frame_len).map(Self::f32_to_i16).collect();

            if let Some(frame) = self
                .encoder
                .encode(&chunk)
                .map_err(|e| format!("AAC encode error during flush: {e}"))?
            {
                self.emit_frame(&frame.data, output)?;
            }
        }

        // Ask the encoder to flush any internally buffered data.
        if let Some(frame) =
            self.encoder.finish().map_err(|e| format!("AAC encoder finish error: {e}"))?
        {
            self.emit_frame(&frame.data, output)?;
        }

        plugin_info!(self.logger, "AAC encoder flushed — {} frames total", self.sequence);
        Ok(())
    }

    fn cleanup(&mut self) {
        plugin_info!(self.logger, "AAC encoder cleanup");
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

streamkit_plugin_sdk_native::native_plugin_entry!(AacEncoderNode);
