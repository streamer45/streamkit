// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::VecDeque;
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

// --- WAV Demuxer Constants ---

/// Channel buffer size for demuxer pipeline communication
const DEMUXER_CHANNEL_CAPACITY: usize = 32;

/// Output frame size - 20ms at 48kHz stereo (960 samples per channel * 2 = 1920 total)
/// This matches Opus encoder expectations
const OUTPUT_FRAME_SIZE: usize = 1920;

// --- WAV Demuxer ---

use crate::streaming_utils::StreamingReader;

#[derive(Deserialize, Debug, Default, JsonSchema)]
#[serde(default)]
pub struct WavDemuxerConfig {}

/// A node that demuxes WAV container files to raw PCM audio frames.
pub struct WavDemuxerNode {
    _config: WavDemuxerConfig,
}

impl WavDemuxerNode {
    /// Creates a new WAV demuxer node.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok`, but the signature allows for future error cases
    /// (e.g., if config validation is added).
    pub const fn new(config: WavDemuxerConfig) -> Result<Self, StreamKitError> {
        Ok(Self { _config: config })
    }
}

#[async_trait]
impl ProcessorNode for WavDemuxerNode {
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
        Some("audio/wav".to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("WavDemuxerNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("wav_packets_processed").build();
        let demux_duration_histogram = meter.f64_histogram("wav_demux_duration").build();

        // Create channels for communication with the blocking task.
        // This must be bounded to provide backpressure and prevent unbounded buffering.
        let (stream_tx, stream_rx) = mpsc::channel::<Bytes>(get_stream_channel_capacity());
        let (result_tx, mut result_rx) = mpsc::channel::<DemuxResult>(DEMUXER_CHANNEL_CAPACITY);

        // Spawn blocking task that will demux as data streams in
        let demux_duration_histogram_clone = demux_duration_histogram.clone();
        let demux_task = tokio::task::spawn_blocking(move || {
            let demux_start_time = Instant::now();

            // Create streaming reader that will block waiting for data from the channel
            let reader = StreamingReader::new(stream_rx);

            let result = demux_wav_streaming_incremental(reader, &result_tx);

            demux_duration_histogram_clone.record(demux_start_time.elapsed().as_secs_f64(), &[]);

            if let Err(e) = result {
                tracing::error!("WAV demux failed: {}", e);
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Stream input data to demuxer as it arrives.
        // This is a separate task so the main loop can keep draining demux results even when
        // the stream channel is full (avoids deadlocks).
        let node_name_for_input = node_name.clone();
        let mut input_task = tokio::spawn(async move {
            let stream_tx = stream_tx;
            let mut total_input_bytes = 0usize;
            let mut input_chunk_count = 0u64;

            while let Some(packet) = input_rx.recv().await {
                if let Packet::Binary { data, .. } = packet {
                    input_chunk_count += 1;
                    total_input_bytes += data.len();
                    tracing::debug!(
                        "Streaming chunk {} with {} bytes to WAV demuxer (total: {} bytes)",
                        input_chunk_count,
                        data.len(),
                        total_input_bytes
                    );
                    if stream_tx.send(data).await.is_err() {
                        break;
                    }
                }
            }

            tracing::info!(
                "{} input stream closed after {} chunks ({} total bytes)",
                node_name_for_input,
                input_chunk_count,
                total_input_bytes
            );
        });
        let mut input_done = false;

        // Process input and results concurrently
        loop {
            tokio::select! {
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some(Ok((samples, sample_rate, channels))) => {
                            packets_processed_counter.add(1, &[KeyValue::new("status", "ok")]);
                            stats_tracker.received();

                            if !samples.is_empty() {
                                let output_frame = AudioFrame::new(
                                    sample_rate,
                                    channels,
                                    samples
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
                            let err_msg = format!("WAV demux error: {e}");
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
                        tracing::info!("WavDemuxerNode received shutdown signal");
                        input_task.abort();
                        break;
                    }
                }
                _ = &mut input_task, if !input_done => {
                    // Input finished (EOF or upstream closed). Keep draining demux results until
                    // the blocking task closes the result channel.
                    input_done = true;
                }
            }
        }

        // Wait for the blocking task to complete
        let _ = demux_task.await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("WavDemuxerNode finished");
        Ok(())
    }
}

// Type alias for demux result to simplify complex signatures
type DemuxResult = Result<(Vec<f32>, u32, u16), String>;

/// Demuxes WAV data incrementally from a streaming reader
/// Demuxes and emits frames as soon as WAV packets are available
/// High complexity due to WAV format handling, frame assembly, and channel conversion
#[allow(clippy::cognitive_complexity)]
fn demux_wav_streaming_incremental(
    reader: StreamingReader,
    result_tx: &mpsc::Sender<DemuxResult>,
) -> Result<(), String> {
    // Wrap the streaming reader in ReadOnlySource, then MediaSourceStream
    let source = ReadOnlySource::new(reader);
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

    // Create a hint for WAV format
    let mut hint = Hint::new();
    hint.with_extension("wav");

    // Probe the media source
    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Failed to probe WAV format: {e}"))?;

    let mut format_reader = probed.format;

    // Get the default track
    let track =
        format_reader.default_track().ok_or_else(|| "No default track found in WAV".to_string())?;

    // Get codec parameters
    let codec_params = &track.codec_params;
    let sample_rate =
        codec_params.sample_rate.ok_or_else(|| "No sample rate found in WAV".to_string())?;
    let channel_count =
        codec_params.channels.ok_or_else(|| "No channel info found in WAV".to_string())?.count();
    let channels = u16::try_from(channel_count)
        .map_err(|_| format!("Channel count {channel_count} exceeds u16::MAX"))?;

    tracing::info!(
        "Detected WAV audio: {} Hz, {} channels (streaming mode)",
        sample_rate,
        channels
    );

    // Create decoder (WAV typically contains PCM, which needs decoding)
    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &decoder_opts)
        .map_err(|e| format!("Failed to create WAV decoder: {e}"))?;

    // Get the track ID for filtering
    let track_id = track.id;

    // Decode packets and rechunk for output
    // Use VecDeque for O(1) front removal instead of O(n) Vec::drain
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut rechunk_buffer: VecDeque<f32> = VecDeque::new();
    let mut frame_count = 0;

    loop {
        // Read next packet - this will block waiting for more data from the stream
        let packet = match format_reader.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::debug!("Reached end of WAV stream after {} frames", frame_count);
                break;
            },
            Err(e) => {
                tracing::warn!("Error reading WAV packet: {}", e);
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

                        // Use blocking_send - more efficient than Handle::block_on
                        if result_tx.blocking_send(Ok((chunk, sample_rate, channels))).is_err() {
                            tracing::info!(
                                "Result channel closed after sending {} frames ({} samples total). Stopping demux.",
                                frame_count,
                                frame_count * OUTPUT_FRAME_SIZE
                            );
                            return Ok(());
                        }

                        frame_count += 1;
                        if frame_count % 100 == 0 {
                            tracing::debug!(
                                "Sent {} WAV frames so far ({} samples)",
                                frame_count,
                                frame_count * OUTPUT_FRAME_SIZE
                            );
                        }
                    }
                }
            },
            Err(Error::DecodeError(err)) => {
                // Log and continue to next packet - no explicit continue needed
                tracing::warn!("WAV decode error (continuing): {}", err);
            },
            Err(e) => {
                return Err(format!("Failed to decode WAV packet: {e}"));
            },
        }
    }

    // Send any remaining samples as a final frame
    let total_samples_sent = frame_count * OUTPUT_FRAME_SIZE;
    if rechunk_buffer.is_empty() {
        tracing::info!(
            "WAV streaming demux complete: {} frames sent, {} total samples",
            frame_count,
            total_samples_sent
        );
    } else {
        let remaining = rechunk_buffer.len();
        tracing::info!("Sending final WAV frame with {} samples", remaining);
        let final_chunk: Vec<f32> = rechunk_buffer.into_iter().collect();
        if result_tx.blocking_send(Ok((final_chunk, sample_rate, channels))).is_err() {
            tracing::info!("Result channel closed before final frame could be sent");
            return Ok(());
        }
        frame_count += 1;

        tracing::info!(
            "WAV streaming demux complete: {} frames sent, {} total samples ({} full frames + {} remainder)",
            frame_count,
            total_samples_sent + remaining,
            frame_count - 1,
            remaining
        );
    }

    Ok(())
}

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the WAV demuxer node.
///
/// # Panics
///
/// Panics if the default WAV demuxer cannot be created (should never happen)
/// or if the config schema cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization and default config should never fail
pub fn register_wav_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "symphonia")]
    {
        let default_demuxer = WavDemuxerNode::new(WavDemuxerConfig::default())
            .expect("default WAV demuxer config should be valid");
        registry.register_static_with_description(
            "containers::wav::demuxer",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(WavDemuxerNode::new(config)?))
            },
            serde_json::to_value(schema_for!(WavDemuxerConfig))
                .expect("WavDemuxerConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_demuxer.input_pins(),
                outputs: default_demuxer.output_pins(),
            },
            vec!["containers".to_string(), "wav".to_string()],
            false,
            "Demuxes WAV audio files to raw PCM samples. \
             Accepts binary WAV data and outputs 48kHz stereo f32 audio.",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
    #[allow(clippy::uninlined_format_args)]
    fn read_sample_file(filename: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/audio").join(filename);
        std::fs::read(&path)
            .unwrap_or_else(|_| panic!("Failed to read test file: {}", path.display()))
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    async fn test_wav_demux() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        // Create WAV demuxer node
        let node = WavDemuxerNode::new(WavDemuxerConfig::default()).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Read and send WAV test file
        let wav_data = read_sample_file("sample.wav");
        let packet = create_test_binary_packet(wav_data);
        input_tx.send(packet).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify output
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert!(!output_packets.is_empty(), "Expected at least one output packet");

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio packet");
        assert!(!audio_data.is_empty(), "Expected non-empty audio data from WAV demuxer");

        if let Packet::Audio(frame) = &output_packets[0] {
            tracing::info!(
                "Demuxed WAV: {} Hz, {} channels, {} samples",
                frame.sample_rate,
                frame.channels,
                frame.samples.len()
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn test_wav_multiple_packets() {
        // Test that demuxer can handle data split across multiple packets
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = WavDemuxerNode::new(WavDemuxerConfig::default()).unwrap();

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Read WAV file and split into multiple packets
        let wav_data = read_sample_file("sample.wav");
        let chunk_size = wav_data.len() / 3;

        for i in 0..3 {
            let start = i * chunk_size;
            let end = if i == 2 { wav_data.len() } else { (i + 1) * chunk_size };
            let chunk = wav_data[start..end].to_vec();
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
