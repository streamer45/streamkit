// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketMetadata, PacketType,
    VideoCodec,
};
use streamkit_core::{
    state_helpers, timing::MediaClock, InputPin, NodeContext, NodeRegistry, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use webm::mux::{
    AudioCodecId, AudioTrack, SegmentBuilder, SegmentMode, VideoCodecId, VideoTrack, Writer,
};

// --- WebM Constants ---

/// Default chunk size for flushing buffers
const DEFAULT_CHUNK_SIZE: usize = 65536;
/// Default audio frame duration when metadata is missing (20ms Opus frame).
const DEFAULT_FRAME_DURATION_US: u64 = 20_000;
/// Default video frame duration when metadata is missing (~30 fps).
const DEFAULT_VIDEO_FRAME_DURATION_US: u64 = 33_333;
/// Opus codec lookahead at 48kHz in samples (typical libopus default).
///
/// This is written to the OpusHead `pre_skip` field so decoders can trim encoder delay.
const OPUS_PRESKIP_SAMPLES: u16 = 312;

fn opus_head_codec_private(sample_rate: u32, channels: u32) -> Result<[u8; 19], StreamKitError> {
    let channels_u8: u8 = channels.try_into().map_err(|_| {
        StreamKitError::Runtime(format!(
            "Invalid channel count for Opus/WebM: {channels} (must fit in u8)"
        ))
    })?;

    if !(channels_u8 == 1 || channels_u8 == 2) {
        return Err(StreamKitError::Runtime(format!(
            "Unsupported channel count for OpusHead mapping family 0: {channels}"
        )));
    }

    // OpusHead structure (little-endian fields):
    // https://wiki.xiph.org/OggOpus#ID_Header
    //
    // While this is commonly seen in Ogg, WebM/Matroska uses the same byte layout in CodecPrivate.
    let mut head = [0u8; 19];
    head[0..8].copy_from_slice(b"OpusHead");
    head[8] = 1; // version
    head[9] = channels_u8;
    head[10..12].copy_from_slice(&OPUS_PRESKIP_SAMPLES.to_le_bytes());
    head[12..16].copy_from_slice(&sample_rate.to_le_bytes());
    head[16..18].copy_from_slice(&0i16.to_le_bytes()); // output gain
    head[18] = 0; // channel mapping family 0 (mono/stereo)

    Ok(head)
}

// --- WebM Muxer ---

/// A shared, thread-safe buffer that wraps a Cursor for WebM writing.
/// This allows us to stream out data as it's written while still supporting Seek.
///
/// Supports two buffering modes:
///
/// - **Streaming (non-seek)**: Bytes are drained on every `take_data()` call.
///   This mode is intended for `Writer::new_non_seek` and avoids copying.
/// - **Seek window**: Keeps a configurable window of recent data for WebM library seeks
///   and trims old data that has already been sent.
///
/// The node selects the appropriate mode based on `WebMStreamingMode`.
#[derive(Clone)]
struct SharedPacketBuffer {
    cursor: Arc<Mutex<Cursor<Vec<u8>>>>,
    last_sent_pos: Arc<Mutex<usize>>,
    base_offset: Arc<Mutex<usize>>,
    window_size: usize,
}

impl SharedPacketBuffer {
    /// Create a new buffer with a sliding window size.
    /// window_size: Maximum bytes to keep in memory (default 1MB for ~6 seconds at 128kbps)
    fn new_with_window(window_size: usize) -> Self {
        Self {
            cursor: Arc::new(Mutex::new(Cursor::new(Vec::new()))),
            last_sent_pos: Arc::new(Mutex::new(0)),
            base_offset: Arc::new(Mutex::new(0)),
            window_size,
        }
    }

    /// Create a non-seek streaming buffer.
    ///
    /// This is designed for `Writer::new_non_seek` in live streaming mode. Since the writer
    /// does not seek/backpatch, we can drain bytes out by moving the underlying `Vec<u8>`
    /// (no copy) and reset the cursor to keep memory bounded.
    fn new_streaming() -> Self {
        // window_size=0 is treated as "drain everything on take_data"
        Self::new_with_window(0)
    }

    /// Takes any new data written since the last call, and trims old data beyond the window.
    /// This allows the WebM library to seek backwards within the window while preventing
    /// unbounded memory growth for long streams.
    fn take_data(&self) -> Option<Bytes> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        let mut buffer_guard = self.cursor.lock().expect("SharedPacketBuffer mutex poisoned");
        let vec = buffer_guard.get_mut();

        #[allow(clippy::expect_used)]
        let mut last_sent_guard = self.last_sent_pos.lock().expect("last_sent_pos mutex poisoned");
        #[allow(clippy::expect_used)]
        let mut base_offset_guard = self.base_offset.lock().expect("base_offset mutex poisoned");

        let last_sent = *last_sent_guard;
        let current_len = vec.len();
        let base = *base_offset_guard;

        let result = if current_len > last_sent {
            if self.window_size == 0 {
                // Streaming mode (non-seek): drain everything written so far without copying.
                //
                // This avoids two major sources of allocation churn in DHAT profiles:
                // - copying out incremental slices on every flush
                // - repeatedly trimming a sliding window with `split_off` (copies the window)
                let data_vec = std::mem::take(vec);
                // Advance base_offset so Seek::Start can clamp consistently if it ever happens.
                *base_offset_guard = base + current_len;
                *last_sent_guard = 0;
                buffer_guard.set_position(0);
                Some(Bytes::from(data_vec))
            } else if self.window_size == usize::MAX && last_sent == 0 {
                // File mode: nothing has been sent yet, so move the entire buffer out.
                // The segment is finalized before this is called, so no more writes/seeks occur.
                let data_vec = std::mem::take(vec);
                *base_offset_guard = base + current_len;
                *last_sent_guard = 0;
                buffer_guard.set_position(0);
                Some(Bytes::from(data_vec))
            } else {
                // Seek-window mode: copy incremental bytes while retaining a backwards-seek window.
                let new_data = Bytes::copy_from_slice(&vec[last_sent..current_len]);
                *last_sent_guard = current_len;

                // Trim old data if buffer exceeds window size.
                if current_len > self.window_size {
                    let trim_amount = current_len - self.window_size;
                    // Keep the last window_size bytes.
                    let remaining = vec.split_off(trim_amount);
                    *vec = remaining;
                    // Update base offset to reflect discarded data.
                    *base_offset_guard = base + trim_amount;
                    // Adjust last_sent and cursor position.
                    *last_sent_guard = self.window_size;
                    buffer_guard.set_position(self.window_size as u64);

                    tracing::debug!(
                        "Trimmed {} bytes from WebM buffer, new base_offset: {}",
                        trim_amount,
                        *base_offset_guard
                    );
                }

                Some(new_data)
            }
        } else {
            None
        };

        drop(base_offset_guard);
        drop(last_sent_guard);
        drop(buffer_guard);
        result
    }
}

impl Write for SharedPacketBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        self.cursor.lock().expect("SharedPacketBuffer mutex poisoned").write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        self.cursor.lock().expect("SharedPacketBuffer mutex poisoned").flush()
    }
}

impl Seek for SharedPacketBuffer {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        // When seeking, we need to adjust for the base_offset since we may have
        // trimmed old data from the beginning of the buffer
        #[allow(clippy::expect_used)]
        let base_guard = self.base_offset.lock().expect("base_offset mutex poisoned");
        let base = *base_guard;
        drop(base_guard);

        #[allow(clippy::expect_used)]
        let mut cursor_guard = self.cursor.lock().expect("SharedPacketBuffer mutex poisoned");

        // Adjust seek position by base_offset for absolute seeks
        let adjusted_pos = match pos {
            SeekFrom::Start(offset) => {
                // Absolute position from start - subtract base_offset
                if offset >= base as u64 {
                    SeekFrom::Start(offset - base as u64)
                } else {
                    // Seeking before our window - this is an error but we'll seek to start
                    tracing::warn!(
                        "WebM seek to {} before base_offset {}, clamping to start",
                        offset,
                        base
                    );
                    SeekFrom::Start(0)
                }
            },
            // Current and End are relative, no adjustment needed
            SeekFrom::Current(offset) => SeekFrom::Current(offset),
            SeekFrom::End(offset) => SeekFrom::End(offset),
        };

        let result = cursor_guard.seek(adjusted_pos)?;
        drop(cursor_guard);

        // Return the absolute position (including base_offset)
        Ok(result + base as u64)
    }
}

#[derive(Deserialize, Debug, Default, Clone, Copy, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WebMStreamingMode {
    /// Live streaming mode - optimized for real-time streaming, no duration/seeking info (default)
    #[default]
    Live,
    /// File mode - includes full duration and seeking information
    File,
}

impl WebMStreamingMode {
    const fn as_segment_mode(self) -> SegmentMode {
        match self {
            Self::Live => SegmentMode::Live,
            Self::File => SegmentMode::File,
        }
    }
}

#[derive(Deserialize, Debug, JsonSchema)]
#[serde(default)]
pub struct WebMMuxerConfig {
    /// Audio sample rate in Hz (used when an audio input is connected)
    pub sample_rate: u32,
    /// Number of audio channels (1 for mono, 2 for stereo)
    pub channels: u32,
    /// Video width in pixels (required when a video input is connected)
    pub video_width: u32,
    /// Video height in pixels (required when a video input is connected)
    pub video_height: u32,
    /// The number of bytes to buffer before flushing to the output. Defaults to 65536.
    pub chunk_size: usize,
    /// Streaming mode: "live" for real-time streaming (no duration), "file" for complete files
    /// with duration (default)
    pub streaming_mode: WebMStreamingMode,
}

impl Default for WebMMuxerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            video_width: 0,
            video_height: 0,
            chunk_size: DEFAULT_CHUNK_SIZE,
            streaming_mode: WebMStreamingMode::default(),
        }
    }
}

/// Track handles resolved during segment setup.
struct MuxTracks {
    audio: Option<AudioTrack>,
    video: Option<VideoTrack>,
}

/// Builds the MIME content-type string based on which tracks are present.
const fn webm_content_type(has_audio: bool, has_video: bool) -> &'static str {
    match (has_audio, has_video) {
        (true, true) => "video/webm; codecs=\"vp9,opus\"",
        (false, true) => "video/webm; codecs=\"vp9\"",
        (true, false) => "audio/webm; codecs=\"opus\"",
        // Shouldn't happen - at least one track is required - but provide a safe fallback.
        (false, false) => "video/webm",
    }
}

/// A node that muxes encoded Opus audio and/or VP9 video packets into a WebM container stream.
///
/// The node exposes two input pins:
/// - `"audio"` - accepts `EncodedAudio(Opus)` packets (optional)
/// - `"video"` - accepts `EncodedVideo(VP9)` packets (optional)
///
/// At least one input must be connected. When both are connected, audio and video frames are
/// interleaved by arrival order as required by the WebM/Matroska container.
pub struct WebMMuxerNode {
    config: WebMMuxerConfig,
}

impl WebMMuxerNode {
    pub const fn new(config: WebMMuxerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl ProcessorNode for WebMMuxerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![
            InputPin {
                name: "audio".to_string(),
                accepts_types: vec![PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                })],
                cardinality: PinCardinality::One,
            },
            InputPin {
                name: "video".to_string(),
                accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                    codec: VideoCodec::Vp9,
                    bitstream_format: None,
                    codec_private: None,
                    profile: None,
                    level: None,
                })],
                cardinality: PinCardinality::One,
            },
        ]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        // Determine content type from config: if video dimensions are specified
        // we assume video will be present, otherwise audio-only.
        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        Some(webm_content_type(true, has_video).to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("WebMMuxerNode starting");

        // Take input pins - they may or may not be connected.
        let mut audio_rx = context.inputs.remove("audio");
        let mut video_rx = context.inputs.remove("video");

        let has_audio = audio_rx.is_some();
        let has_video = video_rx.is_some();

        if !has_audio && !has_video {
            let err_msg = "WebMMuxerNode requires at least one input (audio or video)".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        tracing::info!("WebMMuxerNode tracks: audio={}, video={}", has_audio, has_video);

        let content_type_str: Cow<'static, str> =
            Cow::Borrowed(webm_content_type(has_audio, has_video));

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut packet_count = 0u64;
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // In Live mode we use a non-seek writer, so we can drain bytes out without keeping
        // any history (zero-copy streaming). In File mode we must keep the whole buffer
        // because we only emit bytes once the segment is finalized.
        let shared_buffer = match self.config.streaming_mode {
            WebMStreamingMode::Live => SharedPacketBuffer::new_streaming(),
            WebMStreamingMode::File => SharedPacketBuffer::new_with_window(usize::MAX),
        };

        // Create writer with shared buffer.
        //
        // Important: In `Live` mode we must avoid any backwards seeking/backpatching while
        // bytes are being streamed to the client. Using a non-seek writer forces libwebm to
        // produce a forward-only stream (unknown sizes/no cues), which is required for MSE
        // consumers like Firefox that are less tolerant of inconsistent metadata during
        // progressive append.
        let writer = match self.config.streaming_mode {
            WebMStreamingMode::Live => Writer::new_non_seek(shared_buffer.clone()),
            WebMStreamingMode::File => Writer::new(shared_buffer.clone()),
        };

        // Create WebM segment builder
        let builder = SegmentBuilder::new(writer).map_err(|e| {
            let err_msg = format!("Failed to create SegmentBuilder: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            StreamKitError::Runtime(err_msg)
        })?;

        let builder =
            builder.set_mode(self.config.streaming_mode.as_segment_mode()).map_err(|e| {
                let err_msg = format!("Failed to set streaming mode: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                StreamKitError::Runtime(err_msg)
            })?;

        // -- Add tracks conditionally --

        let mut tracks = MuxTracks { audio: None, video: None };

        // Video track is added first so that the segment header lists it prominently
        // for players that inspect the first track.
        let builder = if has_video {
            let width = self.config.video_width;
            let height = self.config.video_height;
            if width == 0 || height == 0 {
                let err_msg = "WebMMuxerNode: video input connected but \
                               video_width/video_height not configured (both must be > 0)"
                    .to_string();
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Runtime(err_msg));
            }

            let (builder, vt) =
                builder.add_video_track(width, height, VideoCodecId::VP9, None).map_err(|e| {
                    let err_msg = format!("Failed to add video track: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    StreamKitError::Runtime(err_msg)
                })?;
            tracks.video = Some(vt);
            tracing::info!("Added VP9 video track ({}x{})", width, height);
            builder
        } else {
            builder
        };

        let builder = if has_audio {
            let opus_private =
                opus_head_codec_private(self.config.sample_rate, self.config.channels).map_err(
                    |e| {
                        let err_msg = format!("Failed to build OpusHead codec private: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        StreamKitError::Runtime(err_msg)
                    },
                )?;

            let (builder, at) = builder
                .add_audio_track(
                    self.config.sample_rate,
                    self.config.channels,
                    AudioCodecId::Opus,
                    None,
                )
                .map_err(|e| {
                    let err_msg = format!("Failed to add audio track: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    StreamKitError::Runtime(err_msg)
                })?;

            let builder = builder.set_codec_private(at, &opus_private).map_err(|e| {
                let err_msg = format!("Failed to set Opus codec private: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                StreamKitError::Runtime(err_msg)
            })?;

            tracks.audio = Some(at);
            tracing::info!(
                "Added Opus audio track ({}Hz, {} ch)",
                self.config.sample_rate,
                self.config.channels
            );
            builder
        } else {
            builder
        };

        let mut segment = builder.build();

        let mut audio_clock = MediaClock::new(0);
        let mut video_clock = MediaClock::new(0);
        let mut header_sent = false;

        // Monotonic timestamp guard: libwebm requires that timestamps across all tracks
        // are non-decreasing. We track the last written timestamp and clamp if needed.
        let mut last_written_ns: u64 = 0;

        tracing::info!("WebM segment built, entering receive loop to process incoming packets");

        // -- Receive loop: multiplex audio + video inputs --

        let mut audio_done = !has_audio;
        let mut video_done = !has_video;

        while !audio_done || !video_done {
            enum MuxFrame {
                Audio(Bytes, Option<PacketMetadata>),
                Video(Bytes, Option<PacketMetadata>),
                AudioClosed,
                VideoClosed,
            }

            let frame = if audio_done {
                // Only video remains
                match video_rx.as_mut() {
                    Some(rx) => match rx.recv().await {
                        Some(Packet::Binary { data, metadata, .. }) => {
                            MuxFrame::Video(data, metadata)
                        },
                        Some(_) => continue,
                        None => MuxFrame::VideoClosed,
                    },
                    None => break,
                }
            } else if video_done {
                // Only audio remains
                match audio_rx.as_mut() {
                    Some(rx) => match rx.recv().await {
                        Some(Packet::Binary { data, metadata, .. }) => {
                            MuxFrame::Audio(data, metadata)
                        },
                        Some(_) => continue,
                        None => MuxFrame::AudioClosed,
                    },
                    None => break,
                }
            } else {
                // Both active - use select to receive from whichever is ready first
                let audio_rx_ref = audio_rx.as_mut();
                let video_rx_ref = video_rx.as_mut();
                match (audio_rx_ref, video_rx_ref) {
                    (Some(a_rx), Some(v_rx)) => {
                        tokio::select! {
                            biased; // prefer audio first for stable ordering
                            maybe_audio = a_rx.recv() => {
                                match maybe_audio {
                                    Some(Packet::Binary { data, metadata, .. }) => {
                                        MuxFrame::Audio(data, metadata)
                                    },
                                    Some(_) => continue,
                                    None => MuxFrame::AudioClosed,
                                }
                            }
                            maybe_video = v_rx.recv() => {
                                match maybe_video {
                                    Some(Packet::Binary { data, metadata, .. }) => {
                                        MuxFrame::Video(data, metadata)
                                    },
                                    Some(_) => continue,
                                    None => MuxFrame::VideoClosed,
                                }
                            }
                        }
                    },
                    _ => break,
                }
            };

            match frame {
                MuxFrame::AudioClosed => {
                    tracing::info!("WebMMuxerNode audio input closed");
                    audio_done = true;
                },
                MuxFrame::VideoClosed => {
                    tracing::info!("WebMMuxerNode video input closed");
                    video_done = true;
                },
                MuxFrame::Audio(data, metadata) => {
                    let Some(audio_track) = tracks.audio else {
                        continue;
                    };

                    packet_count += 1;
                    stats_tracker.received();

                    let incoming_ts_us = metadata.as_ref().and_then(|m| m.timestamp_us);
                    let incoming_duration_us = metadata
                        .as_ref()
                        .and_then(|m| m.duration_us)
                        .or(Some(DEFAULT_FRAME_DURATION_US));

                    if let Some(ts) = incoming_ts_us {
                        audio_clock.seed_from_timestamp_us(ts);
                    } else if audio_clock.timestamp_us() == 0 {
                        audio_clock.seed_from_timestamp_us(0);
                    }

                    let presentation_ts_us =
                        incoming_ts_us.unwrap_or_else(|| audio_clock.timestamp_us());
                    audio_clock
                        .advance_by_duration_us(incoming_duration_us, DEFAULT_FRAME_DURATION_US);

                    let mut timestamp_ns = presentation_ts_us.saturating_mul(1000);
                    if timestamp_ns < last_written_ns {
                        timestamp_ns = last_written_ns;
                    }

                    // Audio frames are always keyframes
                    if let Err(e) = segment.add_frame(audio_track, &data, timestamp_ns, true) {
                        stats_tracker.errored();
                        stats_tracker.maybe_send();
                        let err_msg = format!("Failed to add audio frame to segment: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }

                    last_written_ns = timestamp_ns;

                    let output_metadata = Some(PacketMetadata {
                        timestamp_us: Some(presentation_ts_us),
                        duration_us: incoming_duration_us,
                        sequence: metadata.as_ref().and_then(|m| m.sequence),
                        keyframe: metadata.as_ref().and_then(|m| m.keyframe),
                    });

                    if flush_output(
                        &mut context,
                        &shared_buffer,
                        &content_type_str,
                        output_metadata,
                        &mut header_sent,
                        &mut stats_tracker,
                        &node_name,
                    )
                    .await?
                    {
                        break;
                    }

                    stats_tracker.maybe_send();
                },
                MuxFrame::Video(data, metadata) => {
                    let Some(video_track) = tracks.video else {
                        continue;
                    };

                    packet_count += 1;
                    stats_tracker.received();

                    let incoming_ts_us = metadata.as_ref().and_then(|m| m.timestamp_us);
                    let incoming_duration_us = metadata
                        .as_ref()
                        .and_then(|m| m.duration_us)
                        .or(Some(DEFAULT_VIDEO_FRAME_DURATION_US));

                    if let Some(ts) = incoming_ts_us {
                        video_clock.seed_from_timestamp_us(ts);
                    } else if video_clock.timestamp_us() == 0 {
                        video_clock.seed_from_timestamp_us(0);
                    }

                    let presentation_ts_us =
                        incoming_ts_us.unwrap_or_else(|| video_clock.timestamp_us());
                    video_clock.advance_by_duration_us(
                        incoming_duration_us,
                        DEFAULT_VIDEO_FRAME_DURATION_US,
                    );

                    let mut timestamp_ns = presentation_ts_us.saturating_mul(1000);
                    if timestamp_ns < last_written_ns {
                        timestamp_ns = last_written_ns;
                    }

                    let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);

                    if let Err(e) = segment.add_frame(video_track, &data, timestamp_ns, is_keyframe)
                    {
                        stats_tracker.errored();
                        stats_tracker.maybe_send();
                        let err_msg = format!("Failed to add video frame to segment: {e}");
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }

                    last_written_ns = timestamp_ns;

                    let output_metadata = Some(PacketMetadata {
                        timestamp_us: Some(presentation_ts_us),
                        duration_us: incoming_duration_us,
                        sequence: metadata.as_ref().and_then(|m| m.sequence),
                        keyframe: Some(is_keyframe),
                    });

                    if flush_output(
                        &mut context,
                        &shared_buffer,
                        &content_type_str,
                        output_metadata,
                        &mut header_sent,
                        &mut stats_tracker,
                        &node_name,
                    )
                    .await?
                    {
                        break;
                    }

                    stats_tracker.maybe_send();
                },
            }
        }

        tracing::info!(
            "WebMMuxerNode input streams closed, processed {} packets total",
            packet_count
        );

        // Finalize the segment
        let _writer = segment.finalize(None).map_err(|_e| {
            let err_msg = "Failed to finalize WebM segment".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            StreamKitError::Runtime(err_msg)
        })?;

        // Flush any remaining data from the buffer
        if let Some(data) = shared_buffer.take_data() {
            tracing::debug!("Writing final data, buffer size: {} bytes", data.len());
            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary {
                        data,
                        content_type: Some(content_type_str.clone()),
                        metadata: None,
                    },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed during final flush");
            } else {
                stats_tracker.sent();
            }
            stats_tracker.force_send();
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("WebMMuxerNode finished");
        Ok(())
    }
}

/// Flushes buffered WebM data to the output sender.
///
/// Returns `Ok(true)` if the output channel is closed (node should stop),
/// `Ok(false)` to continue, or `Err` on fatal errors.
#[allow(clippy::ptr_arg)] // We need Cow::clone() to produce owned Cow for Packet
async fn flush_output(
    context: &mut NodeContext,
    shared_buffer: &SharedPacketBuffer,
    content_type: &Cow<'static, str>,
    output_metadata: Option<PacketMetadata>,
    header_sent: &mut bool,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<bool, StreamKitError> {
    if !*header_sent {
        if let Some(data) = shared_buffer.take_data() {
            tracing::info!("Sending WebM header + first frame ({} bytes)", data.len(),);
            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary {
                        data,
                        content_type: Some(content_type.clone()),
                        metadata: None,
                    },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed, stopping node");
                state_helpers::emit_stopped(&context.state_tx, node_name, "output_closed");
                return Ok(true);
            }
            stats_tracker.sent();
            *header_sent = true;
        }
    }

    // Flush any accumulated bytes after the header has been sent.
    if *header_sent {
        if let Some(data) = shared_buffer.take_data() {
            tracing::trace!("Flushing {} bytes to output", data.len());
            if context
                .output_sender
                .send(
                    "out",
                    Packet::Binary {
                        data,
                        content_type: Some(content_type.clone()),
                        metadata: output_metadata,
                    },
                )
                .await
                .is_err()
            {
                tracing::debug!("Output channel closed, stopping node");
                return Ok(true);
            }
            stats_tracker.sent();
        }
    }

    Ok(false)
}

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the WebM container nodes.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
pub fn register_webm_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "webm")]
    {
        let default_muxer = WebMMuxerNode::new(WebMMuxerConfig::default());
        registry.register_static_with_description(
            "containers::webm::muxer",
            |params| {
                let config = config_helpers::parse_config_with_context(params, "WebMMuxer")?;
                Ok(Box::new(WebMMuxerNode::new(config)))
            },
            serde_json::to_value(schema_for!(WebMMuxerConfig))
                .expect("WebMMuxerConfig schema should serialize to JSON"),
            StaticPins { inputs: default_muxer.input_pins(), outputs: default_muxer.output_pins() },
            vec!["containers".to_string(), "webm".to_string()],
            false,
            "Muxes Opus audio and/or VP9 video into a WebM container. \
             Produces streamable WebM output compatible with web browsers. \
             Supports audio-only, video-only, or combined audio+video muxing.",
        );
    }
}
