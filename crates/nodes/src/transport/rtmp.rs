// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! RTMP publisher (sink) node.
//!
//! Uses [`shiguredo_rtmp`] (a Sans I/O RTMP library) to publish encoded
//! H.264 video and AAC audio to an arbitrary RTMP or RTMPS endpoint
//! (e.g. YouTube Live, Twitch).
//!
//! The node manages the TCP (or TLS) socket itself, feeding bytes between
//! tokio I/O and the library's `feed_recv_buf()` / `send_buf()` interface.

use std::sync::Arc;

use async_trait::async_trait;
use opentelemetry::KeyValue;
use schemars::schema_for;
use schemars::JsonSchema;
use serde::Deserialize;
use shiguredo_rtmp::{
    AudioFormat as RtmpAudioFormat, AudioFrame as RtmpAudioFrame, AvcPacketType, AvcSequenceHeader,
    RtmpConnectionState, RtmpPublishClientConnection, RtmpTimestamp, RtmpTimestampDelta, RtmpUrl,
    VideoCodec as RtmpVideoCodec, VideoFrame as RtmpVideoFrame, VideoFrameType,
};
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketType, VideoCodec,
};
use streamkit_core::{
    config_helpers, registry::StaticPins, state_helpers, InputPin, NodeContext, NodeRegistry,
    OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the RTMP publisher node.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RtmpPublishConfig {
    /// Full RTMP URL including stream key.
    ///
    /// Supports `rtmp://` and `rtmps://` (TLS) schemes.
    ///
    /// Examples:
    /// - `rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx`
    /// - `rtmps://live.twitch.tv/app/live_xxxx`
    pub url: String,

    /// Audio sample rate in Hz for the AAC sequence header.
    ///
    /// Must match the sample rate produced by the upstream AAC encoder.
    /// Common values: 48000, 44100, 32000.
    /// Defaults to 48000.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Number of audio channels for the AAC sequence header.
    ///
    /// Must match the channel count produced by the upstream AAC encoder.
    /// 1 = mono, 2 = stereo.
    /// Defaults to 2 (stereo).
    #[serde(default = "default_channels")]
    pub channels: u8,
}

const fn default_sample_rate() -> u32 {
    48_000
}

const fn default_channels() -> u8 {
    2
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// RTMP publisher sink node.
///
/// Accepts encoded H.264 video and AAC audio on separate input pins and
/// publishes them to an RTMP endpoint using the FLV/RTMP wire format.
pub struct RtmpPublishNode {
    config: RtmpPublishConfig,
}

impl RtmpPublishNode {
    pub const fn new(config: RtmpPublishConfig) -> Self {
        Self { config }
    }
}

// ---------------------------------------------------------------------------
// ProcessorNode implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ProcessorNode for RtmpPublishNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![
            InputPin {
                name: "video".to_string(),
                accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                    codec: VideoCodec::H264,
                    bitstream_format: None,
                    codec_private: None,
                    profile: None,
                    level: None,
                })],
                cardinality: PinCardinality::One,
            },
            InputPin {
                name: "audio".to_string(),
                accepts_types: vec![PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Aac,
                    codec_private: None,
                })],
                cardinality: PinCardinality::One,
            },
        ]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        // Sink node — no outputs.
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        // Log without the stream key (it's effectively a bearer token).
        let masked_url = mask_stream_key(&self.config.url);
        tracing::info!(%node_name, url = %masked_url, "RtmpPublishNode starting");

        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // ── Parse RTMP URL ──────────────────────────────────────────────
        let rtmp_url: RtmpUrl = self.config.url.parse().map_err(|e| {
            StreamKitError::Configuration(format!(
                "Invalid RTMP URL '{}': {e}",
                mask_stream_key(&self.config.url)
            ))
        })?;

        tracing::info!(
            %node_name,
            host = %rtmp_url.host, port = rtmp_url.port,
            app = %rtmp_url.app, tls = rtmp_url.tls,
            "Parsed RTMP URL"
        );

        // ── Connect TCP (+ optional TLS) ────────────────────────────────
        let mut stream = connect(&rtmp_url).await.map_err(|e| {
            let msg = format!("Failed to connect to RTMP server: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(%node_name, "TCP connection established");

        // ── Create RTMP connection and drive handshake ───────────────────
        let mut connection = RtmpPublishClientConnection::new(rtmp_url);

        drive_handshake(&mut connection, &mut stream, &node_name).await.map_err(|e| {
            let msg = format!("RTMP handshake failed: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(%node_name, "RTMP connection in Publishing state");
        state_helpers::emit_running(&context.state_tx, &node_name);

        // ── Obtain input receivers ──────────────────────────────────────
        let mut video_rx = context.take_input("video")?;
        let mut audio_rx = context.take_input("audio")?;

        // ── Stats / metrics ─────────────────────────────────────────────
        let meter = opentelemetry::global::meter("streamkit");
        let packet_counter = meter.u64_counter("rtmp_publish.packets").build();
        let metric_labels = [KeyValue::new("node", node_name.clone())];
        let mut stats = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // ── Publishing state ────────────────────────────────────────────
        let mut audio_seq_header_sent = false;
        let mut packet_count: u64 = 0;
        let mut tcp_read_buf = vec![0u8; 8192];

        // ── Main publishing loop ────────────────────────────────────────
        tracing::info!(%node_name, "Entering RTMP publishing loop");

        loop {
            tokio::select! {
                // Video input
                maybe_pkt = video_rx.recv() => {
                    let Some(pkt) = maybe_pkt else {
                        tracing::info!(%node_name, "Video input channel closed");
                        break;
                    };
                    if let Err(e) = process_video_packet(
                        &pkt, &mut connection, &packet_counter, &metric_labels,
                        &mut stats, &mut packet_count, &node_name,
                    ) {
                        tracing::warn!(%node_name, error = %e, "Error processing video packet");
                        stats.errored();
                    }
                    flush_send_buf(&mut connection, &mut stream).await?;
                }

                // Audio input
                maybe_pkt = audio_rx.recv() => {
                    let Some(pkt) = maybe_pkt else {
                        tracing::info!(%node_name, "Audio input channel closed");
                        break;
                    };
                    if let Err(e) = process_audio_packet(
                        &pkt, &mut connection, &mut audio_seq_header_sent,
                        self.config.sample_rate, self.config.channels,
                        &packet_counter, &metric_labels,
                        &mut stats, &mut packet_count, &node_name,
                    ) {
                        tracing::warn!(%node_name, error = %e, "Error processing audio packet");
                        stats.errored();
                    }
                    flush_send_buf(&mut connection, &mut stream).await?;
                }

                // TCP read (server responses / keepalive)
                read_result = stream.read(&mut tcp_read_buf) => {
                    match read_result {
                        Ok(0) => {
                            tracing::warn!(%node_name, "RTMP server closed connection");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = connection.feed_recv_buf(&tcp_read_buf[..n]) {
                                tracing::warn!(%node_name, error = %e, "Error feeding RTMP recv buffer");
                            }
                            // Drain events (acks, pings, etc.)
                            if drain_events(&mut connection, &node_name) {
                                tracing::info!(%node_name, "Breaking loop: peer disconnected");
                                break;
                            }
                            flush_send_buf(&mut connection, &mut stream).await?;
                        }
                        Err(e) => {
                            tracing::warn!(%node_name, error = %e, "TCP read error");
                            break;
                        }
                    }
                }

                // Shutdown signal
                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!(%node_name, "Received shutdown signal");
                        break;
                    }
                }
            }

            stats.maybe_send();
        }

        tracing::info!(%node_name, packets = packet_count, "RTMP publishing finished");
        state_helpers::emit_stopped(&context.state_tx, &node_name, "finished");

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TCP / TLS connection helpers
// ---------------------------------------------------------------------------

/// Unified async stream over plain TCP or TLS.
enum RtmpStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl RtmpStream {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf).await,
            Self::Tls(s) => s.read(buf).await,
        }
    }

    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.write_all(buf).await,
            Self::Tls(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => tokio::io::AsyncWriteExt::flush(s).await,
            Self::Tls(s) => tokio::io::AsyncWriteExt::flush(s).await,
        }
    }
}

/// Mask the stream-key portion of an RTMP URL for safe logging.
///
/// Returns the URL with everything after the last `/` in the path replaced
/// by `****`.  If parsing fails, returns `<redacted>`.
fn mask_stream_key(url: &str) -> String {
    url.rfind('/')
        .map_or_else(|| "<redacted>".to_string(), |idx| format!("{}/<redacted>", &url[..idx]))
}

/// Connect to the RTMP server, using TLS if the URL scheme is `rtmps://`.
async fn connect(url: &RtmpUrl) -> Result<RtmpStream, String> {
    let addr = format!("{}:{}", url.host, url.port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("TCP connect to {addr} failed: {e}"))?;

    if url.tls {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(rustls_platform_verifier::Verifier::new()))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let server_name = rustls::pki_types::ServerName::try_from(url.host.clone())
            .map_err(|e| format!("Invalid TLS server name '{}': {e}", url.host))?;
        let tls_stream = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| format!("TLS handshake with {} failed: {e}", url.host))?;
        Ok(RtmpStream::Tls(Box::new(tls_stream)))
    } else {
        Ok(RtmpStream::Plain(tcp))
    }
}

// ---------------------------------------------------------------------------
// RTMP protocol helpers
// ---------------------------------------------------------------------------

/// Drive the RTMP handshake until the connection reaches [`RtmpConnectionState::Publishing`].
async fn drive_handshake(
    connection: &mut RtmpPublishClientConnection,
    stream: &mut RtmpStream,
    node_name: &str,
) -> Result<(), String> {
    let mut recv_buf = vec![0u8; 8192];

    loop {
        // Flush outgoing data first.
        flush_send_buf_raw(connection, stream)
            .await
            .map_err(|e| format!("Handshake write failed: {e}"))?;

        if connection.state() == RtmpConnectionState::Publishing {
            return Ok(());
        }

        // Wait for data from the server (with timeout).
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut recv_buf))
                .await;

        match read_result {
            Ok(Ok(0)) => return Err("Server closed connection during handshake".to_string()),
            Ok(Ok(n)) => {
                connection
                    .feed_recv_buf(&recv_buf[..n])
                    .map_err(|e| format!("Handshake feed error: {e}"))?;
            },
            Ok(Err(e)) => return Err(format!("Handshake read error: {e}")),
            Err(_) => return Err("Handshake timed out after 10s".to_string()),
        }

        // Process events emitted by the handshake.
        while let Some(event) = connection.next_event() {
            tracing::debug!(%node_name, ?event, "RTMP handshake event");
        }
    }
}

/// Flush the RTMP connection's send buffer to the TCP stream.
async fn flush_send_buf(
    connection: &mut RtmpPublishClientConnection,
    stream: &mut RtmpStream,
) -> Result<(), StreamKitError> {
    flush_send_buf_raw(connection, stream)
        .await
        .map_err(|e| StreamKitError::Runtime(format!("RTMP send failed: {e}")))
}

/// Flush the RTMP connection's send buffer (returns raw io::Error).
async fn flush_send_buf_raw(
    connection: &mut RtmpPublishClientConnection,
    stream: &mut RtmpStream,
) -> std::io::Result<()> {
    while !connection.send_buf().is_empty() {
        let buf = connection.send_buf();
        stream.write_all(buf).await?;
        let len = buf.len();
        connection.advance_send_buf(len);
    }
    // Explicit flush to ensure TLS buffered data is sent immediately.
    stream.flush().await?;
    Ok(())
}

/// Drain and log any pending RTMP events (acks, pings, ignored commands).
///
/// Returns `true` if the peer signalled a disconnect, indicating that the
/// publishing loop should exit.
fn drain_events(connection: &mut RtmpPublishClientConnection, node_name: &str) -> bool {
    let mut disconnected = false;
    while let Some(event) = connection.next_event() {
        match &event {
            shiguredo_rtmp::RtmpConnectionEvent::DisconnectedByPeer { reason } => {
                tracing::warn!(%node_name, %reason, "RTMP server disconnected");
                disconnected = true;
            },
            shiguredo_rtmp::RtmpConnectionEvent::StateChanged(state) => {
                tracing::info!(%node_name, %state, "RTMP state changed");
            },
            _ => {
                tracing::debug!(%node_name, ?event, "RTMP event");
            },
        }
    }
    disconnected
}

// ---------------------------------------------------------------------------
// Video packet processing
// ---------------------------------------------------------------------------

/// Process one encoded video packet and send it via RTMP.
///
/// Converts H.264 Annex B to AVCC format, extracts SPS/PPS on keyframes
/// to send as an AVC sequence header, then sends the video frame.
#[allow(clippy::too_many_arguments)] // Packet-processing context (connection, counters, stats) is passed individually; bundling into a struct is a future cleanup.
fn process_video_packet(
    packet: &Packet,
    connection: &mut RtmpPublishClientConnection,
    counter: &opentelemetry::metrics::Counter<u64>,
    labels: &[KeyValue],
    stats: &mut NodeStatsTracker,
    packet_count: &mut u64,
    node_name: &str,
) -> Result<(), StreamKitError> {
    let Packet::Binary { data, metadata, .. } = packet else {
        tracing::debug!(%node_name, "Ignoring non-binary video packet");
        stats.discarded();
        return Ok(());
    };

    stats.received();

    #[allow(clippy::cast_possible_truncation)]
    // RTMP timestamps are u32 ms; wrapping after ~49 days is acceptable.
    let timestamp_ms =
        metadata.as_ref().and_then(|m| m.timestamp_us).map_or(0, |us| (us / 1_000) as u32);
    let keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);

    // Convert H.264 Annex B → AVCC
    let conv = convert_annexb_to_avcc(data);

    // On keyframes, send the AVC sequence header (SPS/PPS) first.
    if keyframe && !conv.sps_list.is_empty() && !conv.pps_list.is_empty() {
        let sps = &conv.sps_list[0];
        let (profile, compat, level) = if sps.len() >= 4 {
            (sps[1], sps[2], sps[3])
        } else {
            // Fallback: Constrained Baseline Level 3.1
            (0x42, 0xC0, 0x1F)
        };

        let seq_header = AvcSequenceHeader {
            avc_profile_indication: profile,
            profile_compatibility: compat,
            avc_level_indication: level,
            length_size_minus_one: 3, // 4-byte NAL unit lengths
            sps_list: conv.sps_list.clone(),
            pps_list: conv.pps_list.clone(),
        };

        let seq_data = seq_header.to_bytes().map_err(|e| {
            StreamKitError::Runtime(format!("Failed to serialize AVC sequence header: {e}"))
        })?;

        let seq_frame = RtmpVideoFrame {
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
            composition_timestamp_offset: RtmpTimestampDelta::ZERO,
            frame_type: VideoFrameType::KeyFrame,
            codec: RtmpVideoCodec::Avc,
            avc_packet_type: Some(AvcPacketType::SequenceHeader),
            data: seq_data,
        };

        connection.send_video(seq_frame).map_err(|e| {
            StreamKitError::Runtime(format!("Failed to send AVC sequence header: {e}"))
        })?;

        tracing::debug!(%node_name, %timestamp_ms, "Sent AVC sequence header");
    }

    // Send the actual video data (AVCC-formatted), excluding SPS/PPS NALUs
    // which are already conveyed in the sequence header above.
    let frame = RtmpVideoFrame {
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
        composition_timestamp_offset: RtmpTimestampDelta::ZERO,
        frame_type: if keyframe { VideoFrameType::KeyFrame } else { VideoFrameType::InterFrame },
        codec: RtmpVideoCodec::Avc,
        avc_packet_type: Some(AvcPacketType::NalUnit),
        data: conv.video_data,
    };

    connection
        .send_video(frame)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to send video frame: {e}")))?;

    *packet_count += 1;
    counter.add(1, labels);
    stats.sent();

    if *packet_count <= 5 || (*packet_count).is_multiple_of(100) {
        tracing::debug!(%node_name, packet = *packet_count, %timestamp_ms, %keyframe, "Sent video");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Audio packet processing
// ---------------------------------------------------------------------------

/// Process one encoded audio packet and send it via RTMP.
///
/// On the first audio packet, sends an AAC `AudioSpecificConfig` as the
/// RTMP sequence header.  Subsequent packets are sent as raw AAC frames.
#[allow(clippy::too_many_arguments)] // Packet-processing context (connection, counters, stats) is passed individually; bundling into a struct is a future cleanup.
fn process_audio_packet(
    packet: &Packet,
    connection: &mut RtmpPublishClientConnection,
    seq_header_sent: &mut bool,
    sample_rate: u32,
    channels: u8,
    counter: &opentelemetry::metrics::Counter<u64>,
    labels: &[KeyValue],
    stats: &mut NodeStatsTracker,
    packet_count: &mut u64,
    node_name: &str,
) -> Result<(), StreamKitError> {
    let Packet::Binary { data, .. } = packet else {
        tracing::debug!(%node_name, "Ignoring non-binary audio packet");
        stats.discarded();
        return Ok(());
    };

    stats.received();

    #[allow(clippy::cast_possible_truncation)]
    // RTMP timestamps are u32 ms; wrapping after ~49 days is acceptable.
    let timestamp_ms = match packet {
        Packet::Binary { metadata, .. } => {
            metadata.as_ref().and_then(|m| m.timestamp_us).map_or(0, |us| (us / 1_000) as u32)
        },
        _ => 0,
    };

    // Send AAC sequence header (AudioSpecificConfig) on first audio packet.
    if !*seq_header_sent {
        let asc = build_aac_audio_specific_config(sample_rate, channels);

        let seq_frame = RtmpAudioFrame {
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
            format: RtmpAudioFormat::Aac,
            sample_rate: RtmpAudioFrame::AAC_SAMPLE_RATE,
            is_stereo: RtmpAudioFrame::AAC_STEREO,
            is_8bit_sample: false,
            is_aac_sequence_header: true,
            data: asc,
        };

        connection.send_audio(seq_frame).map_err(|e| {
            StreamKitError::Runtime(format!("Failed to send AAC sequence header: {e}"))
        })?;

        tracing::info!(%node_name, "Sent AAC sequence header (AudioSpecificConfig)");
        *seq_header_sent = true;
    }

    // Send the raw AAC frame.
    let frame = RtmpAudioFrame {
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
        format: RtmpAudioFormat::Aac,
        sample_rate: RtmpAudioFrame::AAC_SAMPLE_RATE,
        is_stereo: RtmpAudioFrame::AAC_STEREO,
        is_8bit_sample: false,
        is_aac_sequence_header: false,
        data: data.to_vec(),
    };

    connection
        .send_audio(frame)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to send audio frame: {e}")))?;

    *packet_count += 1;
    counter.add(1, labels);
    stats.sent();

    if *packet_count <= 5 || (*packet_count).is_multiple_of(200) {
        tracing::debug!(%node_name, packet = *packet_count, %timestamp_ms, "Sent audio");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// H.264 Annex B → AVCC conversion
// ---------------------------------------------------------------------------
//
// These helpers mirror the logic in `containers/mp4.rs`.  A shared
// `h264_utils` module could deduplicate them in a follow-up refactor.

/// NAL unit type bitmask (lower 5 bits of NAL header byte).
const H264_NAL_TYPE_MASK: u8 = 0x1F;
/// NAL unit type: Sequence Parameter Set.
const H264_NAL_SPS: u8 = 7;
/// NAL unit type: Picture Parameter Set.
const H264_NAL_PPS: u8 = 8;

/// Result of converting an H.264 Annex B access unit to AVCC format.
struct AvccConversion {
    /// AVCC-formatted video data (4-byte length-prefixed NAL units),
    /// excluding SPS/PPS parameter sets (those go in the sequence header).
    video_data: Vec<u8>,
    /// SPS NAL units found in this access unit.
    sps_list: Vec<Vec<u8>>,
    /// PPS NAL units found in this access unit.
    pps_list: Vec<Vec<u8>>,
}

/// Parse an H.264 Annex B bitstream into individual NAL unit payloads.
///
/// NAL units are delimited by 3-byte (`00 00 01`) or 4-byte (`00 00 00 01`)
/// start codes.  The returned slices exclude the start-code prefix.
fn parse_annexb_nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut nal_start: Option<usize> = None;
    let len = data.len();
    let mut i = 0;

    while i < len {
        let sc_len = if i + 2 < len && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else if i + 3 < len
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            4
        } else {
            0
        };

        if sc_len > 0 {
            if let Some(start) = nal_start {
                if start < i {
                    nals.push(&data[start..i]);
                }
            }
            i += sc_len;
            nal_start = Some(i);
        } else {
            i += 1;
        }
    }

    if let Some(start) = nal_start {
        if start < len {
            nals.push(&data[start..len]);
        }
    }

    nals
}

/// Convert an H.264 Annex B bitstream to AVCC format.
///
/// Each NAL unit's start code is replaced with a 4-byte big-endian length
/// prefix.  SPS and PPS NAL units are extracted separately so the caller
/// can build the RTMP `AvcSequenceHeader`.
fn convert_annexb_to_avcc(data: &[u8]) -> AvccConversion {
    let nals = parse_annexb_nal_units(data);
    let mut video_data = Vec::with_capacity(data.len());
    let mut sps_list = Vec::new();
    let mut pps_list = Vec::new();

    for nal in nals {
        if nal.is_empty() {
            continue;
        }

        // Classify and extract parameter sets.
        let nal_type = nal[0] & H264_NAL_TYPE_MASK;
        if nal_type == H264_NAL_SPS {
            sps_list.push(nal.to_vec());
            continue; // SPS goes in the sequence header, not the NalUnit data.
        } else if nal_type == H264_NAL_PPS {
            pps_list.push(nal.to_vec());
            continue; // PPS goes in the sequence header, not the NalUnit data.
        }

        // 4-byte big-endian length prefix.
        let len = u32::try_from(nal.len()).unwrap_or(u32::MAX);
        video_data.extend_from_slice(&len.to_be_bytes());
        video_data.extend_from_slice(nal);
    }

    AvccConversion { video_data, sps_list, pps_list }
}

// ---------------------------------------------------------------------------
// AAC AudioSpecificConfig builder
// ---------------------------------------------------------------------------

/// Build a 2-byte AAC-LC `AudioSpecificConfig` for the RTMP sequence header.
///
/// Layout (ISO 14496-3 §1.6.2.1):
///
/// ```text
/// 5 bits  audioObjectType      (2 = AAC-LC)
/// 4 bits  samplingFrequencyIndex
/// 4 bits  channelConfiguration
/// 3 bits  GASpecificConfig (frameLengthFlag=0, dependsOnCoreCoder=0, extensionFlag=0)
/// ```
fn build_aac_audio_specific_config(sample_rate: u32, channels: u8) -> Vec<u8> {
    let freq_index: u8 = match sample_rate {
        96_000 => 0,
        88_200 => 1,
        64_000 => 2,
        48_000 => 3,
        44_100 => 4,
        32_000 => 5,
        24_000 => 6,
        22_050 => 7,
        16_000 => 8,
        12_000 => 9,
        11_025 => 10,
        8_000 => 11,
        7_350 => 12,
        _ => {
            tracing::warn!(sample_rate, "Unrecognized AAC sample rate, defaulting to 48 kHz index");
            3
        },
    };

    // AAC-LC object type = 2
    let object_type: u8 = 2;

    // Pack: 5 bits objectType | 4 bits freqIndex | 4 bits channels | 3 bits zeros
    let byte0 = (object_type << 3) | (freq_index >> 1);
    let byte1 = (freq_index << 7) | (channels << 3);

    vec![byte0, byte1]
}

// ---------------------------------------------------------------------------
// Node registration
// ---------------------------------------------------------------------------

/// Registers all RTMP transport nodes with the engine's registry.
///
/// # Panics
///
/// Panics if `RtmpPublishConfig`'s JSON schema fails to serialize, which
/// should never happen for a valid `schemars`-derived type.
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_rtmp_nodes(registry: &mut NodeRegistry) {
    let default_node = RtmpPublishNode::new(RtmpPublishConfig {
        url: String::new(),
        sample_rate: default_sample_rate(),
        channels: default_channels(),
    });

    registry.register_static_with_description(
        "transport::rtmp::publish",
        |params| {
            let config = config_helpers::parse_config_required(params)?;
            Ok(Box::new(RtmpPublishNode::new(config)))
        },
        serde_json::to_value(schema_for!(RtmpPublishConfig))
            .expect("RtmpPublishConfig schema should serialize to JSON"),
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        vec!["transport".to_string(), "rtmp".to_string()],
        false,
        "Publishes encoded H.264 video and AAC audio to an RTMP endpoint. \
         Accepts Annex B H.264 on the 'video' pin and raw AAC frames on the 'audio' pin, \
         converting to the RTMP/FLV wire format. Supports both RTMP and RTMPS (TLS).",
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_annexb_single_nal_4byte_sc() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0xAA, 0xBB];
        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 1);
        assert_eq!(nals[0], &[0x67, 0xAA, 0xBB]);
    }

    #[test]
    fn parse_annexb_single_nal_3byte_sc() {
        let data = [0x00, 0x00, 0x01, 0x68, 0xCC, 0xDD];
        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 1);
        assert_eq!(nals[0], &[0x68, 0xCC, 0xDD]);
    }

    #[test]
    fn parse_annexb_multiple_nals() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // SPS start code
        data.extend_from_slice(&[0x67, 0x42, 0xC0, 0x1F]); // SPS NAL
        data.extend_from_slice(&[0x00, 0x00, 0x01]); // PPS start code
        data.extend_from_slice(&[0x68, 0xCE, 0x38, 0x80]); // PPS NAL
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // IDR start code
        data.extend_from_slice(&[0x65, 0x88, 0x84]); // IDR NAL

        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(nals[0], &[0x67, 0x42, 0xC0, 0x1F]); // SPS
        assert_eq!(nals[1], &[0x68, 0xCE, 0x38, 0x80]); // PPS
        assert_eq!(nals[2], &[0x65, 0x88, 0x84]); // IDR
    }

    #[test]
    fn parse_annexb_empty_input() {
        let nals = parse_annexb_nal_units(&[]);
        assert!(nals.is_empty());
    }

    #[test]
    fn convert_annexb_extracts_sps_pps() {
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let sps = [0x67, 0x42, 0xC0, 0x1F]; // SPS NAL (type 7)
        annexb.extend_from_slice(&sps);
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let pps = [0x68, 0xCE, 0x38, 0x80]; // PPS NAL (type 8)
        annexb.extend_from_slice(&pps);
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let idr = [0x65, 0x88, 0x84]; // IDR NAL (type 5)
        annexb.extend_from_slice(&idr);

        let result = convert_annexb_to_avcc(&annexb);

        assert_eq!(result.sps_list.len(), 1);
        assert_eq!(result.pps_list.len(), 1);
        assert_eq!(result.sps_list[0], sps.to_vec());
        assert_eq!(result.pps_list[0], pps.to_vec());

        // Verify AVCC video_data contains only the IDR NAL (SPS/PPS excluded).
        let avcc = &result.video_data;
        let len = u32::from_be_bytes([avcc[0], avcc[1], avcc[2], avcc[3]]) as usize;
        assert_eq!(len, idr.len());
        assert_eq!(&avcc[4..4 + len], &idr[..]);
        assert_eq!(avcc.len(), 4 + idr.len());
    }

    #[test]
    fn aac_audio_specific_config_48khz_stereo() {
        let asc = build_aac_audio_specific_config(48_000, 2);
        assert_eq!(asc.len(), 2);
        // AAC-LC=2 (00010), freqIdx=3 (0011), channels=2 (0010), GASpec=000
        // 00010 0011 0010 000 = 0x11 0x90
        assert_eq!(asc[0], 0x11);
        assert_eq!(asc[1], 0x90);
    }

    #[test]
    fn aac_audio_specific_config_44100_mono() {
        let asc = build_aac_audio_specific_config(44_100, 1);
        assert_eq!(asc.len(), 2);
        // AAC-LC=2 (00010), freqIdx=4 (0100), channels=1 (0001), GASpec=000
        // 00010 0100 0001 000 = 0x12 0x08
        assert_eq!(asc[0], 0x12);
        assert_eq!(asc[1], 0x08);
    }

    #[test]
    fn mask_stream_key_hides_key() {
        let url = "rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx";
        let masked = mask_stream_key(url);
        assert_eq!(masked, "rtmp://a.rtmp.youtube.com/live2/<redacted>");
        assert!(!masked.contains("xxxx"));
    }

    #[test]
    fn mask_stream_key_no_slash() {
        let masked = mask_stream_key("no-slash-at-all");
        assert_eq!(masked, "<redacted>");
    }

    #[test]
    fn convert_annexb_sps_pps_not_in_video_data() {
        // Regression test: SPS/PPS NALUs must NOT appear in the AVCC video_data
        // field — they belong only in the AVC sequence header.
        let mut annexb = Vec::new();
        // SPS
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1F]);
        // PPS
        annexb.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]);
        // IDR slice
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x11, 0x22]);

        let result = convert_annexb_to_avcc(&annexb);

        // SPS/PPS should be extracted.
        assert_eq!(result.sps_list.len(), 1);
        assert_eq!(result.pps_list.len(), 1);

        // video_data should contain only the IDR NAL, not SPS/PPS.
        // Verify no NAL in video_data has type 7 (SPS) or 8 (PPS).
        let avcc = &result.video_data;
        let mut offset = 0;
        while offset + 4 <= avcc.len() {
            let len = u32::from_be_bytes([
                avcc[offset],
                avcc[offset + 1],
                avcc[offset + 2],
                avcc[offset + 3],
            ]) as usize;
            offset += 4;
            assert!(offset + len <= avcc.len(), "AVCC data truncated");
            let nal_type = avcc[offset] & H264_NAL_TYPE_MASK;
            assert_ne!(nal_type, H264_NAL_SPS, "SPS should not be in video_data");
            assert_ne!(nal_type, H264_NAL_PPS, "PPS should not be in video_data");
            offset += len;
        }
    }
}
