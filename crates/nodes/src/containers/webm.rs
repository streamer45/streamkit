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

// ---------------------------------------------------------------------------
// VP9 keyframe dimension parser
// ---------------------------------------------------------------------------

/// Minimal bit reader for parsing VP9 uncompressed headers (MSB-first).
struct BitReader<'a> {
    data: &'a [u8],
    byte_offset: usize,
    bit_offset: u8,
}

impl<'a> BitReader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, byte_offset: 0, bit_offset: 0 }
    }

    /// Read `n` bits (1..=16) as a `u32`, MSB first.
    fn read(&mut self, n: u8) -> Option<u32> {
        let mut value: u32 = 0;
        for _ in 0..n {
            if self.byte_offset >= self.data.len() {
                return None;
            }
            let bit = (self.data[self.byte_offset] >> (7 - self.bit_offset)) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit_offset += 1;
            if self.bit_offset == 8 {
                self.bit_offset = 0;
                self.byte_offset += 1;
            }
        }
        Some(value)
    }
}

/// Parse the display dimensions from a VP9 keyframe's uncompressed header.
///
/// Returns `Some((width, height))` when the data starts with a valid VP9
/// keyframe (profile 0–3).  Returns `None` for non-keyframes, truncated
/// data, or invalid sync codes.
fn parse_vp9_keyframe_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 {
        return None;
    }

    let mut r = BitReader::new(data);

    // frame_marker (2 bits) – must be 0b10
    if r.read(2)? != 2 {
        return None;
    }

    let profile_low = r.read(1)?;
    let profile_high = r.read(1)?;
    let profile = (profile_high << 1) | profile_low;

    if profile > 2 {
        r.read(1)?; // reserved_zero
    }

    // show_existing_frame
    if r.read(1)? != 0 {
        return None;
    }

    // frame_type: 0 = KEY_FRAME
    if r.read(1)? != 0 {
        return None;
    }

    r.read(1)?; // show_frame
    r.read(1)?; // error_resilient_mode

    // frame_sync_code must be 0x49_83_42
    if r.read(8)? != 0x49 || r.read(8)? != 0x83 || r.read(8)? != 0x42 {
        return None;
    }

    // color_config
    if profile >= 2 {
        r.read(1)?; // ten_or_twelve_bit
    }
    let color_space = r.read(3)?;
    if color_space != 7 {
        // not CS_RGB
        r.read(1)?; // color_range
        if profile == 1 || profile == 3 {
            r.read(1)?; // subsampling_x
            r.read(1)?; // subsampling_y
            r.read(1)?; // reserved
        }
    } else if profile == 1 || profile == 3 {
        r.read(1)?; // reserved
    }

    // frame_size: width_minus_1 (16 bits), height_minus_1 (16 bits)
    let w = r.read(16)? + 1;
    let h = r.read(16)? + 1;
    Some((w, h))
}
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

/// Internal state for [`SharedPacketBuffer`], protected by a single mutex
/// to eliminate lock-ordering concerns between cursor, position tracking,
/// and offset bookkeeping.
struct BufferState {
    cursor: Cursor<Vec<u8>>,
    last_sent_pos: usize,
    base_offset: usize,
}

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
    state: Arc<Mutex<BufferState>>,
    window_size: usize,
}

impl SharedPacketBuffer {
    /// Create a new buffer with a sliding window size.
    /// window_size: Maximum bytes to keep in memory (default 1MB for ~6 seconds at 128kbps)
    fn new_with_window(window_size: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(BufferState {
                cursor: Cursor::new(Vec::new()),
                last_sent_pos: 0,
                base_offset: 0,
            })),
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
    #[allow(clippy::significant_drop_tightening)] // Guard must span the entire take-trim-update sequence.
    fn take_data(&self) -> Option<Bytes> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        let mut state = self.state.lock().expect("SharedPacketBuffer mutex poisoned");

        // Read bookkeeping fields first (immutable access only).
        let last_sent = state.last_sent_pos;
        let base = state.base_offset;
        let current_len = state.cursor.get_ref().len();

        if current_len <= last_sent {
            return None;
        }

        if self.window_size == 0 {
            // Streaming mode (non-seek): drain everything written so far without copying.
            //
            // This avoids two major sources of allocation churn in DHAT profiles:
            // - copying out incremental slices on every flush
            // - repeatedly trimming a sliding window with `split_off` (copies the window)
            let data_vec = std::mem::take(state.cursor.get_mut());
            // Advance base_offset so Seek::Start can clamp consistently if it ever happens.
            state.base_offset = base + current_len;
            state.last_sent_pos = 0;
            state.cursor.set_position(0);
            Some(Bytes::from(data_vec))
        } else if self.window_size == usize::MAX && last_sent == 0 {
            // File mode: nothing has been sent yet, so move the entire buffer out.
            // The segment is finalized before this is called, so no more writes/seeks occur.
            let data_vec = std::mem::take(state.cursor.get_mut());
            state.base_offset = base + current_len;
            state.last_sent_pos = 0;
            state.cursor.set_position(0);
            Some(Bytes::from(data_vec))
        } else {
            // Seek-window mode: copy incremental bytes while retaining a backwards-seek window.
            let new_data = Bytes::copy_from_slice(&state.cursor.get_ref()[last_sent..current_len]);
            state.last_sent_pos = current_len;

            // Trim old data if buffer exceeds window size.
            if current_len > self.window_size {
                let trim_amount = current_len - self.window_size;
                // Keep the last window_size bytes.
                {
                    let vec = state.cursor.get_mut();
                    let remaining = vec.split_off(trim_amount);
                    *vec = remaining;
                }
                // Update base offset to reflect discarded data.
                state.base_offset = base + trim_amount;
                // Adjust last_sent and cursor position.
                state.last_sent_pos = self.window_size;
                state.cursor.set_position(self.window_size as u64);

                tracing::debug!(
                    "Trimmed {} bytes from WebM buffer, new base_offset: {}",
                    trim_amount,
                    state.base_offset
                );
            }

            Some(new_data)
        }
    }
}

impl Write for SharedPacketBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        self.state.lock().expect("SharedPacketBuffer mutex poisoned").cursor.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        self.state.lock().expect("SharedPacketBuffer mutex poisoned").cursor.flush()
    }
}

impl Seek for SharedPacketBuffer {
    #[allow(clippy::significant_drop_tightening)] // Guard must span base_offset read + seek + result computation.
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        // Single lock covers both base_offset read and cursor seek, eliminating
        // the lock-ordering concern of the previous triple-mutex design.
        #[allow(clippy::expect_used)]
        let mut state = self.state.lock().expect("SharedPacketBuffer mutex poisoned");
        let base = state.base_offset;

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

        let result = state.cursor.seek(adjusted_pos)?;

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
/// Input pins use generic names (`"in"`, `"in_1"`, …) — the media type carried by each
/// input is detected at runtime from the packet's `content_type` field, **not** from the
/// pin name.  This keeps the node future-proof for additional track types (subtitles,
/// data channels, etc.) without requiring pin-name changes.
///
/// Pin layout (determined by config):
/// - Default (no video dimensions): single pin `"in"` accepting audio **or** video.
/// - With `video_width`/`video_height` > 0: two pins `"in"` + `"in_1"`, each accepting
///   audio or video.  The muxer will auto-detect which track type each pin carries.
///
/// At least one input must be connected. When both are connected, audio and video frames
/// are interleaved by arrival order as required by the WebM/Matroska container.
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
        // Each pin accepts both audio and video — the actual media type is detected
        // at runtime from the packet content_type, not from the pin name.
        let media_types = vec![
            PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Vp9,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
        ];

        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        if has_video {
            // Two generic inputs for audio + video (order is determined at runtime).
            vec![
                InputPin {
                    name: "in".to_string(),
                    accepts_types: media_types.clone(),
                    cardinality: PinCardinality::One,
                },
                InputPin {
                    name: "in_1".to_string(),
                    accepts_types: media_types,
                    cardinality: PinCardinality::One,
                },
            ]
        } else {
            // Single generic input — backward compatible with `needs: encoder_node`.
            vec![InputPin {
                name: "in".to_string(),
                accepts_types: media_types,
                cardinality: PinCardinality::One,
            }]
        }
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        // This static hint is used before the node runs.
        // We can only infer from config: if video dimensions are set, video is
        // present. Audio presence is unknown at this stage, so we conservatively
        // report only what we can confirm — the runtime `run()` method uses the
        // actual connected tracks for the real content-type.
        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        // Without a way to know if audio will be connected, assume audio-only
        // when no video dimensions are configured, and video-only when they are.
        // Mixed audio+video pipelines will get the correct type at runtime.
        let has_audio = !has_video;
        Some(webm_content_type(has_audio, has_video).to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("WebMMuxerNode starting");

        // --- Classify generic inputs using connection-time type metadata ---
        //
        // Inputs use generic pin names ("in", "in_1", …).  The graph builder
        // populates `context.input_types` with the upstream output's
        // [`PacketType`] for each connected pin, so we can determine whether a
        // channel carries audio or video without inspecting any packets.

        if context.inputs.is_empty() {
            let err_msg = "WebMMuxerNode requires at least one input (audio or video)".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        let mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;

        for (pin_name, rx) in context.inputs.drain() {
            let is_video = context.input_types.get(&pin_name).is_some_and(|ty| {
                matches!(ty, PacketType::EncodedVideo(_) | PacketType::RawVideo(_))
            });

            if is_video {
                if video_rx.is_some() {
                    let err_msg = format!(
                        "WebMMuxerNode: multiple video inputs detected (pin '{pin_name}'). \
                         Only one video track is supported."
                    );
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
                tracing::info!(
                    "WebMMuxerNode: pin '{pin_name}' classified as VIDEO (from connection type)"
                );
                video_rx = Some(rx);
            } else {
                if audio_rx.is_some() {
                    let err_msg = format!(
                        "WebMMuxerNode: multiple audio inputs detected (pin '{pin_name}'). \
                         Only one audio track is supported."
                    );
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
                tracing::info!(
                    "WebMMuxerNode: pin '{pin_name}' classified as AUDIO (from connection type)"
                );
                audio_rx = Some(rx);
            }
        }

        let has_audio = audio_rx.is_some();
        let has_video = video_rx.is_some();

        if !has_audio && !has_video {
            let err_msg =
                "WebMMuxerNode: no connected inputs could be classified as audio or video"
                    .to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        state_helpers::emit_running(&context.state_tx, &node_name);

        tracing::info!("WebMMuxerNode tracks: audio={}, video={}", has_audio, has_video);

        let content_type_str: Cow<'static, str> =
            Cow::Borrowed(webm_content_type(has_audio, has_video));

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

        // --- Resolve video dimensions -----------------------------------------
        //
        // When `video_width` / `video_height` are both 0 (the default) and a
        // video input is connected, we auto-detect the dimensions from the
        // first VP9 keyframe.  This avoids requiring the user to manually
        // keep the muxer config in sync with the upstream encoder / compositor.
        //
        // The first video packet is buffered so it can be replayed through the
        // normal receive loop after the segment is built.

        let mut first_video_packet: Option<(Bytes, Option<PacketMetadata>)> = None;

        let (video_width, video_height) = if has_video {
            let mut w = self.config.video_width;
            let mut h = self.config.video_height;

            if w == 0 || h == 0 {
                // Auto-detect: wait for the first video packet and parse its VP9 header.
                tracing::info!(
                    "WebMMuxerNode: video_width/video_height not configured, \
                                auto-detecting from first VP9 keyframe"
                );

                let first = match video_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => None,
                };

                if let Some(Packet::Binary { data, metadata, .. }) = first {
                    if let Some((detected_w, detected_h)) = parse_vp9_keyframe_dimensions(&data) {
                        tracing::info!(
                            "Auto-detected video dimensions: {}x{}",
                            detected_w,
                            detected_h
                        );
                        w = detected_w;
                        h = detected_h;
                    } else {
                        let err_msg = "WebMMuxerNode: failed to parse VP9 keyframe dimensions \
                             from first video packet (is the upstream encoder VP9?)"
                            .to_string();
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }
                    first_video_packet = Some((data, metadata));
                } else {
                    let err_msg =
                        "WebMMuxerNode: video input closed before sending any packets".to_string();
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
            }

            if w == 0 || h == 0 {
                let err_msg = "WebMMuxerNode: video dimensions could not be determined".to_string();
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Runtime(err_msg));
            }
            (w, h)
        } else {
            (0, 0)
        };

        // Video track is added first so that the segment header lists it prominently
        // for players that inspect the first track.
        let builder = if has_video {
            let (builder, vt) = builder
                .add_video_track(video_width, video_height, VideoCodecId::VP9, None)
                .map_err(|e| {
                    let err_msg = format!("Failed to add video track: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    StreamKitError::Runtime(err_msg)
                })?;
            tracks.video = Some(vt);
            tracing::info!("Added VP9 video track ({}x{})", video_width, video_height);
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

        // If we buffered the first video packet for dimension detection, replay
        // it through the normal mux path before entering the receive loop.
        if let Some((data, metadata)) = first_video_packet.take() {
            if let Some(video_track) = tracks.video {
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
                video_clock
                    .advance_by_duration_us(incoming_duration_us, DEFAULT_VIDEO_FRAME_DURATION_US);

                let timestamp_ns = presentation_ts_us.saturating_mul(1000);
                let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(true);

                if let Err(e) = segment.add_frame(video_track, &data, timestamp_ns, is_keyframe) {
                    stats_tracker.errored();
                    stats_tracker.maybe_send();
                    let err_msg = format!("Failed to add first video frame to segment: {e}");
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
                    self.config.streaming_mode,
                )
                .await?
                {
                    video_done = true;
                }

                stats_tracker.maybe_send();
            }
        }

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
                        self.config.streaming_mode,
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
                        self.config.streaming_mode,
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
/// In **Live** mode, bytes are drained incrementally after every frame to keep
/// memory bounded and enable real-time streaming.  In **File** mode the writer
/// may seek backwards to back-patch segment sizes and cues, so we must **not**
/// drain intermediate bytes — the buffer is only flushed once after
/// finalization (handled by the caller).
///
/// Returns `Ok(true)` if the output channel is closed (node should stop),
/// `Ok(false)` to continue, or `Err` on fatal errors.
#[allow(clippy::ptr_arg, clippy::too_many_arguments)]
async fn flush_output(
    context: &mut NodeContext,
    shared_buffer: &SharedPacketBuffer,
    content_type: &Cow<'static, str>,
    output_metadata: Option<PacketMetadata>,
    header_sent: &mut bool,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
    streaming_mode: WebMStreamingMode,
) -> Result<bool, StreamKitError> {
    // In File mode, skip all intermediate flushes.  The buffer will be
    // drained once after `segment.finalize()` so back-patched data is
    // consistent.
    if matches!(streaming_mode, WebMStreamingMode::File) {
        return Ok(false);
    }

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
