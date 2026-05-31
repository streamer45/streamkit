// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::rtmp_client::{
    AudioFormat as RtmpAudioFormat, AudioFrame as RtmpAudioFrame, AvcPacketType, AvcSequenceHeader,
    RtmpConnectionState, RtmpPublishClientConnection, RtmpTimestamp, RtmpTimestampDelta, RtmpUrl,
    VideoCodec as RtmpVideoCodec, VideoFrame as RtmpVideoFrame, VideoFrameType,
};
use async_trait::async_trait;
use opentelemetry::KeyValue;
use schemars::JsonSchema;
use serde::Deserialize;
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RtmpPublishConfig {
    /// RTMP server URL.
    ///
    /// Supports `rtmp://` and `rtmps://` (TLS) schemes.
    /// Can include the stream key in the path, or use the separate
    /// `stream_key` / `stream_key_env` fields.
    ///
    /// Examples:
    /// - `rtmp://a.rtmp.youtube.com/live2` (key via `stream_key` or `stream_key_env`)
    /// - `rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx` (key inline)
    /// - `rtmps://live.twitch.tv/app/live_xxxx`
    pub url: String,

    /// Stream key appended to the URL path.
    ///
    /// Optional — if omitted, the URL is used as-is (for URLs that
    /// already include the key).  Ignored when `stream_key_env` is set.
    #[serde(default)]
    pub stream_key: Option<String>,

    /// Environment variable name containing the stream key.
    ///
    /// Read at node startup.  Takes precedence over `stream_key`.
    /// The name is fully user-controlled, so multiple RTMP output nodes
    /// can each reference different variables.
    ///
    /// Example: `"SKIT_RTMP_STREAM_KEY"` → reads `$SKIT_RTMP_STREAM_KEY`.
    #[serde(default)]
    pub stream_key_env: Option<String>,

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

pub struct RtmpPublishNode {
    config: RtmpPublishConfig,
}

impl RtmpPublishNode {
    pub const fn new(config: RtmpPublishConfig) -> Self {
        Self { config }
    }
}

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
        vec![]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();

        state_helpers::emit_initializing(&context.state_tx, &node_name);

        validate_aac_config(&self.config).map_err(|e| {
            let msg = format!("Invalid AAC config: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Configuration(msg)
        })?;

        let full_url = resolve_rtmp_url(&self.config).map_err(|e| {
            let msg = format!("RTMP URL resolution failed: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Configuration(msg)
        })?;

        // Log without the stream key (it's effectively a bearer token).
        let masked_url = mask_stream_key(&full_url);
        tracing::info!(%node_name, url = %masked_url, "RtmpPublishNode starting");

        let rtmp_url: RtmpUrl = full_url.parse().map_err(|e| {
            StreamKitError::Configuration(format!(
                "Invalid RTMP URL '{}': {e}",
                mask_stream_key(&full_url)
            ))
        })?;

        tracing::info!(
            %node_name,
            host = %rtmp_url.host, port = rtmp_url.port,
            app = %rtmp_url.app, tls = rtmp_url.tls,
            "Parsed RTMP URL"
        );

        let mut stream = connect(&rtmp_url).await.map_err(|e| {
            let msg = format!("Failed to connect to RTMP server: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(%node_name, "TCP connection established");

        let mut connection = RtmpPublishClientConnection::new(rtmp_url);

        drive_handshake(&mut connection, &mut stream, &node_name).await.map_err(|e| {
            let msg = format!("RTMP handshake failed: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(%node_name, "RTMP connection in Publishing state");

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut video_rx = context.take_input("video")?;
        let mut audio_rx = context.take_input("audio")?;

        let meter = opentelemetry::global::meter("streamkit");
        let packet_counter = meter.u64_counter("rtmp_publish.packets").build();
        let video_labels =
            [KeyValue::new("node", node_name.clone()), KeyValue::new("track", "video")];
        let audio_labels =
            [KeyValue::new("node", node_name.clone()), KeyValue::new("track", "audio")];
        let mut stats = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        let mut audio_seq_header_sent = false;
        let mut video_packet_count: u64 = 0;
        let mut audio_packet_count: u64 = 0;
        let mut tcp_read_buf = vec![0u8; 8192];

        // Per-track timestamp rebase: aligns audio and video tracks
        // that arrive through different pipeline paths at different
        // wall-clock times (same pattern as the WebM muxer).
        let mut ts_state = RtmpTimestampState::new();

        tracing::info!(%node_name, "Entering RTMP publishing loop");

        let result: Result<(), StreamKitError> = async {
            loop {
                // Biased select: TCP read first so server ACKs/pings
                // are drained before sending more media.
                tokio::select! {
                    biased;

                    read_result = tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        stream.read(&mut tcp_read_buf),
                    ) => {
                        let Ok(read_result) = read_result else {
                            continue;
                        };
                        match read_result {
                            Ok(0) => {
                                tracing::warn!(%node_name, "RTMP server closed connection");
                                break;
                            }
                            Ok(n) => {
                                if let Err(e) = connection.feed_recv_buf(&tcp_read_buf[..n]) {
                                    tracing::warn!(%node_name, error = %e, "Error feeding RTMP recv buffer");
                                }
                                if drain_events(&mut connection, &node_name) {
                                    tracing::info!(%node_name, "Breaking loop: peer disconnected");
                                    break;
                                }
                                flush_send_buf(&mut connection, &mut stream, &mut tcp_read_buf, &node_name).await?;
                            }
                            Err(e) => {
                                tracing::warn!(%node_name, error = %e, "TCP read error");
                                break;
                            }
                        }
                    }

                    maybe_pkt = video_rx.recv() => {
                        let Some(pkt) = maybe_pkt else {
                            tracing::info!(%node_name, "Video input channel closed");
                            break;
                        };
                        if connection.state() != RtmpConnectionState::Publishing {
                            tracing::warn!(%node_name, state = %connection.state(), "Connection no longer publishing, exiting");
                            break;
                        }
                        let timestamp_ms = ts_state.stamp(&pkt, Track::Video, &node_name);
                        if let Err(e) = process_video_packet(
                            &pkt, &mut connection, timestamp_ms,
                            &packet_counter, &video_labels,
                            &mut stats, &mut video_packet_count, &node_name,
                        ) {
                            tracing::warn!(%node_name, error = %e, "Error processing video packet");
                            stats.errored();
                        }
                        flush_send_buf(&mut connection, &mut stream, &mut tcp_read_buf, &node_name).await?;
                    }

                    maybe_pkt = audio_rx.recv() => {
                        let Some(pkt) = maybe_pkt else {
                            tracing::info!(%node_name, "Audio input channel closed");
                            break;
                        };
                        if connection.state() != RtmpConnectionState::Publishing {
                            tracing::warn!(%node_name, state = %connection.state(), "Connection no longer publishing, exiting");
                            break;
                        }
                        let timestamp_ms = ts_state.stamp(&pkt, Track::Audio, &node_name);
                        if let Err(e) = process_audio_packet(
                            &pkt, &mut connection, &mut audio_seq_header_sent,
                            timestamp_ms,
                            self.config.sample_rate, self.config.channels,
                            &packet_counter, &audio_labels,
                            &mut stats, &mut audio_packet_count, &node_name,
                        ) {
                            tracing::warn!(%node_name, error = %e, "Error processing audio packet");
                            stats.errored();
                        }
                        flush_send_buf(&mut connection, &mut stream, &mut tcp_read_buf, &node_name).await?;
                    }

                    Some(control_msg) = context.control_rx.recv() => {
                        if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                            tracing::info!(%node_name, "Received shutdown signal");
                            break;
                        }
                    }
                }

                stats.maybe_send();
            }
            Ok(())
        }
        .await;

        tracing::info!(%node_name, video_packets = video_packet_count, audio_packets = audio_packet_count, "RTMP publishing finished");

        let _ = stream.shutdown().await;

        match result {
            Ok(()) => {
                state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
                Ok(())
            },
            Err(e) => {
                state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
                Err(e)
            },
        }
    }
}

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

    /// Non-blocking read that returns `WouldBlock` when no data is available.
    ///
    /// For plain TCP this calls `TcpStream::try_read`, a direct syscall that
    /// bypasses the tokio reactor.  For TLS there is no synchronous decrypt
    /// path, so this always returns `WouldBlock` — the biased main select
    /// loop handles TLS ACK draining instead.
    fn try_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.try_read(buf),
            Self::Tls(_) => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
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

    async fn shutdown(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => tokio::io::AsyncWriteExt::shutdown(s).await,
            Self::Tls(s) => tokio::io::AsyncWriteExt::shutdown(s).await,
        }
    }
}

/// Mask the stream-key portion of an RTMP URL for safe logging.
///
/// If the URL path has two or more segments (e.g. `/app/stream_key`),
/// the last segment is replaced with `<redacted>`.  If the path has
/// only one segment (e.g. `/app` — no key embedded), the URL is
/// returned as-is so the app name remains visible in logs.
fn mask_stream_key(url: &str) -> String {
    // Find the start of the path portion (after ://host[:port]).
    let path_start = url
        .find("://")
        .and_then(|scheme_end| url[scheme_end + 3..].find('/').map(|p| scheme_end + 3 + p));

    path_start.map_or_else(
        || "<redacted>".to_string(),
        |start| {
            let path = &url[start..];
            // rfind('/') always succeeds (at least the leading `/`).
            // If > 0 there is a second segment to redact.
            match path.rfind('/') {
                Some(last) if last > 0 => format!("{}/<redacted>", &url[..start + last]),
                _ => url.to_string(),
            }
        },
    )
}

/// Valid AAC sampling frequencies (ISO 14496-3 Table 1.18).
const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Validate AAC-related config fields at startup so we fail fast with a
/// clear error instead of producing a corrupt AudioSpecificConfig at runtime.
fn validate_aac_config(config: &RtmpPublishConfig) -> Result<(), String> {
    if config.channels == 0 || config.channels > 7 {
        return Err(format!(
            "channels must be 1..=7 (AAC channelConfiguration is 4 bits), got {}",
            config.channels
        ));
    }
    if !AAC_SAMPLE_RATES.contains(&config.sample_rate) {
        return Err(format!(
            "sample_rate {} is not a standard AAC sampling frequency; \
             valid values: {:?}",
            config.sample_rate, AAC_SAMPLE_RATES
        ));
    }
    Ok(())
}

/// Resolve the final RTMP URL from config fields.
///
/// Priority:
/// 1. `stream_key_env` — read the key from the named environment variable.
/// 2. `stream_key` — use the literal value.
/// 3. Neither set — use `url` as-is (key already embedded).
///
/// The resolved key is appended to the base URL separated by `/`.
fn resolve_rtmp_url(config: &RtmpPublishConfig) -> Result<String, String> {
    resolve_rtmp_url_with_env(config, |name| std::env::var(name))
}

/// Inner implementation that accepts an env-var resolver, allowing tests to
/// avoid `std::env::set_var` (which is unsound in multi-threaded processes
/// since Rust 1.83).
fn resolve_rtmp_url_with_env<F>(config: &RtmpPublishConfig, env_var: F) -> Result<String, String>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    let key = if let Some(ref env_name) = config.stream_key_env {
        let val = env_var(env_name).map_err(|e| {
            format!("stream_key_env references '{env_name}' but the variable is not set: {e}")
        })?;
        if val.is_empty() {
            return Err(format!(
                "stream_key_env references '{env_name}' but the variable is empty"
            ));
        }
        Some(val)
    } else {
        config.stream_key.clone()
    };

    match key {
        Some(k) if !k.is_empty() => Ok(format!("{}/{}", config.url.trim_end_matches('/'), k)),
        _ => Ok(config.url.clone()),
    }
}

/// Connect to the RTMP server, using TLS if the URL scheme is `rtmps://`.
async fn connect(url: &RtmpUrl) -> Result<RtmpStream, String> {
    let addr = format!("{}:{}", url.host, url.port);
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(10), TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("TCP connect to {addr} timed out after 10s"))?
        .map_err(|e| format!("TCP connect to {addr} failed: {e}"))?;
    tcp.set_nodelay(true).map_err(|e| format!("Failed to set TCP_NODELAY: {e}"))?;

    if url.tls {
        use rustls_platform_verifier::BuilderVerifierExt;

        let config = rustls::ClientConfig::builder()
            .with_platform_verifier()
            .map_err(|e| format!("Failed to build TLS config with platform verifier: {e}"))?
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
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
        if connection.state() == RtmpConnectionState::Disconnecting {
            return Err("RTMP server rejected the connection".to_string());
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

/// Flush the RTMP connection's send buffer to the TCP stream (no ACK drain).
///
/// Used during the handshake phase where ACK window overflow is not a concern
/// because the handshake loop already reads server data between flushes.
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
    stream.flush().await?;
    Ok(())
}

/// Flush the RTMP connection's send buffer to the TCP stream.
///
/// After flushing, performs a non-blocking drain of any pending server data
/// (ACK messages, pings, etc.) via `try_read` (a direct non-blocking
/// syscall that works for plain TCP).  For TLS streams `try_read` returns
/// `WouldBlock` immediately because there is no synchronous decryption
/// path — the biased main `select!` loop handles TLS ACK draining instead
/// by always checking the TCP read arm first.
async fn flush_send_buf(
    connection: &mut RtmpPublishClientConnection,
    stream: &mut RtmpStream,
    tcp_read_buf: &mut [u8],
    node_name: &str,
) -> Result<(), StreamKitError> {
    // Write all pending outbound data.
    while !connection.send_buf().is_empty() {
        let buf = connection.send_buf();
        stream
            .write_all(buf)
            .await
            .map_err(|e| StreamKitError::Runtime(format!("RTMP send failed: {e}")))?;
        let len = buf.len();
        connection.advance_send_buf(len);
    }
    // Explicit flush to ensure TLS buffered data is sent immediately.
    stream.flush().await.map_err(|e| StreamKitError::Runtime(format!("RTMP flush failed: {e}")))?;

    // Non-blocking drain: `try_read` does a direct non-blocking syscall
    // (bypasses the tokio reactor) so it returns data that is already
    // sitting in the OS receive buffer.  This catches ACKs that arrived
    // while we were writing.  For TLS, `try_read` returns `WouldBlock`
    // and the biased main loop handles draining instead.
    loop {
        match stream.try_read(tcp_read_buf) {
            Ok(0) => {
                return Err(StreamKitError::Runtime("RTMP server closed connection".to_string()));
            },
            Ok(n) => {
                if let Err(e) = connection.feed_recv_buf(&tcp_read_buf[..n]) {
                    tracing::warn!(%node_name, error = %e, "Error feeding RTMP recv buffer (flush drain)");
                }
                if drain_events(connection, node_name) {
                    return Err(StreamKitError::Runtime(
                        "RTMP server disconnected during flush drain".to_string(),
                    ));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available right now — done draining.
                break;
            },
            Err(e) => {
                return Err(StreamKitError::Runtime(format!(
                    "RTMP read failed during flush drain: {e}"
                )));
            },
        }
    }

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
            super::rtmp_client::RtmpConnectionEvent::DisconnectedByPeer { reason } => {
                tracing::warn!(%node_name, %reason, "RTMP server disconnected");
                disconnected = true;
            },
            super::rtmp_client::RtmpConnectionEvent::StateChanged(state) => {
                tracing::info!(%node_name, %state, "RTMP state changed");
            },
        }
    }
    disconnected
}

/// Backward timestamp jump threshold (ms).  Jumps larger than this trigger
/// a rebase offset reset.  Typically caused by the compositor calibrating
/// its running clock to a newly-arrived remote MoQ input.
const BACKWARD_JUMP_THRESHOLD_MS: u32 = 500;

/// Identifies the media track for timestamp rebase bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Track {
    Video,
    Audio,
}

/// Per-track rebase state for a single media track.
struct TrackTimestamp {
    /// Offset (in ms) added to source timestamps so the track starts at
    /// the current global RTMP position when it first produces output.
    rebase_offset_ms: Option<i64>,
    /// Last RTMP timestamp emitted for this track (for monotonicity).
    last_ms: Option<u32>,
}

impl TrackTimestamp {
    const fn new() -> Self {
        Self { rebase_offset_ms: None, last_ms: None }
    }
}

/// Manages RTMP timestamps for audio and video tracks.
///
/// Source timestamps (from `PacketMetadata::timestamp_us`) are synchronized
/// because mic and camera are captured in the same browser epoch.  However,
/// audio and video arrive through different pipeline paths that may start at
/// different wall-clock times (e.g. the compositor generates early video
/// frames before MoQ input arrives, while audio waits for the opus→AAC
/// chain).
///
/// To align the tracks we apply the same per-track rebase pattern used by
/// the WebM muxer: each track's first frame computes an offset so its RTMP
/// timestamp starts at the current global position.  Subsequent frames
/// preserve the source-timestamp cadence.  Large backward jumps (compositor
/// calibration) trigger an offset reset so the track re-aligns.
struct RtmpTimestampState {
    video: TrackTimestamp,
    audio: TrackTimestamp,
    /// The highest RTMP timestamp written across both tracks (ms).
    global_last_ms: u32,
}

impl RtmpTimestampState {
    const fn new() -> Self {
        Self { video: TrackTimestamp::new(), audio: TrackTimestamp::new(), global_last_ms: 0 }
    }

    /// Compute the RTMP timestamp (u32 ms) for a packet, applying per-track
    /// rebase and monotonicity enforcement.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // RTMP timestamps are u32 ms; wrapping after ~49 days is acceptable.
    // Sign loss is guarded by `.max(0)` before each cast.
    fn stamp(&mut self, packet: &Packet, track: Track, node_name: &str) -> u32 {
        let timestamp_us = match packet {
            // In practice this node only receives Binary packets (encoded
            // H.264 / AAC), but the Video variant is included for
            // completeness since the type system allows it.
            Packet::Binary { metadata, .. }
            | Packet::Video(streamkit_core::types::VideoFrame { metadata, .. }) => {
                metadata.as_ref().and_then(|m| m.timestamp_us)
            },
            _ => None,
        };

        let pkt_ms = timestamp_us.map_or(0i64, |us| i64::try_from(us / 1_000).unwrap_or(i64::MAX));

        let ts = match track {
            Track::Video => &mut self.video,
            Track::Audio => &mut self.audio,
        };

        // First frame for this track: compute rebase offset so the track
        // starts at the current global position.
        let is_new_offset = ts.rebase_offset_ms.is_none();
        let offset =
            *ts.rebase_offset_ms.get_or_insert_with(|| i64::from(self.global_last_ms) - pkt_ms);
        if is_new_offset {
            tracing::info!(
                %node_name,
                track = ?track,
                offset,
                pkt_ms,
                global_last_ms = self.global_last_ms,
                "RTMP timestamp rebase initialized"
            );
        }

        let mut rtmp_ms = pkt_ms.saturating_add(offset).max(0) as u32;

        // Handle large backward jumps — typically caused by the compositor
        // calibrating its running clock to a remote MoQ input.  Reset the
        // rebase offset so the track re-aligns with the global position
        // (same strategy as the WebM muxer).
        if let Some(last) = ts.last_ms {
            if rtmp_ms < last {
                let gap_ms = last - rtmp_ms;
                if gap_ms > BACKWARD_JUMP_THRESHOLD_MS {
                    let new_offset = i64::from(self.global_last_ms) - pkt_ms;
                    tracing::info!(
                        %node_name,
                        track = ?track,
                        gap_ms,
                        old_offset = offset,
                        new_offset,
                        "RTMP timestamp rebase reset (backward jump)"
                    );
                    ts.rebase_offset_ms = Some(new_offset);
                    rtmp_ms = pkt_ms.saturating_add(new_offset).max(0) as u32;
                }
                // Enforce monotonicity for remaining small gaps / jitter.
                if rtmp_ms <= last {
                    rtmp_ms = last.saturating_add(1);
                }
            }
        }

        ts.last_ms = Some(rtmp_ms);
        if rtmp_ms > self.global_last_ms {
            self.global_last_ms = rtmp_ms;
        }

        rtmp_ms
    }
}

/// Process one encoded video packet and send it via RTMP.
///
/// Converts H.264 Annex B to AVCC format, extracts SPS/PPS on keyframes
/// to send as an AVC sequence header, then sends the video frame.
///
/// `timestamp_ms` is the rebased RTMP timestamp computed by the caller
/// via `RtmpTimestampState::stamp`, ensuring audio and video share a
/// common time base derived from source timestamps.
#[allow(clippy::too_many_arguments)] // Packet-processing context (connection, counters, stats) is passed individually; bundling into a struct is a future cleanup.
fn process_video_packet(
    packet: &Packet,
    connection: &mut RtmpPublishClientConnection,
    timestamp_ms: u32,
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

        connection.send_video(&seq_frame).map_err(|e| {
            StreamKitError::Runtime(format!("Failed to send AVC sequence header: {e}"))
        })?;

        tracing::debug!(%node_name, %timestamp_ms, "Sent AVC sequence header");
    }

    // Send the actual video data (AVCC-formatted), excluding SPS/PPS NALUs
    // which are already conveyed in the sequence header above.
    // Guard: if an access unit contained only SPS/PPS (no slice NALUs),
    // video_data will be empty — skip the NalUnit frame to avoid sending
    // a zero-length payload that some RTMP servers reject.
    if !conv.video_data.is_empty() {
        let frame = RtmpVideoFrame {
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
            composition_timestamp_offset: RtmpTimestampDelta::ZERO,
            frame_type: if keyframe {
                VideoFrameType::KeyFrame
            } else {
                VideoFrameType::InterFrame
            },
            codec: RtmpVideoCodec::Avc,
            avc_packet_type: Some(AvcPacketType::NalUnit),
            data: conv.video_data,
        };

        connection
            .send_video(&frame)
            .map_err(|e| StreamKitError::Runtime(format!("Failed to send video frame: {e}")))?;
    }

    *packet_count += 1;
    counter.add(1, labels);
    stats.sent();

    // `% N == 0` instead of `.is_multiple_of(N)` for MSRV < 1.85 compat.
    #[allow(clippy::manual_is_multiple_of)]
    if *packet_count <= 5 || *packet_count % 100 == 0 {
        tracing::debug!(%node_name, packet = *packet_count, %timestamp_ms, %keyframe, "Sent video");
    }

    Ok(())
}

/// Process one encoded audio packet and send it via RTMP.
///
/// On the first audio packet, sends an AAC `AudioSpecificConfig` as the
/// RTMP sequence header.  Subsequent packets are sent as raw AAC frames.
///
/// `timestamp_ms` is the rebased RTMP timestamp computed by the caller
/// via `RtmpTimestampState::stamp`, ensuring audio and video share a
/// common time base derived from source timestamps.
#[allow(clippy::too_many_arguments)] // Packet-processing context (connection, counters, stats) is passed individually; bundling into a struct is a future cleanup.
fn process_audio_packet(
    packet: &Packet,
    connection: &mut RtmpPublishClientConnection,
    seq_header_sent: &mut bool,
    timestamp_ms: u32,
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

        connection.send_audio(&seq_frame).map_err(|e| {
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
        .send_audio(&frame)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to send audio frame: {e}")))?;

    *packet_count += 1;
    counter.add(1, labels);
    stats.sent();

    // `% N == 0` instead of `.is_multiple_of(N)` for MSRV < 1.85 compat.
    #[allow(clippy::manual_is_multiple_of)]
    if *packet_count <= 5 || *packet_count % 200 == 0 {
        tracing::debug!(%node_name, packet = *packet_count, %timestamp_ms, "Sent audio");
    }

    Ok(())
}

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
///
/// **Known limitation**: the 3-byte `00 00 01` pattern can theoretically
/// appear inside NAL payload data (e.g. quantized coefficient blocks).
/// Spec-compliant encoders insert emulation-prevention bytes (`00 00 03`)
/// to avoid this ambiguity, so OpenH264 output is safe.  If this node
/// ever receives data from an external encoder that omits prevention
/// bytes, frames could be mis-split.  This mirrors the Annex B parser in
/// `containers/mp4.rs` — extracting a shared `h264_utils` module is
/// tracked as follow-up work.
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
///
/// # Panics
///
/// Never — `sample_rate` and `channels` are validated at node startup by
/// [`validate_aac_config`].  If an unrecognized rate somehow reaches here
/// the index defaults to 3 (48 kHz) and a warning is logged.
fn build_aac_audio_specific_config(sample_rate: u32, channels: u8) -> Vec<u8> {
    // The array has 13 entries (indices 0..=12), so the position always
    // fits in a u8.  Fallback to index 3 (48 kHz) if not found — callers
    // are expected to validate beforehand, but we avoid panicking here.
    let freq_index: u8 = AAC_SAMPLE_RATES.iter().position(|&r| r == sample_rate).map_or_else(
        || {
            tracing::warn!(sample_rate, "Unrecognized AAC sample rate, defaulting to 48 kHz index");
            3
        },
        |i| {
            // Safe: AAC_SAMPLE_RATES has 13 entries, index ≤ 12.
            // unwrap_or(3) is unreachable but avoids clippy::expect_used.
            u8::try_from(i).unwrap_or(3)
        },
    );

    // AAC-LC object type = 2
    let object_type: u8 = 2;

    // Pack: 5 bits objectType | 4 bits freqIndex | 4 bits channels | 3 bits zeros
    let byte0 = (object_type << 3) | (freq_index >> 1);
    let byte1 = (freq_index << 7) | (channels << 3);

    vec![byte0, byte1]
}

pub fn register_rtmp_nodes(registry: &mut NodeRegistry) {
    let default_node = RtmpPublishNode::new(RtmpPublishConfig {
        url: String::new(),
        stream_key: None,
        stream_key_env: None,
        sample_rate: default_sample_rate(),
        channels: default_channels(),
    });

    register_static_node!(
        registry,
        "transport::rtmp::publish",
        |params| {
            let config = config_helpers::parse_config_required(params)?;
            Ok(Box::new(RtmpPublishNode::new(config)))
        },
        RtmpPublishConfig,
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        ["transport", "rtmp"],
        "Publishes encoded H.264 video and AAC audio to an RTMP endpoint. \
         Accepts Annex B H.264 on the 'video' pin and raw AAC frames on the 'audio' pin, \
         converting to the RTMP/FLV wire format. Supports both RTMP and RTMPS (TLS).",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use streamkit_core::types::PacketMetadata;

    // Note: env-var tests use unique variable names per test (prefixed
    // `_SK_TEST_RTMP_*`) so they are safe to run in parallel without
    // `#[serial]`.  If a test is added that shares a variable name,
    // add the `serial_test` crate.

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
    fn validate_aac_config_valid() {
        let cfg = RtmpPublishConfig {
            url: String::new(),
            stream_key: None,
            stream_key_env: None,
            sample_rate: 48_000,
            channels: 2,
        };
        assert!(validate_aac_config(&cfg).is_ok());
    }

    #[test]
    fn validate_aac_config_channels_zero_rejected() {
        let cfg = RtmpPublishConfig {
            url: String::new(),
            stream_key: None,
            stream_key_env: None,
            sample_rate: 48_000,
            channels: 0,
        };
        let err = validate_aac_config(&cfg).unwrap_err();
        assert!(err.contains("channels"), "{err}");
    }

    #[test]
    fn validate_aac_config_channels_overflow_rejected() {
        let cfg = RtmpPublishConfig {
            url: String::new(),
            stream_key: None,
            stream_key_env: None,
            sample_rate: 48_000,
            channels: 8,
        };
        let err = validate_aac_config(&cfg).unwrap_err();
        assert!(err.contains("channels"), "{err}");
    }

    #[test]
    fn validate_aac_config_invalid_sample_rate_rejected() {
        let cfg = RtmpPublishConfig {
            url: String::new(),
            stream_key: None,
            stream_key_env: None,
            sample_rate: 22_000,
            channels: 2,
        };
        let err = validate_aac_config(&cfg).unwrap_err();
        assert!(err.contains("sample_rate"), "{err}");
    }

    #[test]
    fn mask_stream_key_hides_key() {
        let url = "rtmp://a.rtmp.youtube.com/live2/xxxx-xxxx-xxxx-xxxx";
        let masked = mask_stream_key(url);
        assert_eq!(masked, "rtmp://a.rtmp.youtube.com/live2/<redacted>");
        assert!(!masked.contains("xxxx"));
    }

    #[test]
    fn mask_stream_key_bare_url_not_over_redacted() {
        // When no stream key is embedded, the app name should remain visible.
        let url = "rtmp://a.rtmp.youtube.com/live2";
        let masked = mask_stream_key(url);
        assert_eq!(masked, url, "bare URL without key should not be redacted");
    }

    #[test]
    fn mask_stream_key_no_scheme() {
        let masked = mask_stream_key("no-scheme-at-all");
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

    fn make_config(url: &str, key: Option<&str>, key_env: Option<&str>) -> RtmpPublishConfig {
        RtmpPublishConfig {
            url: url.to_string(),
            stream_key: key.map(String::from),
            stream_key_env: key_env.map(String::from),
            sample_rate: default_sample_rate(),
            channels: default_channels(),
        }
    }

    #[test]
    fn resolve_url_no_key_uses_url_as_is() {
        let cfg = make_config("rtmp://host/app/inline_key", None, None);
        assert_eq!(resolve_rtmp_url(&cfg).unwrap(), "rtmp://host/app/inline_key");
    }

    #[test]
    fn resolve_url_with_stream_key() {
        let cfg = make_config("rtmp://a.rtmp.youtube.com/live2", Some("my-key"), None);
        assert_eq!(resolve_rtmp_url(&cfg).unwrap(), "rtmp://a.rtmp.youtube.com/live2/my-key");
    }

    #[test]
    fn resolve_url_strips_trailing_slash() {
        let cfg = make_config("rtmp://host/app/", Some("key"), None);
        assert_eq!(resolve_rtmp_url(&cfg).unwrap(), "rtmp://host/app/key");
    }

    #[test]
    fn resolve_url_env_takes_precedence() {
        let cfg = make_config("rtmp://host/app", Some("literal-key"), Some("MY_KEY"));
        let result = resolve_rtmp_url_with_env(&cfg, |name| {
            assert_eq!(name, "MY_KEY");
            Ok("env-key".to_string())
        })
        .unwrap();
        assert_eq!(result, "rtmp://host/app/env-key");
    }

    #[test]
    fn resolve_url_env_var_set() {
        let cfg = make_config("rtmp://host/app", None, Some("MY_KEY"));
        let result = resolve_rtmp_url_with_env(&cfg, |_| Ok("secret123".to_string())).unwrap();
        assert_eq!(result, "rtmp://host/app/secret123");
    }

    #[test]
    fn resolve_url_env_var_not_set() {
        let cfg = make_config("rtmp://host/app", None, Some("MISSING"));
        let err =
            resolve_rtmp_url_with_env(&cfg, |_| Err(std::env::VarError::NotPresent)).unwrap_err();
        assert!(err.contains("not set"), "error should mention 'not set': {err}");
    }

    #[test]
    fn resolve_url_env_var_empty() {
        let cfg = make_config("rtmp://host/app", None, Some("MY_KEY"));
        let err = resolve_rtmp_url_with_env(&cfg, |_| Ok(String::new())).unwrap_err();
        assert!(err.contains("empty"), "error should mention 'empty': {err}");
    }

    /// Helper: build a `Packet::Binary` with a given `timestamp_us`.
    fn make_packet(timestamp_us: Option<u64>) -> Packet {
        Packet::Binary {
            data: bytes::Bytes::from_static(&[0]),
            metadata: timestamp_us.map(|ts| PacketMetadata {
                timestamp_us: Some(ts),
                duration_us: None,
                sequence: None,
                keyframe: None,
            }),
            content_type: None,
        }
    }

    #[test]
    fn rebase_first_video_starts_at_zero() {
        let mut state = RtmpTimestampState::new();
        let pkt = make_packet(Some(0));
        let ts = state.stamp(&pkt, Track::Video, "test");
        assert_eq!(ts, 0);
    }

    #[test]
    fn rebase_video_preserves_cadence() {
        let mut state = RtmpTimestampState::new();
        let ts0 = state.stamp(&make_packet(Some(0)), Track::Video, "test");
        let ts1 = state.stamp(&make_packet(Some(33_000)), Track::Video, "test");
        let ts2 = state.stamp(&make_packet(Some(66_000)), Track::Video, "test");
        assert_eq!(ts0, 0);
        assert_eq!(ts1, 33);
        assert_eq!(ts2, 66);
    }

    #[test]
    fn rebase_late_audio_aligns_to_video() {
        // Video has been running for 3 seconds.
        let mut state = RtmpTimestampState::new();
        for i in 0..90 {
            // 30fps video for 3 seconds (90 frames).
            state.stamp(&make_packet(Some(i * 33_333)), Track::Video, "test");
        }
        // 89 * 33_333us = 2_966_637us → global_last_ms ≈ 2966.
        // Audio arrives with source_ts=0 (MoQ normalized).  It should
        // start at the current global position.
        let audio_ts0 = state.stamp(&make_packet(Some(0)), Track::Audio, "test");
        let audio_ts1 = state.stamp(&make_packet(Some(20_000)), Track::Audio, "test");
        // Audio should start near video's current position (~2966ms).
        assert!(
            (2900..=3100).contains(&audio_ts0),
            "audio should start near video position, got {audio_ts0}"
        );
        // Cadence preserved: 20ms between audio frames.
        assert_eq!(audio_ts1 - audio_ts0, 20);
    }

    #[test]
    fn rebase_backward_jump_resets_offset() {
        // Simulate compositor calibration: video starts at running clock
        // ts=0, then after calibration jumps backward to MoQ origin.
        let mut state = RtmpTimestampState::new();

        // Pre-calibration: compositor running clock 0..~4000ms.
        for i in 0..120 {
            state.stamp(&make_packet(Some(i * 33_333)), Track::Video, "test");
        }
        // 119 * 33_333us = 3_966_627us → global_last_ms ≈ 3966.
        let global_before = state.global_last_ms;

        // Post-calibration: compositor jumps to MoQ timestamp ~100ms
        // (a large backward jump).
        let ts = state.stamp(&make_packet(Some(100_000)), Track::Video, "test");
        // Should have reset and re-aligned near the global position.
        assert!(
            ts >= global_before,
            "after rebase reset, ts ({ts}) should be >= global_before ({global_before})"
        );
    }

    #[test]
    fn rebase_monotonicity_enforced() {
        let mut state = RtmpTimestampState::new();
        // First packet at 0ms to establish the offset.
        let _ = state.stamp(&make_packet(Some(0)), Track::Video, "test");
        let ts0 = state.stamp(&make_packet(Some(100_000)), Track::Video, "test");
        // Small backward jitter (< 500ms threshold).
        let ts1 = state.stamp(&make_packet(Some(99_000)), Track::Video, "test");
        assert!(ts1 > ts0, "timestamps must be monotonically increasing: ts0={ts0}, ts1={ts1}");
    }

    #[test]
    fn convert_annexb_sps_pps_only_yields_empty_video_data() {
        // An access unit containing only SPS+PPS (no slice NALUs) should
        // produce empty video_data so the caller can skip the NalUnit frame.
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1F]); // SPS
        annexb.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]); // PPS

        let result = convert_annexb_to_avcc(&annexb);

        assert_eq!(result.sps_list.len(), 1);
        assert_eq!(result.pps_list.len(), 1);
        assert!(
            result.video_data.is_empty(),
            "video_data should be empty for SPS/PPS-only access units"
        );
    }

    #[test]
    fn stamp_with_no_metadata_defaults_to_zero() {
        let mut state = RtmpTimestampState::new();
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(&[0]),
            metadata: None,
            content_type: None,
        };
        let ts = state.stamp(&pkt, Track::Video, "test");
        assert_eq!(ts, 0);
    }

    #[test]
    fn config_default_sample_rate() {
        assert_eq!(default_sample_rate(), 48_000);
    }

    #[test]
    fn config_default_channels() {
        assert_eq!(default_channels(), 2);
    }

    #[test]
    fn convert_annexb_sei_and_aud_in_video_data() {
        let mut annexb = Vec::new();
        // SEI NAL (type 6)
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let sei = [0x06, 0x05, 0x04, 0x03];
        annexb.extend_from_slice(&sei);
        // AUD NAL (type 9)
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let aud = [0x09, 0x10];
        annexb.extend_from_slice(&aud);
        // IDR slice (type 5)
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let idr = [0x65, 0x88, 0x84];
        annexb.extend_from_slice(&idr);

        let result = convert_annexb_to_avcc(&annexb);

        assert!(result.sps_list.is_empty());
        assert!(result.pps_list.is_empty());

        let avcc = &result.video_data;
        let mut nal_types = Vec::new();
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
            nal_types.push(avcc[offset] & H264_NAL_TYPE_MASK);
            offset += len;
        }

        assert!(nal_types.contains(&6), "SEI should be in video_data");
        assert!(nal_types.contains(&9), "AUD should be in video_data");
        assert!(nal_types.contains(&5), "IDR should be in video_data");
    }

    //
    // Packet-processing + connection-driving tests.
    //
    // The production AMF0 / chunk encoders live in `rtmp_client.rs` and are
    // private to that module, so the helpers below reproduce just enough of
    // the server→client wire format to drive a real
    // `RtmpPublishClientConnection` (sans-IO) to the `Publishing` state from
    // this test module — mirroring `rtmp_client.rs`'s own
    // `drive_to_publishing` helper.

    const TEST_HANDSHAKE_SIZE: usize = 1536;

    fn amf0_string(buf: &mut Vec<u8>, s: &str) {
        buf.push(0x02);
        buf.extend_from_slice(&u16::try_from(s.len()).unwrap().to_be_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    fn amf0_number(buf: &mut Vec<u8>, n: f64) {
        buf.push(0x00);
        buf.extend_from_slice(&n.to_be_bytes());
    }

    fn amf0_null(buf: &mut Vec<u8>) {
        buf.push(0x05);
    }

    /// Append an AMF0 object with `level`/`code`/`description` fields — the
    /// shape RTMP servers use for `onStatus` and `connect` `_result` info.
    fn amf0_status_object(buf: &mut Vec<u8>, code: &str) {
        buf.push(0x03); // object marker
        for (key, val) in [("level", "status"), ("code", code), ("description", "ok")] {
            buf.extend_from_slice(&u16::try_from(key.len()).unwrap().to_be_bytes());
            buf.extend_from_slice(key.as_bytes());
            amf0_string(buf, val);
        }
        buf.extend_from_slice(&[0x00, 0x00, 0x09]); // object-end marker
    }

    /// Encode one server→client RTMP message as a fmt=0 chunk, splitting
    /// payloads larger than 128 bytes into fmt=3 continuation chunks (the
    /// client decoder uses the 128-byte default until told otherwise).
    fn server_chunk(csid: u8, msg_type_id: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
        const CHUNK: usize = 128;
        let mut out = Vec::new();
        out.push(csid); // fmt=0 (top two bits zero) + csid in the low six bits
        out.extend_from_slice(&[0, 0, 0]); // timestamp
        out.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes()[1..4]);
        out.push(msg_type_id);
        out.extend_from_slice(&stream_id.to_le_bytes());

        let first = payload.len().min(CHUNK);
        out.extend_from_slice(&payload[..first]);
        let mut offset = first;
        while offset < payload.len() {
            out.push(0xC0 | csid); // fmt=3 continuation header
            let end = (offset + CHUNK).min(payload.len());
            out.extend_from_slice(&payload[offset..end]);
            offset = end;
        }
        out
    }

    /// The 1 + 1536 + 1536 handshake bytes a server sends (S0/S1/S2).  The
    /// client's handshake ignores S1/S2 content, so filler bytes suffice.
    fn handshake_s0s1s2() -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + TEST_HANDSHAKE_SIZE * 2);
        out.push(0x03); // S0
        out.extend_from_slice(&[0xBB; TEST_HANDSHAKE_SIZE]); // S1
        out.extend_from_slice(&[0xCC; TEST_HANDSHAKE_SIZE]); // S2
        out
    }

    /// Post-handshake server messages that take a publish client all the way
    /// to `Publishing`: window-ack + set-bandwidth, the `connect` `_result`,
    /// the `createStream` `_result` (stream id 1) and the
    /// `NetStream.Publish.Start` `onStatus`.
    fn server_publish_sequence() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&server_chunk(2, 5, 0, &2_500_000u32.to_be_bytes()));
        let mut set_bw = 2_500_000u32.to_be_bytes().to_vec();
        set_bw.push(2); // limit type = dynamic
        out.extend_from_slice(&server_chunk(2, 6, 0, &set_bw));

        let mut connect_result = Vec::new();
        amf0_string(&mut connect_result, "_result");
        amf0_number(&mut connect_result, 1.0);
        amf0_null(&mut connect_result);
        amf0_status_object(&mut connect_result, "NetConnection.Connect.Success");
        out.extend_from_slice(&server_chunk(3, 20, 0, &connect_result));

        let mut create_stream_result = Vec::new();
        amf0_string(&mut create_stream_result, "_result");
        amf0_number(&mut create_stream_result, 2.0);
        amf0_null(&mut create_stream_result);
        amf0_number(&mut create_stream_result, 1.0); // media stream id
        out.extend_from_slice(&server_chunk(3, 20, 0, &create_stream_result));

        let mut on_status = Vec::new();
        amf0_string(&mut on_status, "onStatus");
        amf0_number(&mut on_status, 0.0);
        amf0_null(&mut on_status);
        amf0_status_object(&mut on_status, "NetStream.Publish.Start");
        out.extend_from_slice(&server_chunk(4, 20, 1, &on_status));

        out
    }

    fn test_connection() -> RtmpPublishClientConnection {
        let url: RtmpUrl = "rtmp://x.rtmp.example.com/live2/stream-key".parse().unwrap();
        RtmpPublishClientConnection::new(url)
    }

    /// Drive a fresh connection (in-process, no sockets) to `Publishing` and
    /// return it with its send buffer and event queue drained.
    fn drive_to_publishing() -> RtmpPublishClientConnection {
        let mut conn = test_connection();
        assert_eq!(conn.state(), RtmpConnectionState::Handshaking);
        conn.advance_send_buf(conn.send_buf().len()); // drain C0+C1

        conn.feed_recv_buf(&handshake_s0s1s2()).unwrap();
        assert_eq!(conn.state(), RtmpConnectionState::Connecting);
        conn.advance_send_buf(conn.send_buf().len());

        conn.feed_recv_buf(&server_publish_sequence()).unwrap();
        assert_eq!(conn.state(), RtmpConnectionState::Publishing);
        conn.advance_send_buf(conn.send_buf().len());
        while conn.next_event().is_some() {}

        conn
    }

    fn keyframe_annexb() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1F]); // SPS
        data.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]); // PPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84]); // IDR
        data
    }

    fn idr_only_annexb() -> Vec<u8> {
        vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84] // IDR, no SPS/PPS
    }

    fn interframe_annexb() -> Vec<u8> {
        vec![0x00, 0x00, 0x00, 0x01, 0x41, 0x9A, 0x12] // non-IDR slice (type 1)
    }

    fn binary_packet(data: &[u8], keyframe: Option<bool>) -> Packet {
        Packet::Binary {
            data: bytes::Bytes::copy_from_slice(data),
            metadata: Some(PacketMetadata {
                timestamp_us: Some(0),
                duration_us: None,
                sequence: None,
                keyframe,
            }),
            content_type: None,
        }
    }

    fn test_counter() -> opentelemetry::metrics::Counter<u64> {
        opentelemetry::global::meter("test").u64_counter("test").build()
    }

    fn test_stats() -> NodeStatsTracker {
        NodeStatsTracker::new("test".to_string(), None)
    }

    #[allow(clippy::too_many_arguments)]
    fn send_video(
        conn: &mut RtmpPublishClientConnection,
        pkt: &Packet,
        count: &mut u64,
    ) -> Result<(), StreamKitError> {
        let counter = test_counter();
        let mut stats = test_stats();
        let labels = [KeyValue::new("track", "video")];
        process_video_packet(pkt, conn, 0, &counter, &labels, &mut stats, count, "test")
    }

    #[test]
    fn drive_to_publishing_reaches_publishing_state() {
        let conn = drive_to_publishing();
        assert_eq!(conn.state(), RtmpConnectionState::Publishing);
        assert!(conn.send_buf().is_empty());
    }

    #[test]
    fn process_video_keyframe_sends_seq_header_and_nal() {
        let mut conn = drive_to_publishing();
        let mut count = 0u64;
        let pkt = binary_packet(&keyframe_annexb(), Some(true));
        send_video(&mut conn, &pkt, &mut count).unwrap();

        assert_eq!(count, 1);
        let with_params = conn.send_buf().len();
        assert!(with_params > 0, "keyframe should enqueue RTMP data");

        // An IDR-only keyframe (no SPS/PPS) skips the AVC sequence header, so
        // it must enqueue strictly fewer bytes than the SPS+PPS keyframe.
        let mut conn2 = drive_to_publishing();
        let mut count2 = 0u64;
        let pkt2 = binary_packet(&idr_only_annexb(), Some(true));
        send_video(&mut conn2, &pkt2, &mut count2).unwrap();
        let without_params = conn2.send_buf().len();

        assert_eq!(count2, 1);
        assert!(
            with_params > without_params,
            "keyframe with SPS/PPS ({with_params}) should send more than IDR-only \
             ({without_params})"
        );
    }

    #[test]
    fn process_video_interframe_sends_without_seq_header() {
        // A keyframe primes the connection; the following interframe should
        // still enqueue data but without an AVC sequence header.
        let mut conn = drive_to_publishing();
        let mut count = 0u64;
        send_video(&mut conn, &binary_packet(&keyframe_annexb(), Some(true)), &mut count).unwrap();
        conn.advance_send_buf(conn.send_buf().len());

        send_video(&mut conn, &binary_packet(&interframe_annexb(), Some(false)), &mut count)
            .unwrap();
        assert_eq!(count, 2);
        assert!(!conn.send_buf().is_empty(), "interframe should enqueue a NAL unit");
    }

    #[test]
    fn process_video_keyframe_without_slices_sends_only_seq_header() {
        // An access unit with SPS+PPS but no slice NALUs has empty video_data,
        // so only the sequence header is sent (no zero-length NalUnit frame).
        let mut conn = drive_to_publishing();
        let mut count = 0u64;
        let mut params_only = Vec::new();
        params_only.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1F]); // SPS
        params_only.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]); // PPS

        send_video(&mut conn, &binary_packet(&params_only, Some(true)), &mut count).unwrap();
        assert_eq!(count, 1);
        assert!(!conn.send_buf().is_empty(), "sequence header should still be sent");
    }

    #[test]
    fn process_video_ignores_non_binary_packet() {
        let mut conn = drive_to_publishing();
        let mut count = 0u64;
        let pkt = Packet::Text(std::sync::Arc::from("not video"));
        send_video(&mut conn, &pkt, &mut count).unwrap();
        assert_eq!(count, 0);
        assert!(conn.send_buf().is_empty(), "non-binary packet should not enqueue data");
    }

    #[test]
    fn process_audio_first_packet_sends_seq_header_then_frame() {
        let mut conn = drive_to_publishing();
        let counter = test_counter();
        let mut stats = test_stats();
        let labels = [KeyValue::new("track", "audio")];
        let mut count = 0u64;
        let mut seq_sent = false;
        let pkt = binary_packet(&[0xAA; 20], None);

        process_audio_packet(
            &pkt,
            &mut conn,
            &mut seq_sent,
            0,
            48_000,
            2,
            &counter,
            &labels,
            &mut stats,
            &mut count,
            "test",
        )
        .unwrap();

        assert!(seq_sent, "AAC sequence header flag should flip on the first packet");
        assert_eq!(count, 1);
        let first_len = conn.send_buf().len();
        conn.advance_send_buf(first_len);

        // Subsequent packet reuses the cached header → only the raw frame is
        // sent, so it enqueues strictly fewer bytes than the first packet.
        process_audio_packet(
            &pkt,
            &mut conn,
            &mut seq_sent,
            20,
            48_000,
            2,
            &counter,
            &labels,
            &mut stats,
            &mut count,
            "test",
        )
        .unwrap();
        let second_len = conn.send_buf().len();

        assert_eq!(count, 2);
        assert!(
            first_len > second_len,
            "first audio packet ({first_len}) carries the sequence header so it should send more \
             than a subsequent frame ({second_len})"
        );
    }

    #[test]
    fn process_audio_mono_config_succeeds() {
        let mut conn = drive_to_publishing();
        let counter = test_counter();
        let mut stats = test_stats();
        let labels = [KeyValue::new("track", "audio")];
        let mut count = 0u64;
        let mut seq_sent = false;
        let pkt = binary_packet(&[0x01, 0x02, 0x03, 0x04], None);

        process_audio_packet(
            &pkt,
            &mut conn,
            &mut seq_sent,
            0,
            44_100,
            1,
            &counter,
            &labels,
            &mut stats,
            &mut count,
            "test",
        )
        .unwrap();

        assert!(seq_sent);
        assert_eq!(count, 1);
        assert!(!conn.send_buf().is_empty());
    }

    #[test]
    fn process_audio_ignores_non_binary_packet() {
        let mut conn = drive_to_publishing();
        let counter = test_counter();
        let mut stats = test_stats();
        let labels = [KeyValue::new("track", "audio")];
        let mut count = 0u64;
        let mut seq_sent = false;
        let pkt = Packet::Text(std::sync::Arc::from("not audio"));

        process_audio_packet(
            &pkt,
            &mut conn,
            &mut seq_sent,
            0,
            48_000,
            2,
            &counter,
            &labels,
            &mut stats,
            &mut count,
            "test",
        )
        .unwrap();

        assert!(!seq_sent, "non-binary packet must not emit the sequence header");
        assert_eq!(count, 0);
        assert!(conn.send_buf().is_empty());
    }

    #[test]
    fn drain_events_returns_false_for_state_changes_only() {
        let mut conn = test_connection();
        conn.advance_send_buf(conn.send_buf().len());
        conn.feed_recv_buf(&handshake_s0s1s2()).unwrap();
        // Handshake completion queues StateChanged events but no disconnect.
        assert!(!drain_events(&mut conn, "test"));
        assert!(conn.next_event().is_none(), "events should be drained");
    }

    #[test]
    fn drain_events_detects_peer_disconnect() {
        let mut conn = drive_to_publishing();
        let mut on_status = Vec::new();
        amf0_string(&mut on_status, "onStatus");
        amf0_number(&mut on_status, 0.0);
        amf0_null(&mut on_status);
        amf0_status_object(&mut on_status, "NetStream.Publish.Failed");
        conn.feed_recv_buf(&server_chunk(4, 20, 1, &on_status)).unwrap();

        assert!(drain_events(&mut conn, "test"), "a failed onStatus should report a disconnect");
        assert_eq!(conn.state(), RtmpConnectionState::Disconnecting);
    }

    async fn fake_server(listener: tokio::net::TcpListener) {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut c0c1 = vec![0u8; 1 + TEST_HANDSHAKE_SIZE];
        sock.read_exact(&mut c0c1).await.unwrap();

        let mut s0s1s2 = vec![0x03];
        s0s1s2.extend_from_slice(&[0xBB; TEST_HANDSHAKE_SIZE]); // S1
        s0s1s2.extend_from_slice(&c0c1[1..]); // S2 echoes C1
        sock.write_all(&s0s1s2).await.unwrap();

        let mut buf = vec![0u8; 8192];
        let _ = sock.read(&mut buf).await.unwrap(); // C2 + connect sequence
        sock.write_all(&server_publish_sequence()).await.unwrap();

        // Keep draining so the client's later writes never block.
        loop {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {},
            }
        }
    }

    #[tokio::test]
    async fn drive_handshake_reaches_publishing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(fake_server(listener));

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut stream = RtmpStream::Plain(tcp);
        let url: RtmpUrl = "rtmp://x.rtmp.example.com/live2/key".parse().unwrap();
        let mut conn = RtmpPublishClientConnection::new(url);

        drive_handshake(&mut conn, &mut stream, "test").await.unwrap();
        assert_eq!(conn.state(), RtmpConnectionState::Publishing);

        stream.shutdown().await.unwrap();
        drop(stream);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn flush_send_buf_raw_drains_buffer_to_stream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let _ = sock.read_to_end(&mut received).await;
            received.len()
        });

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut stream = RtmpStream::Plain(tcp);
        let mut conn = test_connection();
        let queued = conn.send_buf().len(); // C0 + C1
        assert_eq!(queued, 1 + TEST_HANDSHAKE_SIZE);

        flush_send_buf_raw(&mut conn, &mut stream).await.unwrap();
        assert!(conn.send_buf().is_empty(), "send buffer should be fully drained after flush");

        stream.shutdown().await.unwrap();
        drop(stream);
        assert_eq!(server.await.unwrap(), queued);
    }

    #[tokio::test]
    async fn flush_send_buf_writes_and_returns_when_no_server_data() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            // Drain without ever replying so the client's non-blocking drain
            // sees `WouldBlock` and returns cleanly.
            loop {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {},
                }
            }
        });

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut stream = RtmpStream::Plain(tcp);
        let mut conn = test_connection();
        let mut read_buf = vec![0u8; 8192];

        flush_send_buf(&mut conn, &mut stream, &mut read_buf, "test").await.unwrap();
        assert!(conn.send_buf().is_empty());

        stream.shutdown().await.unwrap();
        drop(stream);
        server.await.unwrap();
    }
}
