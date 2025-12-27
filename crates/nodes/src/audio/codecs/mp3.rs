// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::VecDeque;
use std::io::Cursor;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{AudioFormat, AudioFrame, Packet, PacketType, SampleFormat};
use streamkit_core::{
    get_stream_channel_capacity, state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::sync::mpsc;

// Use the shared StreamingReader from streaming_utils
use crate::streaming_utils::StreamingReader;

// --- MP3 Decoder Constants ---

/// Channel buffer size for decoder pipeline communication
const DECODER_CHANNEL_CAPACITY: usize = 32;

/// Output frame size - 20ms at 48kHz stereo (960 samples per channel * 2 = 1920 total)
/// This matches Opus encoder expectations
const OUTPUT_FRAME_SIZE: usize = 1920;

// --- MP3 Decoder ---

#[derive(Deserialize, Debug, Default, JsonSchema)]
#[serde(default)]
pub struct Mp3DecoderConfig {}

/// A node that decodes MP3 audio files to raw PCM audio frames.
pub struct Mp3DecoderNode {
    _config: Mp3DecoderConfig,
}

impl Mp3DecoderNode {
    /// Creates a new MP3 decoder node.
    ///
    /// # Errors
    /// Currently returns `Ok` in all cases, but the `Result` type is kept for future extensibility.
    pub const fn new(config: Mp3DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { _config: config })
    }
}

#[async_trait]
impl ProcessorNode for Mp3DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawAudio(AudioFormat {
                sample_rate: 48000, // Will be updated based on actual format
                channels: 2,        // Will be updated based on actual format
                sample_format: SampleFormat::F32,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some("audio/mpeg".to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("Mp3DecoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("mp3_packets_processed").build();
        let decode_duration_histogram = meter.f64_histogram("mp3_decode_duration").build();

        // Create channels for communication with the blocking task.
        // This must be bounded to provide backpressure and prevent unbounded buffering.
        let (stream_tx, stream_rx) = mpsc::channel::<Bytes>(get_stream_channel_capacity());
        let (result_tx, mut result_rx) = mpsc::channel::<DecodeResult>(DECODER_CHANNEL_CAPACITY);

        // Spawn blocking task that will decode as data streams in
        let decode_duration_histogram_clone = decode_duration_histogram.clone();
        let decode_task = tokio::task::spawn_blocking(move || {
            let decode_start_time = Instant::now();

            // Create streaming reader that will block waiting for data from the channel
            let reader = StreamingReader::new(stream_rx);

            let result = decode_mp3_streaming_incremental(reader, &result_tx);

            decode_duration_histogram_clone.record(decode_start_time.elapsed().as_secs_f64(), &[]);

            if let Err(e) = result {
                tracing::error!("MP3 decode failed: {}", e);
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Stream input data to decoder as it arrives.
        // This is a separate task so the main loop can keep draining decode results even when
        // the stream channel is full (avoids deadlocks).
        let mut input_task = tokio::spawn(async move {
            let stream_tx = stream_tx;
            while let Some(packet) = input_rx.recv().await {
                if let Packet::Binary { data, .. } = packet {
                    tracing::debug!("Streaming {} bytes to MP3 decoder", data.len());
                    if stream_tx.send(data).await.is_err() {
                        break;
                    }
                }
            }
        });
        let mut input_done = false;

        // Process input and results concurrently
        loop {
            tokio::select! {
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some(Ok((samples, sample_rate, channels, metadata))) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                            stats_tracker.received();

                            // Send the decoded frame directly - already appropriately sized by MP3 packet
                            if !samples.is_empty() {
                                let output_frame = AudioFrame::with_metadata(
                                    sample_rate,
                                    channels,
                                    samples,
                                    Some(metadata),
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
                        Some(Err(e)) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "error")]);
                            stats_tracker.received();
                            stats_tracker.errored();
                            stats_tracker.maybe_send();
                            let err_msg = format!("MP3 decode error: {e}");
                            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                            return Err(StreamKitError::Runtime(err_msg));
                        }
                        None => {
                            // Result channel closed, blocking task is done
                            break;
                        }
                    }
                }
                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("Mp3DecoderNode received shutdown signal");
                        input_task.abort();
                        break;
                    }
                }
                _ = &mut input_task, if !input_done => {
                    // Input finished (EOF or upstream closed). Keep draining decode results until
                    // the blocking task closes the result channel.
                    input_done = true;
                }
            }
        }

        // Drop the result receiver to signal the decode task to stop
        drop(result_rx);

        // Abort the blocking task for immediate shutdown
        decode_task.abort();

        // Wait for decode task to complete with timeout (blocking I/O may not abort immediately)
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

        tracing::info!("Mp3DecoderNode finished");
        Ok(())
    }
}

// Type alias for decode result to simplify complex signatures
// Includes samples, sample_rate, channels, and timing metadata
type DecodeResult = Result<(Vec<f32>, u32, u16, streamkit_core::types::PacketMetadata), String>;

/// Decodes MP3 data incrementally from a streaming reader
/// Decodes and emits frames as soon as MP3 packets are available
#[allow(clippy::cognitive_complexity)] // Decoder state machine is inherently complex
fn decode_mp3_streaming_incremental(
    reader: StreamingReader,
    result_tx: &mpsc::Sender<DecodeResult>,
) -> Result<(), String> {
    // Wrap the streaming reader in ReadOnlySource, then MediaSourceStream
    let source = ReadOnlySource::new(reader);
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

    // Create a hint for MP3 format
    let mut hint = Hint::new();
    hint.with_extension("mp3");

    // Probe the media source
    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Failed to probe MP3 format: {e}"))?;

    let mut format_reader = probed.format;

    // Get the default track
    let track =
        format_reader.default_track().ok_or_else(|| "No default track found in MP3".to_string())?;

    // Get codec parameters
    let codec_params = &track.codec_params;
    let sample_rate =
        codec_params.sample_rate.ok_or_else(|| "No sample rate found in MP3".to_string())?;
    let channel_count =
        codec_params.channels.ok_or_else(|| "No channel info found in MP3".to_string())?.count();
    let channels = u16::try_from(channel_count)
        .map_err(|_| format!("Channel count {channel_count} exceeds u16::MAX"))?;

    tracing::info!(
        "Detected MP3 audio: {} Hz, {} channels (streaming mode)",
        sample_rate,
        channels
    );

    // Create decoder
    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &decoder_opts)
        .map_err(|e| format!("Failed to create MP3 decoder: {e}"))?;

    // Get the track ID for filtering
    let track_id = track.id;

    // Decode packets and rechunk for output
    // Use VecDeque for O(1) front removal instead of O(n) Vec::drain
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut rechunk_buffer: VecDeque<f32> = VecDeque::new();
    let mut frame_count = 0u64;
    let mut cumulative_timestamp_us = 0u64;

    loop {
        // Read next packet - this will block waiting for more data from the stream
        let packet = match format_reader.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::debug!("Reached end of MP3 stream after {} frames", frame_count);
                break;
            },
            Err(e) => {
                tracing::warn!("Error reading MP3 packet: {}", e);
                break;
            },
        };

        // Filter packets by track ID
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                // Initialize sample buffer on first decode
                if sample_buf.is_none() {
                    let spec = *audio_buf.spec();
                    let duration = audio_buf.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                // Copy decoded audio and rechunk for output
                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    rechunk_buffer.extend(buf.samples().iter().copied());

                    // Send fixed-size chunks as they become available
                    while rechunk_buffer.len() >= OUTPUT_FRAME_SIZE {
                        // Drain from front - O(1) amortized with VecDeque
                        let chunk: Vec<f32> = rechunk_buffer.drain(..OUTPUT_FRAME_SIZE).collect();

                        // Calculate duration for this chunk
                        // duration = (total_samples / channels) / sample_rate * 1_000_000
                        let samples_per_channel = chunk.len() / channels as usize;
                        let duration_us =
                            (samples_per_channel as u64 * 1_000_000) / u64::from(sample_rate);

                        // Create metadata with timing information
                        let metadata = streamkit_core::types::PacketMetadata {
                            timestamp_us: Some(cumulative_timestamp_us),
                            duration_us: Some(duration_us),
                            sequence: Some(frame_count),
                        };

                        // Use blocking_send - more efficient than Handle::block_on
                        if result_tx
                            .blocking_send(Ok((chunk, sample_rate, channels, metadata)))
                            .is_err()
                        {
                            tracing::debug!("Result channel closed, stopping decode");
                            return Ok(());
                        }

                        frame_count += 1;
                        cumulative_timestamp_us += duration_us;
                    }
                }
            },
            Err(Error::DecodeError(err)) => {
                // Log and continue to next packet - no explicit continue needed
                tracing::warn!("MP3 decode error (continuing): {}", err);
            },
            Err(e) => {
                return Err(format!("Failed to decode MP3 packet: {e}"));
            },
        }
    }

    // Send any remaining samples as a final chunk
    if !rechunk_buffer.is_empty() {
        // Calculate duration for the final chunk
        let samples_per_channel = rechunk_buffer.len() / channels as usize;
        let duration_us = (samples_per_channel as u64 * 1_000_000) / u64::from(sample_rate);

        let metadata = streamkit_core::types::PacketMetadata {
            timestamp_us: Some(cumulative_timestamp_us),
            duration_us: Some(duration_us),
            sequence: Some(frame_count),
        };

        let final_chunk: Vec<f32> = rechunk_buffer.into_iter().collect();
        if result_tx.blocking_send(Ok((final_chunk, sample_rate, channels, metadata))).is_ok() {
            frame_count += 1;
        }
    }

    tracing::info!("Finished streaming {} MP3 frames", frame_count);

    Ok(())
}

/// Legacy decode function (kept for reference, not used)
#[allow(dead_code)]
#[allow(clippy::cognitive_complexity)] // Decoder state machine is inherently complex
fn decode_mp3_streaming(data: &[u8], result_tx: &mpsc::Sender<DecodeResult>) -> Result<(), String> {
    use std::sync::mpsc as std_mpsc;

    // Create a cursor over the data
    let owned_data = data.to_vec();
    let cursor = Cursor::new(owned_data);
    let mss = MediaSourceStream::new(Box::new(cursor), MediaSourceStreamOptions::default());

    // Create a hint for MP3 format
    let mut hint = Hint::new();
    hint.with_extension("mp3");

    // Probe the media source
    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Failed to probe MP3 format: {e}"))?;

    let mut format_reader = probed.format;

    // Get the default track
    let track =
        format_reader.default_track().ok_or_else(|| "No default track found in MP3".to_string())?;

    // Get codec parameters
    let codec_params = &track.codec_params;
    let sample_rate =
        codec_params.sample_rate.ok_or_else(|| "No sample rate found in MP3".to_string())?;
    let channel_count =
        codec_params.channels.ok_or_else(|| "No channel info found in MP3".to_string())?.count();
    let channels = u16::try_from(channel_count)
        .map_err(|_| format!("Channel count {channel_count} exceeds u16::MAX"))?;

    tracing::info!("Detected MP3 audio: {} Hz, {} channels", sample_rate, channels);

    // Create decoder
    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &decoder_opts)
        .map_err(|e| format!("Failed to create MP3 decoder: {e}"))?;

    // Get the track ID for filtering
    let track_id = track.id;

    // Use a std channel to collect frames in the blocking context
    let (frame_tx, frame_rx) = std_mpsc::channel();

    // Decode packets and collect frames
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut packet_count = 0u64;
    let mut cumulative_timestamp_us = 0u64;

    // Buffer for rechunking - use VecDeque for O(1) front removal
    let mut rechunk_buffer: VecDeque<f32> = VecDeque::new();

    loop {
        // Read next packet
        let packet = match format_reader.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::debug!("Reached end of MP3 stream after {} packets", packet_count);
                break;
            },
            Err(e) => {
                tracing::warn!("Error reading MP3 packet: {}", e);
                break;
            },
        };

        // Filter packets by track ID
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                // Initialize sample buffer on first decode
                if sample_buf.is_none() {
                    let spec = *audio_buf.spec();
                    let duration = audio_buf.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                // Copy decoded audio into sample buffer and add to rechunk buffer
                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    rechunk_buffer.extend(buf.samples().iter().copied());

                    // Send fixed-size chunks as they become available
                    while rechunk_buffer.len() >= OUTPUT_FRAME_SIZE {
                        // Drain from front - O(1) amortized with VecDeque
                        let chunk: Vec<f32> = rechunk_buffer.drain(..OUTPUT_FRAME_SIZE).collect();

                        // Calculate duration for this chunk
                        let samples_per_channel = chunk.len() / channels as usize;
                        let duration_us =
                            (samples_per_channel as u64 * 1_000_000) / u64::from(sample_rate);

                        let metadata = streamkit_core::types::PacketMetadata {
                            timestamp_us: Some(cumulative_timestamp_us),
                            duration_us: Some(duration_us),
                            sequence: Some(packet_count),
                        };

                        if frame_tx.send((chunk, sample_rate, channels, metadata)).is_err() {
                            return Ok(());
                        }
                        packet_count += 1;
                        cumulative_timestamp_us += duration_us;
                    }
                }
            },
            Err(Error::DecodeError(err)) => {
                // Log and continue to next packet - no explicit continue needed
                tracing::warn!("MP3 decode error (continuing): {}", err);
            },
            Err(e) => {
                return Err(format!("Failed to decode MP3 packet: {e}"));
            },
        }
    }

    // Send any remaining samples as a final chunk
    if !rechunk_buffer.is_empty() {
        // Calculate duration for the final chunk
        let samples_per_channel = rechunk_buffer.len() / channels as usize;
        let duration_us = (samples_per_channel as u64 * 1_000_000) / u64::from(sample_rate);

        let metadata = streamkit_core::types::PacketMetadata {
            timestamp_us: Some(cumulative_timestamp_us),
            duration_us: Some(duration_us),
            sequence: Some(packet_count),
        };

        let final_chunk: Vec<f32> = rechunk_buffer.into_iter().collect();
        if frame_tx.send((final_chunk, sample_rate, channels, metadata)).is_ok() {
            packet_count += 1;
        }
    }

    drop(frame_tx); // Signal we're done

    tracing::info!("Decoded {} MP3 packets, sending to async channel", packet_count);

    // Now send all frames to the async channel
    // Use blocking_send - efficient for spawn_blocking context
    for frame in frame_rx {
        if result_tx.blocking_send(Ok(frame)).is_err() {
            tracing::debug!("Result channel closed during frame transmission");
            return Ok(());
        }
    }

    tracing::info!("Finished streaming {} MP3 frames", packet_count);

    Ok(())
}

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the MP3 decoder node.
///
/// # Panics
///
/// Panics if the default MP3 decoder cannot be created (should never happen)
/// or if the config schema cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization and default config should never fail
pub fn register_mp3_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "symphonia")]
    {
        let default_decoder = Mp3DecoderNode::new(Mp3DecoderConfig::default())
            .expect("default MP3 decoder config should be valid");
        registry.register_static_with_description(
            "audio::mp3::decoder",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(Mp3DecoderNode::new(config)?))
            },
            serde_json::to_value(schema_for!(Mp3DecoderConfig))
                .expect("Mp3DecoderConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_decoder.input_pins(),
                outputs: default_decoder.output_pins(),
            },
            vec!["audio".to_string(), "codecs".to_string(), "mp3".to_string()],
            false,
            "Decodes MP3 audio data to raw PCM samples. \
             Accepts binary MP3 data and outputs 48kHz stereo f32 audio.",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::uninlined_format_args)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped,
        create_test_binary_packet, create_test_context, extract_audio_data,
    };
    use std::collections::HashMap;
    use std::path::Path;
    use tokio::sync::mpsc;

    // Helper to read test audio files
    fn read_sample_file(filename: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/audio").join(filename);
        std::fs::read(&path)
            .unwrap_or_else(|_| panic!("Failed to read test file: {}", path.display()))
    }

    #[tokio::test]
    async fn test_mp3_decode() {
        // Create input channel
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create MP3 decoder node
        let node = Mp3DecoderNode::new(Mp3DecoderConfig::default()).unwrap();

        // Spawn node task
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        // Wait for initializing and running states
        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Read and send MP3 test file
        let mp3_data = read_sample_file("sample.mp3");
        let packet = create_test_binary_packet(mp3_data);
        input_tx.send(packet).await.unwrap();

        // Close input to signal completion
        drop(input_tx);

        // Wait for stopped state
        assert_state_stopped(&mut state_rx).await;

        // Wait for node to finish
        node_handle.await.unwrap().unwrap();

        // Verify output packets
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty(), "Expected at least one output packet");

        // Verify the first packet contains audio data
        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio packet");
        assert!(!audio_data.is_empty(), "Expected non-empty audio data from MP3 decoder");

        // Verify audio format
        if let Packet::Audio(frame) = &output_packets[0] {
            assert!(frame.sample_rate > 0, "Sample rate should be greater than 0");
            assert!(frame.channels > 0, "Channels should be greater than 0");
            tracing::info!(
                "Decoded MP3: {} Hz, {} channels, {} samples",
                frame.sample_rate,
                frame.channels,
                frame.samples.len()
            );
        }
    }

    #[tokio::test]
    async fn test_mp3_multiple_packets() {
        // Test that decoder can handle data split across multiple packets
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = Mp3DecoderNode::new(Mp3DecoderConfig::default()).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Read MP3 file and split into multiple packets
        let mp3_data = read_sample_file("sample.mp3");
        let chunk_size = mp3_data.len() / 3;

        for i in 0..3 {
            let start = i * chunk_size;
            let end = if i == 2 { mp3_data.len() } else { (i + 1) * chunk_size };
            let chunk = mp3_data[start..end].to_vec();
            let packet = create_test_binary_packet(chunk);
            input_tx.send(packet).await.unwrap();
        }

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify we got output
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty(), "Expected output even when input split across packets");
    }
}
