// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Audio resampler node - Changes playback speed by resampling audio data

use async_trait::async_trait;
use rubato::{FastFixedIn, Resampler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use streamkit_core::types::{
    AudioFormat, AudioFrame, Packet, PacketMetadata, PacketType, SampleFormat,
};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, AudioFramePool, InputPin, NodeContext,
    OutputPin, PinCardinality, PooledSamples, ProcessorNode, StreamKitError,
};

/// Configuration for the AudioResamplerNode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AudioResamplerConfig {
    /// Target output sample rate in Hz (e.g., 48000, 24000, 16000)
    /// Input audio will be resampled to this rate
    /// Must be greater than 0
    #[schemars(range(min = 1))]
    pub target_sample_rate: u32,
    /// Fixed chunk size for resampler (default: 960 frames = 20ms at 48kHz)
    /// Larger values = better efficiency but more latency
    #[serde(default = "default_chunk_frames")]
    #[schemars(range(min = 1))]
    pub chunk_frames: usize,
    /// Output frame size - packets will be buffered to this exact size (default: 960 = 20ms at 48kHz)
    /// Must be a valid Opus frame size: 120, 240, 480, 960, 1920, or 2880 samples
    /// Set to 0 to disable output buffering (variable frame sizes)
    #[serde(default = "default_output_frame_size")]
    pub output_frame_size: usize,
}

const fn default_chunk_frames() -> usize {
    960 // 20ms at 48kHz, typical Opus frame size
}

const fn default_output_frame_size() -> usize {
    960 // 20ms at 48kHz - matches Opus default
}

/// A node that resamples audio to convert between different sample rates.
///
/// This node uses rubato's FastFixedIn resampler for efficient, good-quality resampling.
/// Common use cases:
/// - Converting 48kHz to 24kHz (downsampling)
/// - Converting 16kHz to 48kHz (upsampling)
/// - Normalizing various input rates to a standard output rate
///
/// **Note**: Sample rate conversion changes playback speed when interpreted at a fixed rate.
/// This also changes pitch. For time-stretching without pitch change, a different algorithm is needed.
///
/// **Real-time optimizations**:
/// - Resampler is created once and reused
/// - Fixed chunk size for consistent processing
/// - Pre-allocated buffers to avoid runtime allocation
/// - Buffering to handle variable input sizes
pub struct AudioResamplerNode {
    config: AudioResamplerConfig,
}

impl AudioResamplerNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: AudioResamplerConfig = match params {
                Some(p) => config_helpers::parse_config_required(Some(p))?,
                // Default config for schema generation
                None => AudioResamplerConfig {
                    target_sample_rate: 48000, // Default to 48kHz
                    chunk_frames: default_chunk_frames(),
                    output_frame_size: default_output_frame_size(),
                },
            };

            // Validate target_sample_rate
            if config.target_sample_rate == 0 {
                return Err(StreamKitError::Configuration(
                    "target_sample_rate must be greater than 0".to_string(),
                ));
            }

            if config.chunk_frames == 0 {
                return Err(StreamKitError::Configuration(
                    "chunk_frames must be greater than 0".to_string(),
                ));
            }

            // Validate output_frame_size is a valid Opus frame size (or 0 for disabled)
            if config.output_frame_size != 0 {
                let valid_sizes = [120, 240, 480, 960, 1920, 2880];
                if !valid_sizes.contains(&config.output_frame_size) {
                    return Err(StreamKitError::Configuration(format!(
                        "output_frame_size must be 0 (disabled) or a valid Opus frame size: {valid_sizes:?}"
                    )));
                }
            }

            Ok(Box::new(Self { config }))
        })
    }

    fn duration_us_for_frames(sample_rate: u32, frames_per_channel: usize) -> u64 {
        if sample_rate == 0 {
            return 0;
        }
        // Safe casts: media timestamp math, frames_per_channel is bounded by packet sizes.
        #[allow(clippy::cast_possible_truncation)]
        let frames_per_channel = frames_per_channel as u64;
        (frames_per_channel * 1_000_000) / u64::from(sample_rate)
    }
}

#[async_trait]
impl ProcessorNode for AudioResamplerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            // Accept any raw audio format (wildcards for sample_rate/channels).
            accepts_types: vec![PacketType::RawAudio(AudioFormat {
                sample_rate: 0, // wildcard
                channels: 0,    // wildcard
                sample_format: SampleFormat::F32,
            })],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            // Resampling changes sample rate; channels pass through unchanged (wildcard here).
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: self.config.target_sample_rate,
                channels: 0, // wildcard (resampler does not currently enforce channel count)
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "AudioResamplerNode starting with target_sample_rate: {}Hz (chunk_frames: {}, using rubato FastFixedIn)",
            self.config.target_sample_rate,
            self.config.chunk_frames
        );

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;
        let audio_pool: Option<Arc<AudioFramePool>> = context.audio_pool.clone();
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let mut packet_count = 0u64;
        let mut total_input_samples = 0u64;
        let mut total_output_samples = 0u64;

        // State variables for resampler (initialized on first audio packet)
        let mut resampler: Option<FastFixedIn<f32>> = None;
        let mut needs_resample: Option<bool> = None;
        let mut sample_rate: Option<u32> = None;
        let mut channels: Option<u16> = None;
        let mut output_sequence: u64 = 0;
        let mut output_timestamp_us: Option<u64> = None;

        // Pre-allocated buffers for planar format conversion
        // These will be resized as needed but reused across packets
        let mut planar_input_buffer: Vec<Vec<f32>> = Vec::new();
        let mut sample_buffer: Vec<f32> = Vec::new(); // Buffer for accumulating input samples
        let mut sample_buffer_offset: usize = 0;
        let mut output_buffer: Vec<f32> = Vec::new(); // Buffer for accumulating output samples to exact frame size
        let mut output_buffer_offset: usize = 0;

        // Helper to create PooledSamples from a slice, using audio_pool if available
        let make_pooled_samples =
            |data: &[f32], pool: &Option<Arc<AudioFramePool>>| -> PooledSamples {
                pool.as_ref().map_or_else(
                    || PooledSamples::from_vec(data.to_vec()),
                    |p| {
                        let mut samples = p.get(data.len());
                        samples.as_mut_slice().copy_from_slice(data);
                        samples
                    },
                )
            };

        // Process packets and resample audio
        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            stats_tracker.received();
            packet_count += 1;

            match packet {
                Packet::Audio(frame) => {
                    total_input_samples += frame.samples.len() as u64;

                    // Initialize stream state on first audio packet.
                    if needs_resample.is_none() {
                        needs_resample = Some(frame.sample_rate != self.config.target_sample_rate);
                        sample_rate = Some(frame.sample_rate);
                        channels = Some(frame.channels);

                        if output_timestamp_us.is_none() {
                            output_timestamp_us =
                                frame.metadata.as_ref().and_then(|meta| meta.timestamp_us);
                        }

                        if needs_resample == Some(true) {
                            let num_channels = frame.channels as usize;
                            let input_rate = frame.sample_rate;
                            let output_rate = self.config.target_sample_rate;

                            tracing::debug!(
                                "Creating resampler: {}→{} Hz, ratio: {:.4}, chunk_frames: {}, channels: {}",
                                input_rate,
                                output_rate,
                                f64::from(output_rate) / f64::from(input_rate),
                                self.config.chunk_frames,
                                num_channels
                            );

                            // Create resampler once with fixed chunk size
                            resampler = Some(
                                FastFixedIn::<f32>::new(
                                    f64::from(output_rate) / f64::from(input_rate),
                                    1.0, // Maximum relative ratio change (not used for FastFixedIn)
                                    rubato::PolynomialDegree::Linear, // Fast linear interpolation
                                    self.config.chunk_frames,
                                    num_channels,
                                )
                                .map_err(|e| {
                                    StreamKitError::Runtime(format!(
                                        "Failed to create resampler: {e}"
                                    ))
                                })?,
                            );

                            // Pre-allocate planar buffers
                            planar_input_buffer =
                                vec![Vec::with_capacity(self.config.chunk_frames); num_channels];
                        }
                    }

                    // Verify audio format matches
                    if Some(frame.sample_rate) != sample_rate || Some(frame.channels) != channels {
                        let (Some(expected_sample_rate), Some(expected_channels)) =
                            (sample_rate, channels)
                        else {
                            stats_tracker.errored();
                            stats_tracker.force_send();
                            let err_msg =
                                "AudioResamplerNode internal error: missing stream format state"
                                    .to_string();
                            state_helpers::emit_failed(
                                &context.state_tx,
                                &node_name,
                                err_msg.clone(),
                            );
                            return Err(StreamKitError::Runtime(err_msg));
                        };

                        stats_tracker.errored();
                        stats_tracker.force_send();
                        let err_msg = format!(
                            "Audio format changed mid-stream: expected {expected_sample_rate}Hz/{expected_channels}ch, got {}Hz/{}ch",
                            frame.sample_rate,
                            frame.channels
                        );
                        state_helpers::emit_failed(&context.state_tx, &node_name, err_msg.clone());
                        return Err(StreamKitError::Runtime(err_msg));
                    }

                    let target_sample_rate = self.config.target_sample_rate;
                    // Safe unwrap: initialized on first packet
                    #[allow(clippy::unwrap_used)]
                    let num_channels = channels.unwrap() as usize;

                    let mut next_metadata = |duration_us: u64| -> Option<PacketMetadata> {
                        let metadata = PacketMetadata {
                            timestamp_us: output_timestamp_us,
                            duration_us: Some(duration_us),
                            sequence: Some(output_sequence),
                        };
                        output_sequence += 1;
                        if let Some(ts) = output_timestamp_us.as_mut() {
                            *ts += duration_us;
                        }
                        Some(metadata)
                    };

                    if needs_resample == Some(false) {
                        // No resampling required. If output_frame_size is configured, still normalize
                        // output packet sizes to avoid downstream codec pacing/underflow issues.
                        if self.config.output_frame_size == 0 {
                            if context
                                .output_sender
                                .send("out", Packet::Audio(frame))
                                .await
                                .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                break;
                            }
                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                            continue;
                        }

                        output_buffer.extend_from_slice(&frame.samples);
                        total_output_samples += frame.samples.len() as u64;

                        let output_frame_samples = self.config.output_frame_size * num_channels;
                        while output_buffer.len().saturating_sub(output_buffer_offset)
                            >= output_frame_samples
                        {
                            let start = output_buffer_offset;
                            let end = start + output_frame_samples;
                            let frame_samples =
                                make_pooled_samples(&output_buffer[start..end], &audio_pool);
                            output_buffer_offset = end;

                            let duration_us = Self::duration_us_for_frames(
                                target_sample_rate,
                                self.config.output_frame_size,
                            );

                            let out_frame = AudioFrame::from_pooled(
                                target_sample_rate,
                                frame.channels,
                                frame_samples,
                                next_metadata(duration_us),
                            );

                            if context
                                .output_sender
                                .send("out", Packet::Audio(out_frame))
                                .await
                                .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                state_helpers::emit_stopped(
                                    &context.state_tx,
                                    &node_name,
                                    "output_closed",
                                );
                                return Ok(());
                            }

                            stats_tracker.sent();
                        }

                        if output_buffer_offset == output_buffer.len() {
                            output_buffer.clear();
                            output_buffer_offset = 0;
                        } else if output_buffer_offset > 0
                            && (output_buffer_offset >= output_frame_samples.saturating_mul(8)
                                || output_buffer_offset.saturating_mul(2) >= output_buffer.len())
                        {
                            output_buffer.drain(..output_buffer_offset);
                            output_buffer_offset = 0;
                        }

                        stats_tracker.maybe_send();
                        continue;
                    }

                    // Resampling path
                    // Add samples to buffer
                    sample_buffer.extend_from_slice(&frame.samples);

                    // Safe unwrap: resampler is Some when needs_resample is true
                    #[allow(clippy::unwrap_used)]
                    let resampler_ref = resampler.as_mut().unwrap();
                    let chunk_size_samples = self.config.chunk_frames * num_channels;

                    while sample_buffer.len().saturating_sub(sample_buffer_offset)
                        >= chunk_size_samples
                    {
                        let chunk_start = sample_buffer_offset;
                        let chunk_end = chunk_start + chunk_size_samples;
                        let chunk = &sample_buffer[chunk_start..chunk_end];

                        // Clear planar buffers (keep capacity)
                        for ch_buf in &mut planar_input_buffer {
                            ch_buf.clear();
                        }

                        // Convert chunk to planar format
                        for frame_idx in 0..self.config.chunk_frames {
                            for ch in 0..num_channels {
                                planar_input_buffer[ch].push(chunk[frame_idx * num_channels + ch]);
                            }
                        }

                        // Resample
                        let planar_output =
                            resampler_ref.process(&planar_input_buffer, None).map_err(|e| {
                                StreamKitError::Runtime(format!("Resampling failed: {e}"))
                            })?;

                        // Convert planar output back to interleaved format
                        let output_frames = planar_output[0].len();
                        if self.config.output_frame_size > 0 {
                            output_buffer.reserve(output_frames * num_channels);
                            for frame_idx in 0..output_frames {
                                for channel_data in planar_output.iter().take(num_channels) {
                                    output_buffer.push(channel_data[frame_idx]);
                                }
                            }
                            total_output_samples += (output_frames * num_channels) as u64;

                            let output_frame_samples = self.config.output_frame_size * num_channels;
                            while output_buffer.len().saturating_sub(output_buffer_offset)
                                >= output_frame_samples
                            {
                                let start = output_buffer_offset;
                                let end = start + output_frame_samples;
                                let frame_samples =
                                    make_pooled_samples(&output_buffer[start..end], &audio_pool);
                                output_buffer_offset = end;

                                let duration_us = Self::duration_us_for_frames(
                                    target_sample_rate,
                                    self.config.output_frame_size,
                                );

                                let out_frame = AudioFrame::from_pooled(
                                    target_sample_rate,
                                    frame.channels,
                                    frame_samples,
                                    next_metadata(duration_us),
                                );

                                if context
                                    .output_sender
                                    .send("out", Packet::Audio(out_frame))
                                    .await
                                    .is_err()
                                {
                                    tracing::debug!("Output channel closed, stopping node");
                                    state_helpers::emit_stopped(
                                        &context.state_tx,
                                        &node_name,
                                        "output_closed",
                                    );
                                    return Ok(());
                                }

                                stats_tracker.sent();
                            }

                            if output_buffer_offset == output_buffer.len() {
                                output_buffer.clear();
                                output_buffer_offset = 0;
                            } else if output_buffer_offset > 0
                                && (output_buffer_offset >= output_frame_samples.saturating_mul(8)
                                    || output_buffer_offset.saturating_mul(2)
                                        >= output_buffer.len())
                            {
                                output_buffer.drain(..output_buffer_offset);
                                output_buffer_offset = 0;
                            }
                        } else {
                            let mut interleaved_output =
                                Vec::with_capacity(output_frames * num_channels);
                            for frame_idx in 0..output_frames {
                                for channel_data in planar_output.iter().take(num_channels) {
                                    interleaved_output.push(channel_data[frame_idx]);
                                }
                            }
                            total_output_samples += interleaved_output.len() as u64;

                            let frames_per_channel = interleaved_output.len() / num_channels;
                            let duration_us = Self::duration_us_for_frames(
                                target_sample_rate,
                                frames_per_channel,
                            );

                            let out_frame = AudioFrame::with_metadata(
                                target_sample_rate,
                                frame.channels,
                                interleaved_output,
                                next_metadata(duration_us),
                            );

                            if context
                                .output_sender
                                .send("out", Packet::Audio(out_frame))
                                .await
                                .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                state_helpers::emit_stopped(
                                    &context.state_tx,
                                    &node_name,
                                    "output_closed",
                                );
                                return Ok(());
                            }

                            stats_tracker.sent();
                        }

                        // Mark processed samples as consumed (compaction happens opportunistically).
                        sample_buffer_offset += chunk_size_samples;
                    }

                    if sample_buffer_offset == sample_buffer.len() {
                        sample_buffer.clear();
                        sample_buffer_offset = 0;
                    } else if sample_buffer_offset > 0
                        && (sample_buffer_offset >= chunk_size_samples.saturating_mul(4)
                            || sample_buffer_offset.saturating_mul(2) >= sample_buffer.len())
                    {
                        sample_buffer.drain(..sample_buffer_offset);
                        sample_buffer_offset = 0;
                    }

                    stats_tracker.maybe_send();
                },
                other => {
                    // Pass through non-audio packets unchanged
                    tracing::debug!("Passing through non-audio packet");
                    if context.output_sender.send("out", other).await.is_err() {
                        tracing::debug!("Output channel closed, stopping node");
                        break;
                    }
                    stats_tracker.sent();
                    stats_tracker.maybe_send();
                },
            }
        }

        // Process any remaining buffered samples (resampling path only)
        if sample_buffer.len() > sample_buffer_offset && needs_resample == Some(true) {
            // Safe unwraps: channels and sample_rate are Some when resampler is Some
            let Some(channels_u16) = channels else {
                return Err(StreamKitError::Runtime(
                    "Resampler ended with pending samples but no channel count".to_string(),
                ));
            };
            let num_channels = channels_u16 as usize;
            let remaining_samples = sample_buffer.len() - sample_buffer_offset;
            let remaining_frames = remaining_samples / num_channels;

            tracing::debug!("Processing {} remaining frames", remaining_frames);

            // For remaining samples, we need to create a new resampler with the exact size
            // or pad to chunk_frames
            if remaining_frames > 0 {
                #[allow(clippy::unwrap_used)]
                let input_rate = sample_rate.unwrap();
                let output_rate = self.config.target_sample_rate;

                // Create a temporary resampler for the remainder
                let mut remainder_resampler = FastFixedIn::<f32>::new(
                    f64::from(output_rate) / f64::from(input_rate),
                    1.0,
                    rubato::PolynomialDegree::Linear,
                    remaining_frames,
                    num_channels,
                )
                .map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create remainder resampler: {e}"))
                })?;

                // Convert remaining samples to planar
                let mut planar_remainder: Vec<Vec<f32>> =
                    vec![Vec::with_capacity(remaining_frames); num_channels];
                let remainder_samples = &sample_buffer[sample_buffer_offset..];
                for frame_idx in 0..remaining_frames {
                    for ch in 0..num_channels {
                        planar_remainder[ch].push(remainder_samples[frame_idx * num_channels + ch]);
                    }
                }

                // Resample remainder
                let planar_output =
                    remainder_resampler.process(&planar_remainder, None).map_err(|e| {
                        StreamKitError::Runtime(format!("Resampling remainder failed: {e}"))
                    })?;

                // Convert to interleaved
                let output_frames = planar_output[0].len();
                let mut interleaved_output = Vec::with_capacity(output_frames * num_channels);
                for frame_idx in 0..output_frames {
                    for channel_data in planar_output.iter().take(num_channels) {
                        interleaved_output.push(channel_data[frame_idx]);
                    }
                }

                total_output_samples += interleaved_output.len() as u64;

                if self.config.output_frame_size > 0 {
                    output_buffer.extend_from_slice(&interleaved_output);

                    let output_frame_samples = self.config.output_frame_size * num_channels;
                    while output_buffer.len().saturating_sub(output_buffer_offset)
                        >= output_frame_samples
                    {
                        let start = output_buffer_offset;
                        let end = start + output_frame_samples;
                        let frame_samples =
                            make_pooled_samples(&output_buffer[start..end], &audio_pool);
                        output_buffer_offset = end;

                        let duration_us = Self::duration_us_for_frames(
                            self.config.target_sample_rate,
                            self.config.output_frame_size,
                        );

                        let metadata = PacketMetadata {
                            timestamp_us: output_timestamp_us,
                            duration_us: Some(duration_us),
                            sequence: Some(output_sequence),
                        };
                        output_sequence += 1;
                        if let Some(ts) = output_timestamp_us.as_mut() {
                            *ts += duration_us;
                        }

                        let out_frame = AudioFrame::from_pooled(
                            self.config.target_sample_rate,
                            channels_u16,
                            frame_samples,
                            Some(metadata),
                        );

                        if context
                            .output_sender
                            .send("out", Packet::Audio(out_frame))
                            .await
                            .is_err()
                        {
                            tracing::debug!("Output channel closed, stopping node");
                            return Ok(());
                        }

                        stats_tracker.sent();
                    }

                    if output_buffer_offset == output_buffer.len() {
                        output_buffer.clear();
                        output_buffer_offset = 0;
                    } else if output_buffer_offset > 0
                        && (output_buffer_offset >= output_frame_samples.saturating_mul(8)
                            || output_buffer_offset.saturating_mul(2) >= output_buffer.len())
                    {
                        output_buffer.drain(..output_buffer_offset);
                        output_buffer_offset = 0;
                    }
                } else {
                    let duration_us =
                        Self::duration_us_for_frames(self.config.target_sample_rate, output_frames);
                    let metadata = PacketMetadata {
                        timestamp_us: output_timestamp_us,
                        duration_us: Some(duration_us),
                        sequence: Some(output_sequence),
                    };
                    output_sequence += 1;
                    if let Some(ts) = output_timestamp_us.as_mut() {
                        *ts += duration_us;
                    }

                    let out_frame = AudioFrame::with_metadata(
                        self.config.target_sample_rate,
                        channels_u16,
                        interleaved_output,
                        Some(metadata),
                    );

                    if context.output_sender.send("out", Packet::Audio(out_frame)).await.is_err() {
                        tracing::debug!("Output channel closed, stopping node");
                        return Ok(());
                    }

                    stats_tracker.sent();
                }
            }
        }

        // Flush any remaining output buffer samples
        if output_buffer.len() > output_buffer_offset && self.config.output_frame_size > 0 {
            if output_buffer_offset > 0 {
                output_buffer.drain(..output_buffer_offset);
            }
            tracing::debug!("Flushing {} remaining output samples", output_buffer.len());

            let Some(channels_u16) = channels else {
                return Err(StreamKitError::Runtime(
                    "Resampler ended with pending output but no channel count".to_string(),
                ));
            };
            let num_channels = channels_u16 as usize;
            let frames_per_channel = output_buffer.len() / num_channels;
            let duration_us =
                Self::duration_us_for_frames(self.config.target_sample_rate, frames_per_channel);

            let metadata = PacketMetadata {
                timestamp_us: output_timestamp_us,
                duration_us: Some(duration_us),
                sequence: Some(output_sequence),
            };
            if let Some(ts) = output_timestamp_us.as_mut() {
                *ts += duration_us;
            }

            let resampled_frame = AudioFrame::with_metadata(
                self.config.target_sample_rate,
                channels_u16,
                output_buffer,
                Some(metadata),
            );

            if context.output_sender.send("out", Packet::Audio(resampled_frame)).await.is_err() {
                tracing::debug!("Output channel closed, stopping node");
                // Can't break here, we're in cleanup after loop
                return Ok(());
            }

            stats_tracker.sent();
        }

        stats_tracker.force_send();
        tracing::info!(
            "AudioResamplerNode processed {} packets, resampled to {}Hz ({} -> {} samples)",
            packet_count,
            self.config.target_sample_rate,
            total_input_samples,
            total_output_samples
        );

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use streamkit_core::node::RoutedPacketMessage;
    use streamkit_core::NodeStatsUpdate;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_audio_resampler_structure() {
        let config = AudioResamplerConfig {
            target_sample_rate: 24000,
            chunk_frames: 960,
            output_frame_size: 0, // Disabled for this test
        };
        let node = Box::new(AudioResamplerNode { config });

        // Verify pins
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.input_pins()[0].name, "in");
        assert_eq!(node.output_pins().len(), 1);
        assert_eq!(node.output_pins()[0].name, "out");
    }

    #[tokio::test]
    async fn test_audio_resampler_downsample() {
        // Create test context
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);
        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_audio_resampler".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs,
            control_rx,
            output_sender,
            batch_size: 32,
            state_tx,
            stats_tx: Some(stats_tx),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: None,
            pin_management_rx: None, // Test contexts don't support dynamic pins
            audio_pool: None,
        };

        // Create node that downsamples from 48kHz to 24kHz
        let config = AudioResamplerConfig {
            target_sample_rate: 24000,
            chunk_frames: 960,
            output_frame_size: 0, // Disabled for this test
        };
        let node = Box::new(AudioResamplerNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for states
        state_rx.recv().await.unwrap(); // Initializing
        state_rx.recv().await.unwrap(); // Running

        // Send audio packet with 960 samples (480 frames stereo) - exactly one chunk
        let input_samples = vec![0.5; 960];
        let audio_packet = Packet::Audio(AudioFrame::new(48000, 2, input_samples.clone()));

        input_tx.send(audio_packet).await.unwrap();
        drop(input_tx);

        // Receive resampled packet
        let (_node, _pin, resampled_packet) = packet_rx.recv().await.unwrap();

        if let Packet::Audio(frame) = resampled_packet {
            // Downsampling 48kHz->24kHz = approximately half the samples
            let expected_samples = 480;
            let tolerance = 10;
            assert!(
                (frame.samples.len() as i32 - expected_samples).abs() < tolerance,
                "Expected ~{} samples, got {}",
                expected_samples,
                frame.samples.len()
            );
            assert_eq!(frame.sample_rate, 24000);
            assert_eq!(frame.channels, 2);
            // Note: Metadata is not tested here due to buffering complexity
        } else {
            panic!("Expected Audio packet");
        }

        state_rx.recv().await.unwrap(); // Stopped
        node_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_audio_resampler_buffering() {
        // Test that resampler properly buffers across multiple packets
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);
        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_audio_resampler_buffer".to_string(),
            streamkit_core::node::OutputRouting::Routed(mock_sender),
        );

        let context = NodeContext {
            inputs,
            control_rx,
            output_sender,
            batch_size: 32,
            state_tx,
            stats_tx: Some(stats_tx),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: None,
            pin_management_rx: None, // Test contexts don't support dynamic pins
            audio_pool: None,
        };

        let config = AudioResamplerConfig {
            target_sample_rate: 24000,
            chunk_frames: 960,    // Chunk size
            output_frame_size: 0, // Disabled for this test
        };
        let node = Box::new(AudioResamplerNode { config });

        let node_handle = tokio::spawn(async move { node.run(context).await });

        state_rx.recv().await.unwrap(); // Initializing
        state_rx.recv().await.unwrap(); // Running

        // Send multiple small packets that need to be buffered
        // 3 packets of 480 samples = 1440 samples total
        // Should produce 1 full chunk (960 samples) + remainder
        for _ in 0..3 {
            let audio_packet = Packet::Audio(AudioFrame::new(48000, 2, vec![0.5; 480]));
            input_tx.send(audio_packet).await.unwrap();
        }
        drop(input_tx);

        // Should receive at least 1 packet (from full chunk)
        let (_node, _pin, resampled_packet) = packet_rx.recv().await.unwrap();
        if let Packet::Audio(frame) = resampled_packet {
            // First packet from full chunk
            assert!(!frame.samples.is_empty());
        } else {
            panic!("Expected Audio packet");
        }

        // May receive another packet from remainder
        // Just drain any remaining packets
        while packet_rx.try_recv().is_ok() {}

        state_rx.recv().await.unwrap(); // Stopped
        node_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_audio_resampler_invalid_sample_rate() {
        // Test that zero target_sample_rate is rejected
        let factory = AudioResamplerNode::factory();
        let params = serde_json::json!({ "target_sample_rate": 0 });
        let result = factory(Some(&params));
        assert!(result.is_err());
    }
}
