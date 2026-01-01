// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{AudioFormat, AudioFrame, Packet, PacketType, SampleFormat};
use streamkit_core::{
    get_codec_channel_capacity, packet_helpers, state_helpers, AudioFramePool, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, PooledSamples, ProcessorNode,
    StreamKitError,
};
use tokio::sync::mpsc;

// --- Opus Constants ---

/// Standard Opus sample rate (48 kHz)
const OPUS_SAMPLE_RATE: u32 = 48000;

/// Maximum Opus frame size in samples (20ms at 48kHz)
const OPUS_MAX_FRAME_SIZE: usize = 1920;

/// Output buffer size for encoded Opus packets
const OPUS_OUTPUT_BUFFER_SIZE: usize = 4000;

// --- Opus Decoder ---

#[derive(Deserialize, Debug, Default, JsonSchema)]
#[serde(default)]
pub struct OpusDecoderConfig {}

/// A node that decodes Opus packets into raw audio frames.
pub struct OpusDecoderNode {
    _config: OpusDecoderConfig,
}

impl OpusDecoderNode {
    /// Creates a new Opus decoder node.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok`, but the signature allows for future error cases
    /// (e.g., if decoder initialization fails).
    pub const fn new(config: OpusDecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { _config: config })
    }
}

#[async_trait]
impl ProcessorNode for OpusDecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::OpusAudio],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: OPUS_SAMPLE_RATE,
                channels: 1,
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("OpusDecoderNode starting");
        let mut input_rx = context.take_input("in")?;
        let audio_pool: Option<Arc<AudioFramePool>> = context.audio_pool.clone();

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("opus_packets_processed").build();
        let decode_duration_histogram = meter.f64_histogram("opus_decode_duration").build();

        // Create channels for communication with the blocking task
        // Now includes metadata with each packet
        let (decode_tx, mut decode_rx) = mpsc::channel::<(
            Bytes,
            Option<streamkit_core::types::PacketMetadata>,
        )>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) = mpsc::channel::<(
            Result<PooledSamples, String>,
            Option<streamkit_core::types::PacketMetadata>,
        )>(get_codec_channel_capacity());

        // Spawn a single blocking task that will handle all decode operations
        // Uses blocking_recv/blocking_send for efficiency - no need for block_on
        let decode_task = tokio::task::spawn_blocking(move || {
            let mut decoder = match opus::Decoder::new(OPUS_SAMPLE_RATE, opus::Channels::Mono) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Failed to create Opus decoder: {}", e);
                    return;
                },
            };

            // Reusable decode buffer - avoids allocation per frame (~7.5KB savings per decode)
            // This buffer lives for the lifetime of the decode task
            let mut decode_buffer = vec![0f32; OPUS_MAX_FRAME_SIZE];

            // Use blocking_recv - efficient for spawn_blocking context
            while let Some((data, metadata)) = decode_rx.blocking_recv() {
                let decode_start_time = Instant::now();

                let result = {
                    // Note: No need to zero the buffer - opus writes to it and we only
                    // copy out decoded_len samples, so stale data is never read.
                    match decoder.decode_float(&data, &mut decode_buffer, false) {
                        Ok(decoded_len) => audio_pool.as_ref().map_or_else(
                            || Ok(PooledSamples::from_vec(decode_buffer[..decoded_len].to_vec())),
                            |pool| {
                                let mut samples = pool.get(decoded_len);
                                samples
                                    .as_mut_slice()
                                    .copy_from_slice(&decode_buffer[..decoded_len]);
                                Ok(samples)
                            },
                        ),
                        Err(e) => Err(e.to_string()),
                    }
                };

                decode_duration_histogram.record(decode_start_time.elapsed().as_secs_f64(), &[]);

                // Use blocking_send - efficient for spawn_blocking context
                if result_tx.blocking_send((result, metadata)).is_err() {
                    break; // Main task has shut down
                }
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut audio_packet_count = 0;

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Process input packets and send them for decoding
        let decode_tx_clone = decode_tx.clone();
        let batch_size = context.batch_size;
        let mut input_task = tokio::spawn(async move {
            let mut packet_count = 0;
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Binary { data, metadata, .. } = packet {
                        packet_count += 1;

                        // Skip Opus header packets - they start with "OpusHead" or "OpusTags"
                        if data.len() >= 8
                            && (&data[0..8] == b"OpusHead" || &data[0..8] == b"OpusTags")
                        {
                            tracing::debug!(
                                packet_num = packet_count,
                                size = data.len(),
                                "Skipping Opus header packet"
                            );
                            continue;
                        }

                        // Skip very small packets that are likely metadata (less than 10 bytes)
                        if data.len() < 10 {
                            tracing::debug!(
                                packet_num = packet_count,
                                size = data.len(),
                                "Skipping small metadata packet"
                            );
                            continue;
                        }

                        // Send to blocking task for decoding (with metadata)
                        if decode_tx_clone.send((data, metadata)).await.is_ok() {
                            // Note: We don't have access to stats_tracker in this closure,
                            // so we'll track stats in the main loop where we process results
                        } else {
                            tracing::error!("Decode task has shut down unexpectedly");
                            return;
                        }
                    }
                }
            }
            tracing::info!("OpusDecoderNode input stream closed");
        });

        // Process results from the blocking task
        loop {
            tokio::select! {
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some((Ok(decoded_samples), metadata)) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                            stats_tracker.received();

                            if !decoded_samples.is_empty() {
                                audio_packet_count += 1;

                                let output_frame = AudioFrame::from_pooled(
                                    OPUS_SAMPLE_RATE,
                                    1,
                                    decoded_samples,
                                    metadata, // Propagate metadata from input packet
                                );
                                if context
                                    .output_sender
                                    .send("out", Packet::Audio(output_frame))
                                    .await
                                    .is_err()
                                {
                                    tracing::debug!("Output channel closed, stopping node");
                                    break;
                                }
                                stats_tracker.sent();
                            }
                            stats_tracker.maybe_send();
                        }
                        Some((Err(e), _metadata)) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "error")]);
                            stats_tracker.received();
                            stats_tracker.errored();
                            stats_tracker.maybe_send();
                            tracing::warn!("Decode error for packet: {}", e);
                            // Don't fail the entire node for decode errors, just skip the packet
                        }
                        None => {
                            // Result channel closed, blocking task is done
                            break;
                        }
                    }
                }
                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("OpusDecoderNode received shutdown signal");
                        // Abort input task
                        input_task.abort();
                        // Abort blocking decode task for immediate shutdown
                        decode_task.abort();
                        // Signal blocking task to shut down (in case it's still running)
                        drop(decode_tx);
                        // Break out of main loop
                        break;
                    }
                    // Ignore other control messages
                }
                _ = &mut input_task => {
                    // Input task finished, signal blocking task to shut down
                    drop(decode_tx);

                    // Continue processing any remaining results, but also check for shutdown
                    loop {
                        tokio::select! {
                            maybe_result = result_rx.recv() => {
                                match maybe_result {
                                    Some((Ok(decoded_samples), metadata)) => {
                                        packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                                        stats_tracker.received();

                                        if !decoded_samples.is_empty() {
                                            audio_packet_count += 1;

                                            let output_frame = AudioFrame::from_pooled(
                                                OPUS_SAMPLE_RATE,
                                                1,
                                                decoded_samples,
                                                metadata, // Propagate metadata
                                            );
                                            if context
                                                .output_sender
                                                .send("out", Packet::Audio(output_frame))
                                                .await
                                                .is_err()
                                            {
                                                tracing::debug!("Output channel closed, stopping node");
                                                break;
                                            }
                                            stats_tracker.sent();
                                        }
                                        stats_tracker.maybe_send();
                                    }
                                    Some((Err(e), _metadata)) => {
                                        packets_processed_counter.add(1, &[KeyValue::new("status", "error")]);
                                        stats_tracker.received();
                                        stats_tracker.errored();
                                        stats_tracker.maybe_send();
                                        tracing::warn!("Decode error for packet: {}",  e);
                                    }
                                    None => {
                                        // Result channel closed, all results processed
                                        break;
                                    }
                                }
                            }
                            Some(ctrl_msg) = context.control_rx.recv() => {
                                if matches!(ctrl_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                    tracing::info!("OpusDecoderNode received shutdown signal during drain");
                                    // Abort blocking decode task for immediate shutdown
                                    decode_task.abort();
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }

        // Abort the blocking task if not already aborted (for immediate shutdown)
        decode_task.abort();

        // Wait for the blocking task to complete with timeout (blocking I/O may not abort immediately)
        match tokio::time::timeout(std::time::Duration::from_millis(100), decode_task).await {
            Ok(Ok(())) => {
                // Task completed successfully
            },
            Ok(Err(e)) => {
                // Task panicked or was aborted
                if !e.is_cancelled() {
                    tracing::error!("Decode task panicked: {}", e);
                }
            },
            Err(_) => {
                // Timeout - blocking task is stuck in I/O, this is expected on abort
                tracing::debug!(
                    "Decode task did not respond to abort within 100ms (stuck in blocking I/O)"
                );
            },
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("OpusDecoderNode finished after {} audio packets", audio_packet_count);
        Ok(())
    }
}

// --- Opus Encoder ---

fn bitrate_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "minimum": 6000,
        "maximum": 510_000,  // 510 kbps max bitrate
        "multipleOf": 1000,
        "default": 64000,
        "tunable": false
    })
}

#[derive(Deserialize, Debug, JsonSchema)]
#[serde(default)]
pub struct OpusEncoderConfig {
    #[schemars(schema_with = "bitrate_schema")]
    pub bitrate: i32,
}

impl Default for OpusEncoderConfig {
    fn default() -> Self {
        Self {
            bitrate: 64000, // 64 kbps - good balance for voice
        }
    }
}

/// A node that encodes raw audio frames into Opus packets.
pub struct OpusEncoderNode {
    config: OpusEncoderConfig,
}

impl OpusEncoderNode {
    /// Creates a new Opus encoder node.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok`, but the signature allows for future error cases
    /// (e.g., if encoder initialization fails or config validation is added).
    pub const fn new(config: OpusEncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for OpusEncoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::RawAudio(AudioFormat {
                sample_rate: OPUS_SAMPLE_RATE,
                channels: 1,
                sample_format: SampleFormat::F32,
            })],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::OpusAudio,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        // Raw Opus frames over HTTP are conventionally labeled as audio/opus
        // When wrapped in Ogg, the container node (ogg::muxer) overrides to audio/ogg.
        Some("audio/opus".to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("OpusEncoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("opus_packets_processed").build();
        let encode_duration_histogram = meter.f64_histogram("opus_encode_duration").build();

        // Create channels for communication with the blocking task
        // Now includes channel count with each frame
        // Use Arc<PooledSamples> to avoid cloning samples unless padding is needed
        let (encode_tx, mut encode_rx) =
            mpsc::channel::<(Arc<PooledSamples>, u16)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<Vec<u8>, String>>(get_codec_channel_capacity());

        let target_bitrate = self.config.bitrate;

        // Spawn a single blocking task that will handle all encode operations
        // Uses blocking_recv/blocking_send for efficiency - no need for block_on
        let encode_task = tokio::task::spawn_blocking(move || {
            let mut encoder: Option<opus::Encoder> = None;
            let mut current_channels: Option<u16> = None;

            // Reusable encode buffer - avoids 4KB allocation per frame
            // Actual Opus output is typically 200-500 bytes, but we need the full buffer
            // for the encode call. We only allocate the actual encoded size for the result.
            let mut encode_buffer = vec![0u8; OPUS_OUTPUT_BUFFER_SIZE];

            // Use blocking_recv - efficient for spawn_blocking context
            while let Some((samples, channels)) = encode_rx.blocking_recv() {
                let encode_start_time = Instant::now();

                // Initialize or recreate encoder if channel count changed
                if current_channels != Some(channels) {
                    let opus_channels =
                        if channels == 1 { opus::Channels::Mono } else { opus::Channels::Stereo };

                    encoder = match opus::Encoder::new(
                        OPUS_SAMPLE_RATE,
                        opus_channels,
                        opus::Application::Audio,
                    ) {
                        Ok(mut e) => {
                            // Set the configured bitrate
                            if let Err(err) = e.set_bitrate(opus::Bitrate::Bits(target_bitrate)) {
                                tracing::error!("Failed to set Opus bitrate: {}", err);
                                let _ = result_tx.blocking_send(Err(err.to_string()));
                                return;
                            }
                            tracing::info!(
                                "Created Opus encoder for {} channels with bitrate {} bps",
                                channels,
                                target_bitrate
                            );
                            current_channels = Some(channels);
                            Some(e)
                        },
                        Err(e) => {
                            tracing::error!("Failed to create Opus encoder: {}", e);
                            let _ = result_tx.blocking_send(Err(e.to_string()));
                            return;
                        },
                    };
                }

                let result = {
                    // Encoder must exist at this point because we just initialized it above
                    // based on the channel count. If it doesn't exist, something went wrong.
                    let Some(ref mut enc) = encoder else {
                        tracing::error!("Encoder not initialized after channel setup");
                        let _ = result_tx.blocking_send(Err("Encoder not initialized".to_string()));
                        continue;
                    };

                    // Pad undersized frames with silence to meet Opus requirements
                    // Opus expects exact frame sizes (e.g., 960 samples for 20ms at 48kHz)
                    let expected_samples =
                        (OPUS_SAMPLE_RATE as usize * 20) / 1000 * channels as usize;

                    // Only clone if padding is needed, otherwise use slice directly
                    let encode_result = if samples.len() < expected_samples {
                        let mut padded = samples.as_ref().to_vec();
                        padded.resize(expected_samples, 0.0);
                        enc.encode_float(&padded, &mut encode_buffer)
                    } else {
                        // Use slice directly - no allocation needed
                        enc.encode_float(samples.as_ref(), &mut encode_buffer)
                    };

                    match encode_result {
                        Ok(len) => {
                            // Only allocate the actual encoded size (typically 200-500 bytes)
                            // instead of the full 4KB buffer
                            Ok(encode_buffer[..len].to_vec())
                        },
                        Err(e) => Err(e.to_string()),
                    }
                };

                encode_duration_histogram.record(encode_start_time.elapsed().as_secs_f64(), &[]);

                // Use blocking_send - efficient for spawn_blocking context
                if result_tx.blocking_send(result).is_err() {
                    break; // Main task has shut down
                }
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Process input packets and send them for encoding
        let encode_tx_clone = encode_tx.clone();
        let batch_size = context.batch_size;
        let mut input_task = tokio::spawn(async move {
            let mut frame_count = 0;
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Audio(frame) = packet {
                        frame_count += 1;

                        // Send to blocking task for encoding with channel count
                        if encode_tx_clone.send((frame.samples, frame.channels)).await.is_err() {
                            tracing::error!("Encode task has shut down unexpectedly");
                            return;
                        }
                    }
                }
            }
            tracing::info!("OpusEncoderNode input stream closed after {} frames", frame_count);
        });

        // Process results from the blocking task
        loop {
            tokio::select! {
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some(Ok(encoded_data)) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                            stats_tracker.received();

                            // Calculate packet duration: Opus typically uses 20ms frames at 48kHz
                            // (960 samples per frame). Set duration for pacing nodes downstream.
                            let duration_us = 20_000u64; // 20ms = 20,000 microseconds

                            let output_packet = Packet::Binary {
                                data: Bytes::from(encoded_data),
                                content_type: None, // Opus packets don't have a content-type
                                metadata: Some(streamkit_core::types::PacketMetadata {
                                    timestamp_us: None, // No absolute timestamp
                                    duration_us: Some(duration_us),
                                    sequence: None, // No sequence tracking yet
                                }),
                            };
                            if context
                                .output_sender
                                .send("out", output_packet)
                                .await
                                .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                break;
                            }
                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                        }
                        Some(Err(e)) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "error")]);
                            stats_tracker.received();
                            stats_tracker.errored();
                            stats_tracker.maybe_send();
                            tracing::error!("Encode error: {}", e);
                            // Don't fail the entire pipeline for encode errors (e.g., last frame with invalid size)
                            // Just skip the frame and continue
                        }
                        None => {
                            // Result channel closed, blocking task is done
                            break;
                        }
                    }
                }
                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("OpusEncoderNode received shutdown signal");
                        // Abort input task
                        input_task.abort();
                        // Signal blocking task to shut down
                        drop(encode_tx);
                        // Break out of main loop
                        break;
                    }
                    // Ignore other control messages
                }
                _ = &mut input_task => {
                    // Input task finished, signal blocking task to shut down
                    drop(encode_tx);

                    // Continue processing any remaining results
                    while let Some(maybe_result) = result_rx.recv().await {
                        match maybe_result {
                            Ok(encoded_data) => {
                                packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                                stats_tracker.received();

                                let output_packet = Packet::Binary {
                                    data: Bytes::from(encoded_data),
                                    content_type: None, // Opus packets don't have a content-type
                                    metadata: None,
                                };
                                if context
                                    .output_sender
                                    .send("out", output_packet)
                                    .await
                                    .is_err()
                                {
                                    tracing::debug!("Output channel closed, stopping node");
                                    break;
                                }
                                stats_tracker.sent();
                                stats_tracker.maybe_send();
                            }
                            Err(e) => {
                                packets_processed_counter.add(1, &[KeyValue::new("status", "error")]);
                                stats_tracker.received();
                                stats_tracker.errored();
                                stats_tracker.maybe_send();
                                tracing::error!("Encode error: {}", e);
                                // Don't fail the entire pipeline for encode errors (e.g., last frame with invalid size)
                                // Just skip the frame and continue processing remaining results
                            }
                        }
                    }
                    break;
                }
            }
        }

        // Wait for the blocking task to complete
        let _ = encode_task.await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("OpusEncoderNode finished");
        Ok(())
    }
}

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the Opus codec nodes.
///
/// # Panics
///
/// Panics if default Opus encoder/decoder cannot be created (should never happen)
/// or if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization and default configs should never fail
pub fn register_opus_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "opus")]
    {
        let default_decoder = OpusDecoderNode::new(OpusDecoderConfig::default())
            .expect("default Opus decoder config should be valid");
        registry.register_static_with_description(
            "audio::opus::decoder",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(OpusDecoderNode::new(config)?))
            },
            serde_json::to_value(schema_for!(OpusDecoderConfig))
                .expect("OpusDecoderConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_decoder.input_pins(),
                outputs: default_decoder.output_pins(),
            },
            vec!["audio".to_string(), "codecs".to_string(), "opus".to_string()],
            false,
            "Decodes Opus-compressed audio packets into raw PCM samples. \
             Opus is the preferred codec for real-time audio due to its low latency \
             and excellent quality across all bitrates.",
        );

        let default_encoder = OpusEncoderNode::new(OpusEncoderConfig::default())
            .expect("default Opus encoder config should be valid");
        registry.register_static_with_description(
            "audio::opus::encoder",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(OpusEncoderNode::new(config)?))
            },
            serde_json::to_value(schema_for!(OpusEncoderConfig))
                .expect("OpusEncoderConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_encoder.input_pins(),
                outputs: default_encoder.output_pins(),
            },
            vec!["audio".to_string(), "codecs".to_string(), "opus".to_string()],
            false,
            "Encodes raw PCM audio into Opus-compressed packets. \
             Configurable bitrate, application mode (VoIP/audio), and complexity settings. \
             Ideal for streaming and real-time communication.",
        );
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped,
        create_test_audio_packet, create_test_binary_packet, create_test_context,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_opus_encoder_mono() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create Opus encoder (default is 64 kbps)
        let config = OpusEncoderConfig::default();
        let node = OpusEncoderNode::new(config).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send audio frames (20ms at 48kHz mono = 960 samples)
        for _ in 0..10 {
            let packet = create_test_audio_packet(48000, 1, 960, 0.5);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify output - should have Opus-encoded packets
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 10, "Should have 10 encoded Opus packets");

        // Verify they're Binary packets
        for packet in &output_packets {
            match packet {
                Packet::Binary { data, .. } => {
                    assert!(!data.is_empty(), "Opus packet should have data");
                    // Opus packets are typically much smaller than raw audio
                    assert!(data.len() < 1000, "Opus packet should be compressed");
                },
                _ => panic!("Expected Binary packet from Opus encoder"),
            }
        }

        println!("✅ Opus encoder (mono) produced {} packets", output_packets.len());
    }

    #[tokio::test]
    async fn test_opus_encoder_stereo() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create Opus encoder with higher bitrate for stereo
        let config = OpusEncoderConfig { bitrate: 128_000 };
        let node = OpusEncoderNode::new(config).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send stereo audio frames (20ms at 48kHz stereo = 960 samples per channel * 2 = 1920 total)
        for _ in 0..5 {
            let packet = create_test_audio_packet(48000, 2, 960, 0.3);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 5, "Should have 5 encoded Opus packets");

        println!("✅ Opus encoder (stereo, 128kbps) produced {} packets", output_packets.len());
    }

    #[tokio::test]
    async fn test_opus_decoder_mono() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create Opus decoder
        let config = OpusDecoderConfig::default();
        let node = OpusDecoderNode::new(config).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // First, encode some audio to get valid Opus packets
        let mut encoder =
            opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Audio).unwrap();
        encoder.set_bitrate(opus::Bitrate::Bits(64000)).unwrap();

        // Encode a few frames
        let mut opus_out = vec![0u8; 4000];
        for _ in 0..5 {
            let audio_samples = vec![0.5f32; 960]; // 20ms mono at 48kHz
            let len = encoder.encode_float(&audio_samples, &mut opus_out).unwrap();
            let opus_packet = opus_out[..len].to_vec();
            let packet = create_test_binary_packet(opus_packet);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify output - should have decoded audio frames
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 5, "Should have 5 decoded audio frames");

        // Verify they're Audio packets with correct format
        for packet in &output_packets {
            match packet {
                Packet::Audio(frame) => {
                    assert_eq!(frame.sample_rate, 48000);
                    assert_eq!(frame.channels, 1);
                    assert_eq!(
                        frame.samples.len(),
                        960,
                        "Should have 960 samples (20ms at 48kHz mono)"
                    );
                },
                _ => panic!("Expected Audio packet from Opus decoder"),
            }
        }

        println!("✅ Opus decoder (mono) decoded {} frames", output_packets.len());
    }

    #[tokio::test]
    async fn test_opus_roundtrip() {
        // Test encoding then decoding
        // Step 1: Encode audio to Opus
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_mock_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);

        let enc_config = OpusEncoderConfig { bitrate: 96000 };
        let enc_node = OpusEncoderNode::new(enc_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(enc_node).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        // Send original audio
        let original_packets = vec![
            create_test_audio_packet(48000, 2, 960, 0.1),
            create_test_audio_packet(48000, 2, 960, 0.2),
            create_test_audio_packet(48000, 2, 960, 0.3),
            create_test_audio_packet(48000, 2, 960, 0.4),
            create_test_audio_packet(48000, 2, 960, 0.5),
        ];

        for packet in &original_packets {
            enc_input_tx.send(packet.clone()).await.unwrap();
        }

        drop(enc_input_tx);
        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_mock_sender.get_packets_for_pin("out").await;
        assert_eq!(encoded_packets.len(), 5, "Should have 5 encoded packets");

        println!("✅ Encoded {} audio frames to Opus", encoded_packets.len());

        // Step 2: Decode Opus back to audio
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_mock_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);

        let dec_config = OpusDecoderConfig::default();
        let dec_node = OpusDecoderNode::new(dec_config).unwrap();

        let dec_handle = tokio::spawn(async move { Box::new(dec_node).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        // Send encoded packets to decoder
        for packet in encoded_packets {
            dec_input_tx.send(packet).await.unwrap();
        }

        drop(dec_input_tx);
        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_mock_sender.get_packets_for_pin("out").await;
        assert_eq!(decoded_packets.len(), 5, "Should have 5 decoded frames");

        println!("✅ Decoded {} Opus packets back to audio", decoded_packets.len());

        // Verify decoded audio has correct format
        // Note: Current decoder implementation decodes to mono, even if input was stereo
        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Audio(frame) => {
                    assert_eq!(frame.sample_rate, 48_000, "Frame {i} should have 48kHz");
                    // Decoder is mono-only currently
                    assert_eq!(frame.channels, 1, "Frame {i} should be mono (decoder limitation)");
                    assert_eq!(
                        frame.samples.len(),
                        960,
                        "Frame {i} should have 960 samples (mono)"
                    );
                },
                _ => panic!("Expected Audio packet at index {i}"),
            }
        }

        println!("✅ Opus roundtrip complete: audio → Opus → audio");
    }

    #[tokio::test]
    async fn test_opus_encoder_channel_switching() {
        // Test that encoder handles mono → stereo transitions
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = OpusEncoderConfig::default();
        let node = OpusEncoderNode::new(config).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send mono frames
        for _ in 0..3 {
            let packet = create_test_audio_packet(48000, 1, 960, 0.5);
            input_tx.send(packet).await.unwrap();
        }

        // Switch to stereo
        for _ in 0..3 {
            let packet = create_test_audio_packet(48000, 2, 960, 0.5);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 6, "Should handle 6 frames across channel change");

        println!("✅ Opus encoder handled mono → stereo channel switching");
    }

    #[tokio::test]
    async fn test_opus_encoder_undersized_frame() {
        // Test that encoder handles frames smaller than 20ms
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let config = OpusEncoderConfig::default();
        let node = OpusEncoderNode::new(config).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send normal frame
        input_tx.send(create_test_audio_packet(48000, 1, 960, 0.5)).await.unwrap();

        // Send undersized frame (will be padded with silence)
        input_tx.send(create_test_audio_packet(48000, 1, 480, 0.3)).await.unwrap();

        // Send another normal frame
        input_tx.send(create_test_audio_packet(48000, 1, 960, 0.5)).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 3, "Should handle all frames including undersized one");

        println!("✅ Opus encoder handled undersized frame with padding");
    }

    #[tokio::test]
    async fn test_opus_encoder_bitrate_modes() {
        // Test different bitrate settings
        for (bitrate, label) in
            [(32_000, "32kbps"), (64_000, "64kbps"), (128_000, "128kbps"), (256_000, "256kbps")]
        {
            let (input_tx, input_rx) = mpsc::channel(10);
            let mut inputs = HashMap::new();
            inputs.insert("in".to_string(), input_rx);

            let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

            let config = OpusEncoderConfig { bitrate };
            let node = OpusEncoderNode::new(config).unwrap();

            let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

            assert_state_initializing(&mut state_rx).await;
            assert_state_running(&mut state_rx).await;

            // Send a few frames
            for _ in 0..3 {
                input_tx.send(create_test_audio_packet(48000, 2, 960, 0.5)).await.unwrap();
            }

            drop(input_tx);
            assert_state_stopped(&mut state_rx).await;
            node_handle.await.unwrap().unwrap();

            let output_packets = mock_sender.get_packets_for_pin("out").await;
            assert_eq!(output_packets.len(), 3, "Should encode 3 frames at {label}");
        }

        println!("✅ Opus encoder tested with multiple bitrates");
    }
}
