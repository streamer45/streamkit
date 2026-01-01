// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use ogg::reading::async_api::PacketReader;
use ogg::{PacketWriteEndInfo, PacketWriter};
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::io::Write;
use std::sync::{Arc, Mutex};
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    get_demuxer_buffer_size, get_stream_channel_capacity, state_helpers, InputPin, NodeContext,
    NodeRegistry, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::io::duplex;

// --- Ogg Constants ---

/// Default page flush threshold for Ogg muxer (typical max Ogg page size)
const DEFAULT_CHUNK_SIZE: usize = 65536;

// --- Ogg Muxer ---

// A shared, thread-safe buffer that implements io::Write. This is used to
// work around the borrow checker when using the ogg::PacketWriter, allowing
// us to stream out completed pages as they are written.
#[derive(Clone)]
struct SharedPacketBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedPacketBuffer {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl Write for SharedPacketBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Mutex poisoning is a fatal error - standard pattern in Rust
        #[allow(clippy::unwrap_used)]
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Mutex poisoning is a fatal error - standard pattern in Rust
        #[allow(clippy::unwrap_used)]
        self.0.lock().unwrap().flush()
    }
}

#[derive(Deserialize, Debug, Default, Clone, Copy, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OggMuxerCodec {
    #[default]
    Opus,
}

#[derive(Deserialize, Debug, JsonSchema)]
#[serde(default)]
pub struct OggMuxerConfig {
    // A serial number is required for an Ogg stream.
    pub stream_serial: u32,
    // The codec being muxed, to handle headers correctly.
    pub codec: OggMuxerCodec,
    /// Number of audio channels (1 for mono, 2 for stereo). Defaults to 1.
    pub channels: u8,
    /// The number of bytes to buffer before flushing to the output. Defaults to 65536.
    pub chunk_size: usize,
}

impl Default for OggMuxerConfig {
    fn default() -> Self {
        Self {
            stream_serial: 0,
            codec: OggMuxerCodec::default(),
            channels: 1, // Default to mono
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }
}

/// A node that muxes compressed packets (like Opus) into an Ogg container stream.
pub struct OggMuxerNode {
    config: OggMuxerConfig,
    is_first_packet: bool,
}

impl OggMuxerNode {
    pub const fn new(config: OggMuxerConfig) -> Self {
        Self { config, is_first_packet: true }
    }
}

#[async_trait]
impl ProcessorNode for OggMuxerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::OpusAudio], // Accepts Opus for now
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some("audio/ogg".to_string())
    }

    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("OggMuxerNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut input_rx = context.take_input("in")?;
        let mut packet_count = 0u64;
        let mut last_granule_pos = 0u64;

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        let shared_buffer = SharedPacketBuffer::new();

        {
            let mut writer_buffer = shared_buffer.clone();
            let mut writer = PacketWriter::new(&mut writer_buffer);

            // --- Write codec-specific headers before processing any input packets ---
            match self.config.codec {
                OggMuxerCodec::Opus => {
                    tracing::info!("Writing Opus headers to OGG stream");
                    // 1. Opus Identification Header (19 bytes)
                    // https://www.rfc-editor.org/rfc/rfc7845.html#section-5.1
                    let opus_head = vec![
                        b'O',
                        b'p',
                        b'u',
                        b's',
                        b'H',
                        b'e',
                        b'a',
                        b'd',                 // Magic signature
                        1,                    // Version
                        self.config.channels, // Channel count from config
                        0,
                        0, // Pre-skip (LE)
                        0x80,
                        0xBB,
                        0,
                        0, // 48000 Hz sample rate (LE)
                        0,
                        0, // Output gain (LE)
                        0, // Channel mapping family
                    ];
                    tracing::debug!("Writing OpusHead header...");
                    if let Err(e) = writer.write_packet(
                        opus_head,
                        self.config.stream_serial,
                        PacketWriteEndInfo::EndPage, // First packet must end a page.
                        0,                           // Granule position for headers is 0
                    ) {
                        let err_msg = format!("Failed to write OpusHead: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }
                    tracing::debug!("OpusHead written successfully");

                    // 2. Opus Comment Header
                    tracing::debug!("Writing OpusTags header...");
                    let vendor_string = "streamkit";
                    let mut opus_tags = Vec::new();
                    opus_tags.extend_from_slice(b"OpusTags");
                    // vendor_string is a constant &str, so len() will never exceed u32
                    #[allow(clippy::expect_used)]
                    let vendor_len = u32::try_from(vendor_string.len())
                        .expect("vendor string length fits in u32");
                    opus_tags.extend_from_slice(&vendor_len.to_le_bytes());
                    opus_tags.extend_from_slice(vendor_string.as_bytes());
                    opus_tags.extend_from_slice(&0_u32.to_le_bytes()); // 0 comments

                    if let Err(e) = writer.write_packet(
                        opus_tags,
                        self.config.stream_serial,
                        PacketWriteEndInfo::NormalPacket, // This doesn't need to end a page
                        0,
                    ) {
                        let err_msg = format!("Failed to write OpusTags: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }
                    tracing::debug!("OpusTags written successfully");
                },
            }

            tracing::info!("Headers written, entering receive loop to process incoming packets");
            while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
                if let Packet::Binary { data, metadata, .. } = packet {
                    packet_count += 1;
                    stats_tracker.received();
                    if packet_count.is_multiple_of(1000) {
                        tracing::debug!(
                            "OggMuxer processed {} packets (last packet: {} bytes)",
                            packet_count,
                            data.len()
                        );
                    }

                    // Force every packet to end a page for maximum streaming behavior.
                    // This allows chunk_size to work as expected by ensuring
                    // the buffer fills up regularly. Trade-off: slightly higher OGG overhead.
                    if self.is_first_packet {
                        self.is_first_packet = false;
                    }
                    let pck_info = PacketWriteEndInfo::EndPage;

                    // Calculate granule position from metadata if available, otherwise use packet count
                    // For Opus: granule position is at 48kHz sample rate
                    if let Some(meta) = metadata {
                        if let Some(timestamp_us) = meta.timestamp_us {
                            // Convert timestamp from microseconds to 48kHz samples
                            last_granule_pos = (timestamp_us * 48000) / 1_000_000;
                        } else if let Some(duration_us) = meta.duration_us {
                            // If we don't have timestamp but have duration, accumulate
                            let samples = (duration_us * 48000) / 1_000_000;
                            last_granule_pos += samples;
                        } else {
                            // Fallback: assume 960 samples (20ms at 48kHz)
                            last_granule_pos = 960 * packet_count;
                        }
                    } else {
                        // No metadata: fallback to assuming 960 samples per packet
                        last_granule_pos = 960 * packet_count;
                    }

                    if let Err(e) = writer.write_packet(
                        data.to_vec(),
                        self.config.stream_serial,
                        pck_info,
                        last_granule_pos,
                    ) {
                        stats_tracker.errored();
                        stats_tracker.maybe_send();
                        let err_msg = e.to_string();
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }

                    // Flush any bytes accumulated by the Ogg writer immediately to maximize streaming.
                    // This avoids buffering large chunks in memory and delivers data as soon as pages are ready.
                    let data_to_send = {
                        #[allow(clippy::unwrap_used)]
                        let mut buffer_guard = shared_buffer.0.lock().unwrap();
                        if buffer_guard.is_empty() {
                            drop(buffer_guard);
                            None
                        } else {
                            let data = Bytes::from(std::mem::take(&mut *buffer_guard));
                            drop(buffer_guard);
                            Some(data)
                        }
                    };

                    if let Some(data) = data_to_send {
                        if context
                            .output_sender
                            .send(
                                "out",
                                Packet::Binary {
                                    data,
                                    content_type: Some(Cow::Borrowed("audio/ogg")),
                                    metadata: None,
                                },
                            )
                            .await
                            .is_err()
                        {
                            tracing::debug!("Output channel closed, stopping muxer");
                            break;
                        }
                        stats_tracker.sent();
                    }
                    stats_tracker.maybe_send();
                }
            }
            tracing::info!(
                "OggMuxerNode input stream closed, processed {} packets total",
                packet_count
            );

            if let Err(e) = writer.write_packet(
                Vec::new(),
                self.config.stream_serial,
                PacketWriteEndInfo::EndStream,
                last_granule_pos,
            ) {
                let err_msg = e.to_string();
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Runtime(err_msg));
            }
        }

        // Flush any remaining data from the buffer.
        let data_to_send = {
            #[allow(clippy::unwrap_used)]
            let mut buffer_guard = shared_buffer.0.lock().unwrap();
            if buffer_guard.is_empty() {
                drop(buffer_guard);
                None
            } else {
                tracing::debug!("Writing final data, buffer size: {} bytes", buffer_guard.len());
                let data = Bytes::from(std::mem::take(&mut *buffer_guard));
                drop(buffer_guard);
                Some(data)
            }
        };

        if let Some(data) = data_to_send {
            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed("audio/ogg")),
                        metadata: None,
                    },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed during final flush");
                // Don't return error, we're already shutting down
            } else {
                stats_tracker.sent();
            }
            stats_tracker.force_send();
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("OggMuxerNode finished");
        Ok(())
    }
}

// --- Ogg Demuxer ---

#[derive(Deserialize, Debug, Default, JsonSchema)]
#[serde(default)]
pub struct OggDemuxerConfig {}

/// A node that demuxes an Ogg container stream into its underlying compressed packets.
pub struct OggDemuxerNode {
    _config: OggDemuxerConfig,
}

impl OggDemuxerNode {
    pub const fn new(config: OggDemuxerConfig) -> Self {
        Self { _config: config }
    }
}

#[async_trait]
impl ProcessorNode for OggDemuxerNode {
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
            produces_type: PacketType::OpusAudio,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("OggDemuxerNode starting");
        state_helpers::emit_running(&context.state_tx, &node_name);
        let mut input_rx = context.take_input("in")?;

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Create a duplex stream to feed data to the async PacketReader
        let (mut writer, reader) = duplex(get_demuxer_buffer_size());

        // Create the async packet reader
        let mut packet_reader = PacketReader::new(reader);

        // Clone cancellation token before moving context
        let cancellation_token = context.cancellation_token.clone();

        // Spawn a task to forward input data to the duplex writer
        let writer_task = tokio::spawn(async move {
            let mut input_chunks = 0;
            let mut total_bytes = 0;

            loop {
                let packet = if let Some(token) = &cancellation_token {
                    tokio::select! {
                        () = token.cancelled() => break,
                        packet = input_rx.recv() => packet,
                    }
                } else {
                    input_rx.recv().await
                };

                let Some(packet) = packet else { break };
                if let Packet::Binary { data, .. } = packet {
                    input_chunks += 1;
                    total_bytes += data.len();
                    tracing::debug!(
                        "Writing chunk {} with {} bytes to Ogg reader",
                        input_chunks,
                        data.len()
                    );

                    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut writer, &data).await {
                        tracing::error!("Failed to write data to Ogg reader: {}", e);
                        break;
                    }
                }
            }

            tracing::info!(
                "OggDemuxer input stream closed, received {} chunks with {} total bytes",
                input_chunks,
                total_bytes
            );

            // Close the writer to signal end of stream
            drop(writer);
        });

        // Process packets from the async reader
        let mut packets_extracted = 0u64;
        let mut last_granule_pos: Option<u64> = None;
        let mut packets_at_granule_pos = 0u64;
        let mut detected_frame_duration_us: Option<u64> = None;

        loop {
            let packet_result = if let Some(token) = &context.cancellation_token {
                tokio::select! {
                    () = token.cancelled() => {
                        tracing::info!("Ogg demuxer cancelled after {} packets", packets_extracted);
                        break;
                    }
                    result = packet_reader.next() => result,
                }
            } else {
                packet_reader.next().await
            };

            let Some(packet_result) = packet_result else {
                break;
            };

            match packet_result {
                Ok(packet) => {
                    packets_extracted += 1;
                    stats_tracker.received();
                    if packets_extracted.is_multiple_of(1000) {
                        tracing::debug!("OggDemuxer extracted {} packets", packets_extracted);
                    }

                    // Extract granule position for timing metadata
                    let granule_pos = packet.absgp_page();

                    // Calculate timing metadata from granule position
                    // For Opus (RFC 7845): granule position is at 48kHz sample rate
                    let metadata = if granule_pos > 0 {
                        let timestamp_us = (granule_pos * 1_000_000) / 48000;

                        // Determine packet duration
                        // Complex conditional logic for Opus frame duration detection
                        #[allow(clippy::option_if_let_else)]
                        #[allow(clippy::redundant_closure)]
                        let duration_us = detected_frame_duration_us.map_or_else(
                            || {
                                if let Some(last_gp) = last_granule_pos {
                                    if granule_pos > last_gp && packets_at_granule_pos > 0 {
                                        // First granule position change detected - calculate frame duration
                                        let total_duration_us =
                                            ((granule_pos - last_gp) * 1_000_000) / 48000;
                                        let frame_duration =
                                            total_duration_us / packets_at_granule_pos;
                                        detected_frame_duration_us = Some(frame_duration);

                                        // Precision loss acceptable for logging display
                                        #[allow(clippy::cast_precision_loss)]
                                        let frame_duration_ms = frame_duration as f64 / 1000.0;
                                        tracing::info!(
                                    "Detected Opus frame duration: {:.1}ms ({} packets per {}ms)",
                                    frame_duration_ms,
                                    packets_at_granule_pos,
                                    total_duration_us / 1000
                                );

                                        Some(frame_duration)
                                    } else {
                                        // Still on first granule position, use default
                                        Some(20_000) // 20ms default for Opus
                                    }
                                } else {
                                    // Very first packet with granule position
                                    Some(20_000) // 20ms default for Opus
                                }
                            },
                            |detected| Some(detected),
                        );

                        // Track packets at current granule position for probing
                        if Some(granule_pos) == last_granule_pos {
                            packets_at_granule_pos += 1;
                        } else {
                            packets_at_granule_pos = 1;
                            last_granule_pos = Some(granule_pos);
                        }

                        Some(streamkit_core::types::PacketMetadata {
                            timestamp_us: Some(timestamp_us),
                            duration_us,
                            sequence: Some(packets_extracted),
                        })
                    } else {
                        // No valid granule position (header packets)
                        None
                    };

                    // Send the packet data to the output with timing metadata
                    let output_packet = Packet::Binary {
                        data: Bytes::from(packet.data),
                        content_type: None,
                        metadata,
                    };
                    if context.output_sender.send("out", output_packet).await.is_err() {
                        tracing::debug!("Output channel closed, stopping demuxer");
                        break;
                    }
                    stats_tracker.sent();
                    stats_tracker.maybe_send();
                },
                Err(e) => {
                    stats_tracker.errored();
                    stats_tracker.maybe_send();
                    tracing::error!("Error reading Ogg packet: {}", e);
                    break;
                },
            }
        }

        // Give the writer task a brief moment to complete, then abort if needed
        // The writer task might be blocked waiting for input from upstream
        match tokio::time::timeout(std::time::Duration::from_millis(100), writer_task).await {
            Ok(Ok(())) => {
                tracing::debug!("Writer task completed gracefully");
            },
            Ok(Err(e)) => {
                tracing::error!("Writer task failed: {}", e);
            },
            Err(_) => {
                tracing::debug!("Writer task did not complete within timeout, continuing shutdown");
                // Task will be dropped and aborted automatically
            },
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("OggDemuxerNode finished, extracted {} packets total", packets_extracted);
        Ok(())
    }
}

// --- Symphonia-based Ogg Demuxer (Alternative Implementation) ---

#[cfg(feature = "symphonia")]
use symphonia::core::formats::FormatOptions;
#[cfg(feature = "symphonia")]
use symphonia::core::formats::FormatReader;
#[cfg(feature = "symphonia")]
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};

#[cfg(feature = "symphonia")]
use crate::streaming_utils::StreamingReader;

#[cfg(feature = "symphonia")]
#[derive(Deserialize, Debug, Default, JsonSchema)]
#[serde(default)]
pub struct SymphoniaOggDemuxerConfig {}

/// Symphonia-based Ogg demuxer node (more robust alternative to the ogg crate based one)
#[cfg(feature = "symphonia")]
pub struct SymphoniaOggDemuxerNode {
    _config: SymphoniaOggDemuxerConfig,
}

#[cfg(feature = "symphonia")]
impl SymphoniaOggDemuxerNode {
    pub const fn new(config: SymphoniaOggDemuxerConfig) -> Self {
        Self { _config: config }
    }
}

#[cfg(feature = "symphonia")]
#[async_trait]
impl ProcessorNode for SymphoniaOggDemuxerNode {
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
            produces_type: PacketType::OpusAudio,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("SymphoniaOggDemuxerNode starting");
        let mut input_rx = context.take_input("in")?;

        // Create channels for streaming reader and results
        let (data_tx, data_rx) =
            tokio::sync::mpsc::channel::<bytes::Bytes>(get_stream_channel_capacity());
        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<Result<Packet, String>>(32);

        // Spawn blocking task for Symphonia processing
        let state_tx = context.state_tx.clone();
        let stats_tx = context.stats_tx.clone();
        let cancellation_token = context.cancellation_token.clone();
        let node_name_clone = node_name.clone();

        // Ogg decoder state machine with streaming, packet handling, and channel conversion
        // Moderate complexity due to format probing, codec handling, and real-time streaming
        // Allows for common patterns in media decoding (unwraps on Results that should succeed, mutex locks, drops)
        #[allow(clippy::cognitive_complexity)]
        #[allow(clippy::unwrap_used)]
        #[allow(clippy::expect_used)]
        #[allow(clippy::significant_drop_tightening)]
        #[allow(clippy::cast_precision_loss)]
        #[allow(clippy::option_if_let_else)]
        let decode_task = tokio::task::spawn_blocking(move || {
            let node_name = node_name_clone;
            let reader = StreamingReader::new(data_rx);
            #[allow(clippy::default_trait_access)]
            let mss =
                MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());

            let format_opts = FormatOptions::default();
            // Explicitly instantiate the Ogg demuxer instead of using Symphonia's probe.
            //
            // Symphonia's probe scans for *any* supported format marker and returns the first
            // match; on Ogg/Opus input it can occasionally lock onto a false-positive MP3 marker
            // later in the stream, causing very noisy "skipping junk" logs and incorrect demuxing.
            let mut format_reader =
                match symphonia::default::formats::OggReader::try_new(mss, &format_opts) {
                    Ok(reader) => reader,
                    Err(e) => {
                        state_helpers::emit_failed(
                            &state_tx,
                            &node_name,
                            format!("Failed to open Ogg stream: {e}"),
                        );
                        let _ = result_tx.blocking_send(Err(format!("Failed to open Ogg: {e}")));
                        return;
                    },
                };
            let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), stats_tx);

            state_helpers::emit_running(&state_tx, &node_name);

            // Read packets and forward them
            let mut packets_extracted = 0;
            loop {
                if let Some(token) = &cancellation_token {
                    if token.is_cancelled() {
                        tracing::info!(
                            "Symphonia Ogg demuxer cancelled after {} packets",
                            packets_extracted
                        );
                        break;
                    }
                }

                match format_reader.next_packet() {
                    Ok(packet) => {
                        packets_extracted += 1;
                        stats_tracker.received();

                        // Extract timing metadata
                        let metadata = if packet.ts() > 0 {
                            // Opus uses 48kHz timebase
                            let timestamp_us = (packet.ts() * 1_000_000) / 48000;
                            let duration_us = (packet.dur() * 1_000_000) / 48000;

                            Some(streamkit_core::types::PacketMetadata {
                                timestamp_us: Some(timestamp_us),
                                duration_us: Some(duration_us),
                                sequence: Some(packets_extracted),
                            })
                        } else {
                            None
                        };

                        // tracing::debug!(
                        //     "Extracted Ogg packet {} with {} bytes (ts: {}, dur: {})",
                        //     packets_extracted,
                        //     packet.data.len(),
                        //     packet.ts(),
                        //     packet.dur()
                        // );

                        let output_packet = Packet::Binary {
                            data: Bytes::from(Vec::from(packet.data)),
                            content_type: None,
                            metadata,
                        };

                        // Send packet through result channel
                        if result_tx.blocking_send(Ok(output_packet)).is_err() {
                            tracing::debug!("Result channel closed, stopping demuxer");
                            break;
                        }

                        stats_tracker.sent();
                        stats_tracker.maybe_send();
                    },
                    Err(symphonia::core::errors::Error::IoError(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        tracing::info!(
                            "Reached end of Ogg stream after {} packets",
                            packets_extracted
                        );
                        break;
                    },
                    Err(e) => {
                        stats_tracker.errored();
                        stats_tracker.maybe_send();
                        tracing::error!("Error reading Ogg packet: {}", e);
                        state_helpers::emit_failed(
                            &state_tx,
                            &node_name,
                            format!("Read error: {e}"),
                        );
                        let _ = result_tx.blocking_send(Err(format!("Read error: {e}")));
                        break;
                    },
                }
            }

            stats_tracker.force_send();
            state_helpers::emit_stopped(&state_tx, &node_name, "input_closed");
            tracing::info!(
                "SymphoniaOggDemuxerNode finished, extracted {} packets",
                packets_extracted
            );
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Stream input data to the blocking demuxer.
        // This is a separate task so the main loop can keep draining demux results even when
        // the stream channel is full (avoids deadlocks).
        let mut input_task = tokio::spawn(async move {
            let data_tx = data_tx;
            while let Some(packet) = input_rx.recv().await {
                if let Packet::Binary { data, .. } = packet {
                    if data_tx.send(data).await.is_err() {
                        break;
                    }
                }
            }
        });
        let mut input_done = false;

        loop {
            tokio::select! {
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some(Ok(output_packet)) => {
                            stats_tracker.received();

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
                            stats_tracker.errored();
                            stats_tracker.maybe_send();
                            let err_msg = format!("Ogg demux error: {e}");
                            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                            return Err(StreamKitError::Runtime(err_msg));
                        }
                        None => {
                            // Result channel closed, blocking task is done
                            break;
                        }
                    }
                }
                Some(ctrl_msg) = context.control_rx.recv() => {
                    if matches!(ctrl_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("SymphoniaOggDemuxerNode received shutdown signal");
                        // Abort blocking decode task for immediate shutdown
                        decode_task.abort();
                        input_task.abort();
                        // Break out of main loop
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

        stats_tracker.force_send();

        // Abort the blocking task if not already aborted (for immediate shutdown)
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

        Ok(())
    }
}

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the Ogg container nodes.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_ogg_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "ogg")]
    {
        let default_muxer = OggMuxerNode::new(OggMuxerConfig::default());
        registry.register_static_with_description(
            "containers::ogg::muxer",
            |params| {
                let config = config_helpers::parse_config_with_context(params, "OggMuxer")?;
                Ok(Box::new(OggMuxerNode::new(config)))
            },
            serde_json::to_value(schema_for!(OggMuxerConfig))
                .expect("OggMuxerConfig schema should serialize to JSON"),
            StaticPins { inputs: default_muxer.input_pins(), outputs: default_muxer.output_pins() },
            vec!["containers".to_string(), "ogg".to_string()],
            false,
            "Muxes Opus audio packets into an Ogg container. \
             Produces streamable Ogg/Opus output for playback or storage.",
        );
    }

    // Use Symphonia-based demuxer if available, otherwise fall back to ogg crate
    #[cfg(feature = "symphonia")]
    {
        let default_demuxer = SymphoniaOggDemuxerNode::new(SymphoniaOggDemuxerConfig::default());
        registry.register_static_with_description(
            "containers::ogg::demuxer",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(SymphoniaOggDemuxerNode::new(config)))
            },
            serde_json::to_value(schema_for!(SymphoniaOggDemuxerConfig))
                .expect("SymphoniaOggDemuxerConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_demuxer.input_pins(),
                outputs: default_demuxer.output_pins(),
            },
            vec!["containers".to_string(), "ogg".to_string()],
            false,
            "Demuxes Ogg containers to extract Opus audio packets. \
             Accepts binary Ogg data and outputs Opus-encoded audio frames.",
        );
    }
    #[cfg(all(feature = "ogg", not(feature = "symphonia")))]
    {
        let default_demuxer = OggDemuxerNode::new(OggDemuxerConfig::default());
        registry.register_static_with_description(
            "containers::ogg::demuxer",
            |params| {
                let config = config_helpers::parse_config_optional(params)?;
                Ok(Box::new(OggDemuxerNode::new(config)))
            },
            serde_json::to_value(schema_for!(OggDemuxerConfig))
                .expect("OggDemuxerConfig schema should serialize to JSON"),
            StaticPins {
                inputs: default_demuxer.input_pins(),
                outputs: default_demuxer.output_pins(),
            },
            vec!["containers".to_string(), "ogg".to_string()],
            false,
            "Demuxes Ogg containers to extract Opus audio packets. \
             Accepts binary Ogg data and outputs Opus-encoded audio frames.",
        );
    }
}
