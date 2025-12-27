// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pacer node - Paces packet output based on timing metadata or calculated durations

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Duration;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::types::{AudioFrame, Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::time::{Instant, MissedTickBehavior};

/// Configuration for the PacerNode
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PacerConfig {
    /// Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed)
    pub speed: f32,
    /// Maximum number of packets to buffer internally (for backpressure control)
    /// Higher values = more memory, smoother pacing. Lower values = less memory, more backpressure.
    /// Default: 16 packets (~320ms of audio at 20ms/frame)
    #[schemars(range(min = 1))]
    pub buffer_size: usize,
    /// Number of initial packets to send at 10x speed before starting paced delivery.
    /// This builds up a client-side buffer to absorb network jitter.
    /// Default: 0 (no initial burst). Recommended: 5-25 packets for networked streaming.
    pub initial_burst_packets: usize,
}

impl Default for PacerConfig {
    fn default() -> Self {
        Self { speed: 1.0, buffer_size: 16, initial_burst_packets: 0 }
    }
}

/// A node that paces packet output based on timing metadata or calculated durations.
///
/// Uses `tokio::time::interval` with `MissedTickBehavior::Skip` for drift-free pacing.
/// Supports dynamic speed control via runtime parameter updates.
///
/// **Backpressure**: Maintains an internal bounded queue to prevent loading entire files
/// into memory while still allowing smooth pacing. When the queue fills, backpressure
/// propagates upstream to slow down file reading.
///
/// **Initial Burst**: Sends the first N packets at 10x speed (reduced delay) to build up
/// client-side buffers for absorbing network jitter. For example, with 20ms Opus frames,
/// burst packets are sent every 2ms instead of 20ms, building a 500ms buffer in just 50ms.
/// The burst counter resets when there's a significant gap (>300ms) between incoming packets,
/// allowing each logical audio segment (e.g., TTS sentences, AI responses) to get its own
/// initial burst. This distinguishes between real gaps (sentence boundaries) and temporary
/// queue emptying (small chunks from upstream nodes).
///
/// Timing sources (in order of preference):
/// 1. `Packet::Binary.metadata.duration_us` - Explicit duration from demuxer
/// 2. `Packet::Audio.metadata.duration_us` - Explicit duration
/// 3. Calculated from AudioFrame: `samples.len() / (sample_rate * channels)`
/// 4. Zero duration (pass through immediately) for packets without timing info
pub struct PacerNode {
    speed: f32,
    buffer_size: usize,
    initial_burst_packets: usize,
}

impl PacerNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: PacerConfig = config_helpers::parse_config_optional(params)?;

            // Validate speed
            if config.speed <= 0.0 {
                return Err(StreamKitError::Configuration(
                    "Speed must be greater than 0".to_string(),
                ));
            }

            // Validate buffer size
            if config.buffer_size == 0 {
                return Err(StreamKitError::Configuration(
                    "Buffer size must be greater than 0".to_string(),
                ));
            }

            Ok(Box::new(Self {
                speed: config.speed,
                buffer_size: config.buffer_size,
                initial_burst_packets: config.initial_burst_packets,
            }))
        })
    }

    /// Extract duration from packet metadata or calculate from AudioFrame
    fn get_packet_duration(packet: &Packet) -> Duration {
        match packet {
            Packet::Audio(frame) => {
                // Try metadata first
                if let Some(metadata) = &frame.metadata {
                    if let Some(duration_us) = metadata.duration_us {
                        return Duration::from_micros(duration_us);
                    }
                }

                // Fallback: calculate from AudioFrame
                Self::calculate_audio_duration(frame)
            },
            Packet::Binary { metadata, .. } => {
                // Use metadata if available
                metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .map_or(Duration::ZERO, Duration::from_micros)
            },
            Packet::Text(_) | Packet::Transcription(_) | Packet::Custom(_) => Duration::ZERO, // Pass through immediately
        }
    }

    /// Calculate duration from AudioFrame samples
    fn calculate_audio_duration(frame: &AudioFrame) -> Duration {
        // Precision loss is acceptable for audio timing calculations (microsecond precision)
        #[allow(clippy::cast_precision_loss)]
        let samples_per_channel = frame.samples.len() as f64 / f64::from(frame.channels);
        let duration_secs = samples_per_channel / f64::from(frame.sample_rate);
        Duration::from_secs_f64(duration_secs)
    }

    /// Adjust duration by speed multiplier
    fn adjust_for_speed(&self, duration: Duration) -> Duration {
        duration.div_f32(self.speed)
    }
}

#[async_trait]
impl ProcessorNode for PacerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            // Accepts any packet type
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            // Outputs same type as input (passthrough type inference)
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!(
            "PacerNode starting (speed: {}x, buffer_size: {}, initial_burst: {})",
            self.speed,
            self.buffer_size,
            self.initial_burst_packets
        );

        let mut input_rx = context.take_input("in")?;
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Internal bounded queue for backpressure control
        let mut packet_queue: VecDeque<streamkit_core::types::Packet> =
            VecDeque::with_capacity(self.buffer_size);
        let mut interval: Option<tokio::time::Interval> = None;
        let mut current_duration: Option<Duration> = None;
        let mut packet_count = 0u64;
        let mut packets_sent = 0usize;
        let mut last_packet_time = Instant::now();
        // Gap threshold for detecting new segments (e.g., between TTS sentences)
        // A gap longer than this resets the burst counter
        let segment_gap_threshold = Duration::from_millis(300);

        tracing::debug!("PacerNode entering main loop (waiting for packets)");

        loop {
            tokio::select! {
                // Receive packets from upstream - only when queue isn't full (backpressure)
                Some(packet) = input_rx.recv(), if packet_queue.len() < self.buffer_size => {
                    stats_tracker.received();
                    packet_count += 1;

                    // Check for segment gap - reset burst if there's been a significant pause
                    let now = Instant::now();
                    let gap = now.duration_since(last_packet_time);
                    if gap > segment_gap_threshold && packets_sent >= self.initial_burst_packets {
                        tracing::info!(
                            "Detected segment gap ({:?}), resetting burst counter for new audio segment",
                            gap
                        );
                        packets_sent = 0;
                    }
                    last_packet_time = now;

                    // Check if this packet has a duration
                    let duration = Self::get_packet_duration(&packet);
                    let adjusted_duration = self.adjust_for_speed(duration);

                    if adjusted_duration == Duration::ZERO {
                        // Zero-duration packets (e.g., headers) - send immediately without queueing
                        tracing::debug!("Packet #{} has zero duration, sending immediately", packet_count);
                        if context.output_sender.send("out", packet).await.is_err() {
                            tracing::debug!("Output channel closed, stopping node");
                            break;
                        }
                        stats_tracker.sent();
                        stats_tracker.maybe_send();
                        continue; // Skip queuing
                    }

                    // Queue packet for pacing
                    packet_queue.push_back(packet);

                    // If this is the first packet, or duration changed, create/recreate the interval
                    if interval.is_none() || current_duration != Some(adjusted_duration) {
                        if current_duration.is_some() && current_duration != Some(adjusted_duration) {
                            tracing::debug!(
                                "Duration changed from {:?} to {:?}, recreating interval",
                                current_duration,
                                adjusted_duration
                            );
                        }

                        let start = Instant::now() + adjusted_duration;
                        let mut iv = tokio::time::interval_at(start, adjusted_duration);
                        // For real-time streaming, skipping ticks permanently drops packets and can
                        // starve downstream mixers/clients. Burst lets us catch up after scheduler delays.
                        iv.set_missed_tick_behavior(MissedTickBehavior::Burst);
                        interval = Some(iv);
                        current_duration = Some(adjusted_duration);

                        tracing::debug!("Started pacing interval: {:?} period", adjusted_duration);
                    }
                }

                // Wait for interval tick when we have packets to send
                () = async {
                    // Initial burst: send with reduced delay (10% of normal pacing)
                    // This builds buffer faster than real-time while avoiding network congestion
                    if packets_sent < self.initial_burst_packets {
                        if let Some(duration) = current_duration {
                            // Send at 10x speed during burst (2ms instead of 20ms for Opus)
                            tokio::time::sleep(duration / 10).await;
                        }
                        return;
                    }

                    if let Some(iv) = &mut interval {
                        iv.tick().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if !packet_queue.is_empty() => {
                    // Send the next packet from queue
                    if let Some(packet) = packet_queue.pop_front() {
                        let is_burst = packets_sent < self.initial_burst_packets;

                        if is_burst {
                            tracing::debug!(
                                "Initial burst: sending packet {}/{} at 10x speed",
                                packets_sent + 1,
                                self.initial_burst_packets
                            );
                        } else {
                            tracing::trace!(
                                "Tick: sending packet, {} remaining in queue",
                                packet_queue.len()
                            );
                        }

                        if context.output_sender.send("out", packet).await.is_err() {
                            tracing::debug!("Output channel closed, stopping node");
                            break;
                        }
                        stats_tracker.sent();
                        stats_tracker.maybe_send();
                        packets_sent += 1;

                        // Check if next packet has different duration - will recreate interval on next recv
                        if packet_queue.is_empty() {
                            // Queue empty - clear interval but DON'T reset burst counter
                            // The burst is a one-time buffer at stream start, not per-chunk
                            interval = None;
                            current_duration = None;
                        } else if let Some(next_packet) = packet_queue.front() {
                            let next_duration = Self::get_packet_duration(next_packet);
                            let next_adjusted = self.adjust_for_speed(next_duration);

                            // If duration changes, we'll recreate the interval on the next loop iteration
                            // The interval will keep ticking, but we'll check and recreate if needed
                            if next_adjusted != current_duration.unwrap_or(Duration::ZERO) {
                                tracing::trace!(
                                    "Next packet has different duration {:?}, will recreate interval",
                                    next_adjusted
                                );
                            }
                        }
                    }
                }

                // Handle control messages (speed updates)
                Some(ctrl_msg) = context.control_rx.recv() => {
                    match ctrl_msg {
                        NodeControlMessage::UpdateParams(params) => {
                            if let Some(speed_value) = params.get("speed") {
                                match speed_value {
                                    serde_json::Value::Number(n) => {
                                        if let Some(speed) = n.as_f64() {
                                            // Truncation is acceptable for speed parameter (f32 precision sufficient)
                                            #[allow(clippy::cast_possible_truncation)]
                                            let speed = speed as f32;
                                            if speed > 0.0 {
                                                tracing::info!(
                                                    "PacerNode updating speed: {}x -> {}x",
                                                    self.speed,
                                                    speed
                                                );
                                                self.speed = speed;
                                                // Speed change will take effect on next packet
                                            } else {
                                                tracing::warn!(
                                                    "PacerNode received invalid speed: {}",
                                                    speed
                                                );
                                            }
                                        }
                                    }
                                    _ => {
                                        tracing::warn!("PacerNode speed parameter must be a number");
                                    }
                                }
                            }
                        }
                        NodeControlMessage::Start => {
                            // Pacer doesn't implement ready/start lifecycle - ignore
                        }
                        NodeControlMessage::Shutdown => {
                            tracing::info!("PacerNode received shutdown signal");
                            break;
                        }
                    }
                }

                // Input closed - drain remaining queue
                else => {
                    break;
                }
            }
        }

        // Drain any remaining packets in queue
        if !packet_queue.is_empty() {
            tracing::info!(
                "Input closed, draining {} remaining packets from queue",
                packet_queue.len()
            );

            'drain: while let Some(packet) = packet_queue.pop_front() {
                // Get duration for pacing
                let duration = Self::get_packet_duration(&packet);
                let adjusted_duration = self.adjust_for_speed(duration);

                // Wait for pacing interval if needed, but also listen for shutdown
                if adjusted_duration > Duration::ZERO {
                    tokio::select! {
                        () = tokio::time::sleep(adjusted_duration) => {
                            // Pacing delay completed, continue to send
                        }
                        Some(ctrl_msg) = context.control_rx.recv() => {
                            if matches!(ctrl_msg, NodeControlMessage::Shutdown) {
                                tracing::info!(
                                    "PacerNode received shutdown during drain, stopping immediately"
                                );
                                break 'drain;
                            }
                        }
                    }
                }

                if context.output_sender.send("out", packet).await.is_err() {
                    tracing::debug!("Output channel closed, stopping node");
                    break 'drain;
                }
                stats_tracker.sent();
            }
        }

        stats_tracker.force_send();
        tracing::info!(
            "PacerNode finished pacing {} packets at {}x speed ({} burst, {} paced)",
            packet_count,
            self.speed,
            packets_sent.min(self.initial_burst_packets),
            packets_sent.saturating_sub(self.initial_burst_packets)
        );
        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::uninlined_format_args)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use streamkit_core::node::RoutedPacketMessage;
    use streamkit_core::types::PacketMetadata;
    use streamkit_core::NodeStatsUpdate;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_pacer_passes_through_packets() {
        // Simple test - just verify pacer passes packets through
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (_control_tx, control_rx) = mpsc::channel(10);
        let (state_tx, mut state_rx) = mpsc::channel(10);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsUpdate>(10);
        let (mock_sender, mut packet_rx) = mpsc::channel::<RoutedPacketMessage>(10);

        let output_sender = streamkit_core::OutputSender::new(
            "test_pacer".to_string(),
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

        // Create node with very fast speed to minimize test time
        let node = Box::new(PacerNode { speed: 100.0, buffer_size: 16, initial_burst_packets: 0 });
        let node_handle = tokio::spawn(async move { node.run(context).await });

        // Wait for states
        state_rx.recv().await.unwrap(); // Initializing
        state_rx.recv().await.unwrap(); // Running

        // Send packets with short duration
        for i in 0..3 {
            input_tx
                .send(Packet::Binary {
                    data: bytes::Bytes::from(format!("packet{}", i)),
                    content_type: None,
                    metadata: Some(PacketMetadata {
                        timestamp_us: None,
                        duration_us: Some(1_000), // 1ms
                        sequence: Some(i),
                    }),
                })
                .await
                .unwrap();
        }

        // Close input
        drop(input_tx);

        // Collect packets with timeout
        let mut received_count = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(100), packet_rx.recv()).await
        {
            received_count += 1;
        }

        // Wait for node to finish
        let _ = tokio::time::timeout(Duration::from_secs(1), node_handle).await;

        assert_eq!(received_count, 3);
    }
}
