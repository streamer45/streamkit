// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Audio pacer node - Paces audio output and fills gaps with silence

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Duration;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::types::{AudioFormat, AudioFrame, Packet, PacketType, SampleFormat};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::time::{Instant, Interval, MissedTickBehavior};

/// Configuration for the AudioPacerNode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioPacerConfig {
    /// Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed)
    pub speed: f32,
    /// Maximum number of audio frames to buffer internally
    /// Default: 32 frames (~640ms of audio at 20ms/frame)
    #[schemars(range(min = 1))]
    pub buffer_size: usize,
    /// Generate silence frames when input queue is empty to maintain continuous stream
    /// Prevents gaps in audio output (useful for real-time streaming protocols like MoQ)
    /// Default: true
    pub generate_silence: bool,
    /// Optional initial audio format used to start pacing immediately (before the first input frame).
    ///
    /// Without an initial format, the pacer learns `(sample_rate, channels)` from the first
    /// received frame. For pipelines that may take seconds before producing the first frame
    /// (e.g., STT → LLM → TTS), this can cause downstream consumers to see a long gap and
    /// underflow. Setting these lets the pacer emit silence right away.
    pub initial_sample_rate: Option<u32>,
    pub initial_channels: Option<u16>,
}

impl Default for AudioPacerConfig {
    fn default() -> Self {
        Self {
            speed: 1.0,
            buffer_size: 32,
            generate_silence: true,
            initial_sample_rate: None,
            initial_channels: None,
        }
    }
}

/// An audio-specific pacer that maintains continuous audio streams by generating silence
/// when the input queue is empty.
///
/// Unlike the generic `core::pacer`, this node:
/// - Only works with Audio packets (raw PCM audio)
/// - Generates silence frames to fill gaps in the stream
/// - Maintains audio format consistency (sample rate, channels, format)
/// - Prevents client buffer starvation in real-time streaming scenarios
///
/// Use cases:
/// - Real-time voice agents where TTS generates audio in bursts
/// - Streaming protocols (MoQ, WebRTC) that expect continuous audio
/// - Preventing "audio underflow" errors in client decoders
///
/// Pipeline placement:
/// - After TTS/audio generation, before encoding
/// - Example: `tts → resample → audio::pacer → opus_encoder → transport`
pub struct AudioPacerNode {
    speed: f32,
    buffer_size: usize,
    generate_silence: bool,
    initial_format: Option<(u32, u16)>,
}

impl AudioPacerNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: AudioPacerConfig = config_helpers::parse_config_optional(params)?;

            if config.speed <= 0.0 {
                return Err(StreamKitError::Configuration(
                    "Speed must be greater than 0".to_string(),
                ));
            }

            if config.buffer_size == 0 {
                return Err(StreamKitError::Configuration(
                    "Buffer size must be greater than 0".to_string(),
                ));
            }

            match (config.initial_sample_rate, config.initial_channels) {
                (Some(sample_rate), Some(channels)) => {
                    if sample_rate == 0 {
                        return Err(StreamKitError::Configuration(
                            "initial_sample_rate must be greater than 0".to_string(),
                        ));
                    }
                    if channels == 0 {
                        return Err(StreamKitError::Configuration(
                            "initial_channels must be greater than 0".to_string(),
                        ));
                    }
                },
                (None, None) => {},
                _ => {
                    return Err(StreamKitError::Configuration(
                        "initial_sample_rate and initial_channels must be set together".to_string(),
                    ));
                },
            }

            Ok(Box::new(Self {
                speed: config.speed,
                buffer_size: config.buffer_size,
                generate_silence: config.generate_silence,
                initial_format: config.initial_sample_rate.zip(config.initial_channels),
            }))
        })
    }

    /// Calculate duration from AudioFrame samples
    fn calculate_audio_duration(frame: &AudioFrame) -> Duration {
        #[allow(clippy::cast_precision_loss)]
        let samples_per_channel = frame.samples.len() as f64 / f64::from(frame.channels);
        let duration_secs = samples_per_channel / f64::from(frame.sample_rate);
        Duration::from_secs_f64(duration_secs)
    }

    fn create_silence_frame(sample_rate: u32, channels: u16) -> AudioFrame {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let samples_per_channel = (f64::from(sample_rate) * 0.020) as usize; // 20ms
        let total_samples = samples_per_channel * channels as usize;

        let samples = vec![0.0f32; total_samples];

        AudioFrame::new(sample_rate, channels, samples)
    }

    /// Returns a clone of the cached silence frame (O(1) due to Arc-backed samples).
    fn get_cached_silence(
        cached_silence: &mut Option<AudioFrame>,
        sample_rate: u32,
        channels: u16,
    ) -> AudioFrame {
        if let Some(ref frame) = cached_silence {
            if frame.sample_rate == sample_rate && frame.channels == channels {
                return frame.clone();
            }
        }

        let silence = Self::create_silence_frame(sample_rate, channels);
        *cached_silence = Some(silence.clone());
        silence
    }

    /// Adjust duration by speed multiplier
    fn adjust_for_speed(&self, duration: Duration) -> Duration {
        duration.div_f32(self.speed)
    }
}

#[async_trait]
impl ProcessorNode for AudioPacerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::RawAudio(AudioFormat {
                sample_rate: 0,
                channels: 0,
                sample_format: SampleFormat::F32,
            })],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "AudioPacerNode starting (speed: {}x, buffer_size: {}, generate_silence: {})",
            self.speed,
            self.buffer_size,
            self.generate_silence
        );

        let mut input_rx = context.take_input("in")?;
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut audio_format: Option<(u32, u16)> = self.initial_format;
        let mut cached_silence: Option<AudioFrame> = None;
        let mut audio_queue: VecDeque<AudioFrame> = VecDeque::with_capacity(self.buffer_size);
        let mut interval: Option<Interval> = None;
        let mut frame_duration: Option<Duration> = None;

        let mut frames_sent = 0u64;
        let mut silence_frames_sent = 0u64;
        let mut input_closed = false;

        if let Some((sample_rate, channels)) = audio_format {
            let silence = Self::create_silence_frame(sample_rate, channels);
            let duration = Self::calculate_audio_duration(&silence);
            let adjusted_duration = self.adjust_for_speed(duration);

            cached_silence = Some(silence);

            let mut iv = tokio::time::interval_at(Instant::now(), adjusted_duration);
            // For real-time streaming, skipping ticks permanently drops audio time and will
            // eventually underflow receivers. Burst lets us catch up after scheduler delays.
            iv.set_missed_tick_behavior(MissedTickBehavior::Burst);
            interval = Some(iv);
            frame_duration = Some(adjusted_duration);

            tracing::info!(
                sample_rate,
                channels,
                frame_duration_ms = adjusted_duration.as_millis(),
                "AudioPacerNode prewarmed; emitting silence until first frame arrives"
            );
        }

        loop {
            tokio::select! {
                // Receive audio frames from upstream - only when queue isn't full
                Some(packet) = input_rx.recv(), if !input_closed && audio_queue.len() < self.buffer_size => {
                    match packet {
                        Packet::Audio(frame) => {
                            stats_tracker.received();

                            let detected_format = (frame.sample_rate, frame.channels);
                            if audio_format != Some(detected_format) {
                                let previous_format = audio_format;
                                audio_format = Some(detected_format);
                                tracing::info!(
                                    previous_format = ?previous_format,
                                    sample_rate = frame.sample_rate,
                                    channels = frame.channels,
                                    "Audio format detected/updated"
                                );
                            }

                            let duration = Self::calculate_audio_duration(&frame);
                            let adjusted_duration = self.adjust_for_speed(duration);

                            audio_queue.push_back(frame);

                            if interval.is_none() || frame_duration != Some(adjusted_duration) {
                                if frame_duration.is_some() && frame_duration != Some(adjusted_duration) {
                                    tracing::debug!(
                                        "Frame duration changed from {:?} to {:?}, recreating interval",
                                        frame_duration,
                                        adjusted_duration
                                    );
                                }

                                let start = Instant::now() + adjusted_duration;
                                let mut iv = tokio::time::interval_at(start, adjusted_duration);
                                iv.set_missed_tick_behavior(MissedTickBehavior::Burst);
                                interval = Some(iv);
                                frame_duration = Some(adjusted_duration);

                                tracing::debug!("Started pacing interval: {:?} period", adjusted_duration);
                            }
                        }
                        _ => {
                            tracing::warn!("Received non-audio packet, ignoring");
                        }
                    }
                }

                () = async {
                    if let Some(iv) = &mut interval {
                        iv.tick().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if interval.is_some() && audio_format.is_some() => {
                    if !audio_queue.is_empty() {
                        let Some(frame) = audio_queue.pop_front() else {
                            continue;
                        };

                        if context.output_sender.send("out", Packet::Audio(frame)).await.is_err() {
                            tracing::debug!("Output channel closed, stopping node");
                            break;
                        }

                        stats_tracker.sent();
                        frames_sent += 1;

                        if frames_sent.is_multiple_of(100) {
                            tracing::trace!("Sent {} frames ({} silence)", frames_sent, silence_frames_sent);
                        }
                    } else if self.generate_silence && !input_closed {
                        if let Some((sample_rate, channels)) = audio_format {
                            let silence = Self::get_cached_silence(&mut cached_silence, sample_rate, channels);

                            if context.output_sender.send("out", Packet::Audio(silence)).await.is_err() {
                                tracing::debug!("Output channel closed, stopping node");
                                break;
                            }

                            stats_tracker.sent();
                            silence_frames_sent += 1;
                            frames_sent += 1;

                            if silence_frames_sent.is_multiple_of(50) {
                                tracing::debug!("Generated {} silence frames (total: {})", silence_frames_sent, frames_sent);
                            }
                        }
                    }

                    stats_tracker.maybe_send();
                }

                Some(ctrl_msg) = context.control_rx.recv() => {
                    match ctrl_msg {
                        NodeControlMessage::UpdateParams(params) => {
                            if let Some(speed_value) = params.get("speed") {
                                match speed_value {
                                    serde_json::Value::Number(n) => {
                                        if let Some(speed) = n.as_f64() {
                                            #[allow(clippy::cast_possible_truncation)]
                                            let speed = speed as f32;
                                            if speed > 0.0 {
                                                tracing::info!(
                                                    "AudioPacerNode updating speed: {}x -> {}x",
                                                    self.speed,
                                                    speed
                                                );
                                                self.speed = speed;
                                            } else {
                                                tracing::warn!("AudioPacerNode received invalid speed: {}", speed);
                                            }
                                        }
                                    }
                                    _ => {
                                        tracing::warn!("AudioPacerNode speed parameter must be a number");
                                    }
                                }
                            }
                        }
                        NodeControlMessage::Start => {}
                        NodeControlMessage::Shutdown => {
                            tracing::info!("AudioPacerNode received shutdown signal");
                            break;
                        }
                    }
                }

                else => {
                    if !input_closed {
                        tracing::info!("Input closed, draining {} queued frames", audio_queue.len());
                        input_closed = true;

                        if !self.generate_silence && audio_queue.is_empty() {
                            break;
                        }
                    } else if audio_queue.is_empty() && !self.generate_silence {
                        break;
                    }
                }
            }
        }

        stats_tracker.force_send();
        tracing::info!(
            "AudioPacerNode finished: {} frames sent ({} real, {} silence) at {}x speed",
            frames_sent,
            frames_sent - silence_frames_sent,
            silence_frames_sent,
            self.speed
        );
        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
        Ok(())
    }
}
