// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioPacerConfig {
    pub speed: f32,
    #[schemars(range(min = 1))]
    pub buffer_size: usize,
    /// Emit silence when input queue is empty (prevents gaps in real-time streams).
    pub generate_silence: bool,
    /// Set both to start emitting silence immediately instead of waiting for the
    /// first input frame (avoids downstream underflow in slow pipelines).
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

/// Unlike `core::pacer`, generates silence to fill gaps and maintain
/// continuous audio output for real-time streaming.
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Tests use unwrap/expect for concise assertions.
mod tests {
    use super::*;
    use streamkit_core::types::AudioFrame;

    #[test]
    fn audio_duration_mono() {
        let frame = AudioFrame::new(48000, 1, vec![0.0; 960]);
        let dur = AudioPacerNode::calculate_audio_duration(&frame);
        let expected_ms = 20.0;
        let actual_ms = dur.as_secs_f64() * 1000.0;
        assert!(
            (actual_ms - expected_ms).abs() < 0.1,
            "expected ~{expected_ms}ms, got {actual_ms}ms"
        );
    }

    #[test]
    fn audio_duration_stereo() {
        let frame = AudioFrame::new(48000, 2, vec![0.0; 1920]);
        let dur = AudioPacerNode::calculate_audio_duration(&frame);
        let expected_ms = 20.0;
        let actual_ms = dur.as_secs_f64() * 1000.0;
        assert!((actual_ms - expected_ms).abs() < 0.1);
    }

    #[test]
    fn audio_duration_different_sample_rate() {
        let frame = AudioFrame::new(16000, 1, vec![0.0; 16000]);
        let dur = AudioPacerNode::calculate_audio_duration(&frame);
        let expected_ms = 1000.0;
        let actual_ms = dur.as_secs_f64() * 1000.0;
        assert!((actual_ms - expected_ms).abs() < 0.1);
    }

    #[test]
    fn silence_frame_20ms_mono() {
        let frame = AudioPacerNode::create_silence_frame(48000, 1);
        assert_eq!(frame.sample_rate, 48000);
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.samples.len(), 960); // 48000 * 0.020
        assert!(frame.samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silence_frame_20ms_stereo() {
        let frame = AudioPacerNode::create_silence_frame(48000, 2);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples.len(), 1920); // 48000 * 0.020 * 2
        assert!(frame.samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silence_frame_16k_mono() {
        let frame = AudioPacerNode::create_silence_frame(16000, 1);
        assert_eq!(frame.samples.len(), 320); // 16000 * 0.020
    }

    #[test]
    fn cached_silence_miss_creates_and_caches() {
        let mut cached: Option<AudioFrame> = None;
        let frame = AudioPacerNode::get_cached_silence(&mut cached, 48000, 2);
        assert_eq!(frame.sample_rate, 48000);
        assert_eq!(frame.channels, 2);
        assert!(cached.is_some());
    }

    #[test]
    fn cached_silence_hit_returns_clone() {
        let mut cached: Option<AudioFrame> = None;
        let first = AudioPacerNode::get_cached_silence(&mut cached, 48000, 2);
        let second = AudioPacerNode::get_cached_silence(&mut cached, 48000, 2);
        assert_eq!(first.samples.len(), second.samples.len());
        assert_eq!(first.sample_rate, second.sample_rate);
    }

    #[test]
    fn cached_silence_miss_on_format_change() {
        let mut cached: Option<AudioFrame> = None;
        AudioPacerNode::get_cached_silence(&mut cached, 48000, 2);
        let new_frame = AudioPacerNode::get_cached_silence(&mut cached, 16000, 1);
        assert_eq!(new_frame.sample_rate, 16000);
        assert_eq!(new_frame.channels, 1);
        let cached_frame = cached.unwrap();
        assert_eq!(cached_frame.sample_rate, 16000);
    }

    #[test]
    fn speed_2x_halves_duration() {
        let node = AudioPacerNode {
            speed: 2.0,
            buffer_size: 32,
            generate_silence: true,
            initial_format: None,
        };
        let dur = Duration::from_millis(20);
        let adjusted = node.adjust_for_speed(dur);
        assert_eq!(adjusted, Duration::from_millis(10));
    }

    #[test]
    fn speed_1x_unchanged() {
        let node = AudioPacerNode {
            speed: 1.0,
            buffer_size: 32,
            generate_silence: true,
            initial_format: None,
        };
        let dur = Duration::from_millis(20);
        let adjusted = node.adjust_for_speed(dur);
        assert_eq!(adjusted, Duration::from_millis(20));
    }

    #[test]
    fn speed_half_doubles_duration() {
        let node = AudioPacerNode {
            speed: 0.5,
            buffer_size: 32,
            generate_silence: true,
            initial_format: None,
        };
        let dur = Duration::from_millis(20);
        let adjusted = node.adjust_for_speed(dur);
        let target = Duration::from_millis(40);
        let diff = adjusted.abs_diff(target);
        assert!(diff < Duration::from_micros(100), "expected ~40ms, got {adjusted:?}");
    }

    #[test]
    fn factory_default_config_succeeds() {
        let factory = AudioPacerNode::factory();
        assert!(factory(None).is_ok());
    }

    #[test]
    fn factory_valid_config_succeeds() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({
            "speed": 1.5,
            "buffer_size": 16,
            "generate_silence": false
        });
        assert!(factory(Some(&params)).is_ok());
    }

    #[test]
    fn factory_speed_zero_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({ "speed": 0.0 });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("Speed"));
    }

    #[test]
    fn factory_speed_negative_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({ "speed": -1.0 });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("Speed"));
    }

    #[test]
    fn factory_buffer_size_zero_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({ "buffer_size": 0 });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("Buffer size"));
    }

    #[test]
    fn factory_initial_format_only_sample_rate_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({ "initial_sample_rate": 48000 });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("together"));
    }

    #[test]
    fn factory_initial_format_only_channels_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({ "initial_channels": 2 });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("together"));
    }

    #[test]
    fn factory_initial_sample_rate_zero_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({
            "initial_sample_rate": 0,
            "initial_channels": 2
        });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("initial_sample_rate"));
    }

    #[test]
    fn factory_initial_channels_zero_rejected() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({
            "initial_sample_rate": 48000,
            "initial_channels": 0
        });
        let err = factory(Some(&params)).err().expect("should fail");
        assert!(err.to_string().contains("initial_channels"));
    }

    #[test]
    fn factory_valid_initial_format() {
        let factory = AudioPacerNode::factory();
        let params = serde_json::json!({
            "initial_sample_rate": 48000,
            "initial_channels": 2
        });
        assert!(factory(Some(&params)).is_ok());
    }
}
