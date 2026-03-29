// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::io::{BufWriter, Cursor, Read as _, Seek, SeekFrom, Write};
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

/// Default audio frame duration when metadata is missing (20ms Opus frame).
const DEFAULT_FRAME_DURATION_US: u64 = 20_000;

use crate::video::{
    DEFAULT_VIDEO_FRAME_DURATION_US, VP9_BIT_DEPTH, VP9_CHROMA_SUBSAMPLING, VP9_LEVEL, VP9_PROFILE,
};

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

    /// Read `n` bits (1..=32) as a `u32`, MSB first.
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
/// Build a VP9 CodecPrivate (VPCodecConfigurationRecord) for WebM/Matroska.
///
/// Layout follows the VP Codec ISO Media File Format Binding specification:
/// <https://www.webmproject.org/vp9/mp4/>
///
/// | Byte | Field                         |
/// |------|-------------------------------|
/// |  0   | Profile (8 bits)              |
/// |  1   | Level (8 bits)                |
/// |  2   | bitDepth(4) | chromaSub(3) | fullRange(1) |
/// |  3   | colourPrimaries (8 bits)      |
/// |  4   | transferCharacteristics       |
/// |  5   | matrixCoefficients            |
/// |  6-7 | codecInitializationDataSize   |
const fn vp9_codec_private(
    profile: u8,
    level: u8,
    bit_depth: u8,
    chroma_subsampling: u8,
) -> [u8; 8] {
    [
        profile,
        level,
        (bit_depth << 4) | ((chroma_subsampling & 0x07) << 1), // fullRange = 0
        1,                                                     // colourPrimaries: BT.709
        1,                                                     // transferCharacteristics: BT.709
        1,                                                     // matrixCoefficients: BT.709
        0,
        0, // codecInitializationDataSize = 0
    ]
}

/// Pre-computed VP9 codec-private data (VPCodecConfigurationRecord, 8 bytes)
/// for the default encoder config (profile 0, level 1.0, 8-bit, 4:2:0).
const VP9_CODEC_PRIVATE: [u8; 8] =
    vp9_codec_private(VP9_PROFILE, VP9_LEVEL, VP9_BIT_DEPTH, VP9_CHROMA_SUBSAMPLING);

/// AV1CodecConfigurationRecord for WebM/Matroska (4 bytes).
///
/// Byte layout:
///   [0] marker(1)=1 | version(7)=1        → 0x81
///   [1] seq_profile(3)=0 | seq_level_idx_0(5)=8  → 0x08  (Main profile, level 4.0)
///   [2] seq_tier_0(1)=0 | high_bitdepth(1)=0 | twelve_bit(1)=0 | monochrome(1)=0
///       | chroma_subsampling_x(1)=1 | chroma_subsampling_y(1)=1
///       | chroma_sample_position(2)=0     → 0x0C  (8-bit, 4:2:0)
///   [3] reserved(8)=0                     → 0x00
///
/// Reference: <https://aomediacodec.github.io/av1-isobmff/#av1codecconfigurationbox>
///
/// NOTE: level 4.0 supports resolutions up to 2048×1152.  If 4K+ output is
/// ever needed, `seq_level_idx_0` must be bumped (same debt exists on VP9).
const AV1_CODEC_PRIVATE: [u8; 4] = [0x81, 0x08, 0x0C, 0x00];

/// Opus codec lookahead at 48kHz in samples (typical libopus default).
///
/// This is written to the OpusHead `pre_skip` field so decoders can trim encoder delay.
/// The actual lookahead depends on the Opus encoder build; override via
/// [`WebMMuxerConfig::opus_preskip_samples`] if your encoder reports a different value.
const OPUS_PRESKIP_SAMPLES: u16 = 312;

fn opus_head_codec_private(
    sample_rate: u32,
    channels: u32,
    pre_skip: u16,
) -> Result<[u8; 19], StreamKitError> {
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
    head[10..12].copy_from_slice(&pre_skip.to_le_bytes());
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
/// Used for **Live** (streaming) mode only.  Bytes are drained on every
/// `take_data()` call so that memory stays bounded.
#[derive(Clone)]
struct SharedPacketBuffer {
    state: Arc<Mutex<BufferState>>,
}

impl SharedPacketBuffer {
    /// Create a non-seek streaming buffer.
    ///
    /// This is designed for `Writer::new_non_seek` in live streaming mode. Since the writer
    /// does not seek/backpatch, we can drain bytes out by moving the underlying `Vec<u8>`
    /// (no copy) and reset the cursor to keep memory bounded.
    fn new_streaming() -> Self {
        Self {
            state: Arc::new(Mutex::new(BufferState {
                cursor: Cursor::new(Vec::new()),
                last_sent_pos: 0,
                base_offset: 0,
            })),
        }
    }

    /// Takes any new data written since the last call.
    ///
    /// Streaming mode (non-seek): drain everything written so far without copying.
    fn take_data(&self) -> Option<Bytes> {
        // Mutex poisoning is a fatal error - allows expect() for this common pattern
        #[allow(clippy::expect_used)]
        let mut state = self.state.lock().expect("SharedPacketBuffer mutex poisoned");

        let base = state.base_offset;
        let current_len = state.cursor.get_ref().len();

        if current_len == 0 {
            return None;
        }

        // Drain everything written so far without copying.
        //
        // This avoids two major sources of allocation churn in DHAT profiles:
        // - copying out incremental slices on every flush
        // - repeatedly trimming a sliding window with `split_off` (copies the window)
        let data_vec = std::mem::take(state.cursor.get_mut());
        // Advance base_offset so Seek::Start can clamp consistently if it ever happens.
        state.base_offset = base + current_len;
        state.last_sent_pos = 0;
        state.cursor.set_position(0);
        drop(state);
        Some(Bytes::from(data_vec))
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

/// A file-backed buffer for **File** mode WebM muxing.
///
/// Instead of accumulating the entire muxed output in memory (which violates
/// the "never keep entire files in memory" principle), all writes go to an
/// anonymous temporary file on disk.  The temp file supports full seek so
/// libwebm can back-patch segment sizes and cues as needed.
///
/// At finalization the file contents are read back into a `Bytes` for the
/// single downstream send.  This is a one-time, bounded operation — the
/// file is deleted automatically when the struct is dropped.
struct FileBackedBuffer {
    inner: BufWriter<std::fs::File>,
}

impl FileBackedBuffer {
    /// Create a new file-backed buffer using an anonymous temp file.
    fn new() -> std::io::Result<Self> {
        let file = tempfile::tempfile()?;
        Ok(Self { inner: BufWriter::new(file) })
    }

    /// Read the entire temp file contents as `Bytes`.
    ///
    /// This should only be called **once** after `segment.finalize()` — all
    /// writes and seeks are complete at that point.
    fn take_data(&mut self) -> std::io::Result<Option<Bytes>> {
        self.inner.flush()?;
        let file = self.inner.get_mut();
        let len = file.stream_position()?;
        if len == 0 {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(0))?;
        let len_usize = usize::try_from(len).map_err(std::io::Error::other)?;
        let mut buf = vec![0u8; len_usize];
        file.read_exact(&mut buf)?;
        Ok(Some(Bytes::from(buf)))
    }
}

impl Write for FileBackedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for FileBackedBuffer {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Unified buffer type used by the WebM muxer.
///
/// - **Live** mode: wraps a [`SharedPacketBuffer`] (in-memory streaming, non-seek writer).
/// - **File** mode: wraps a [`FileBackedBuffer`] (temp file on disk, seekable writer).
///
/// This enum allows the muxer's `run()` method to use a single generic code
/// path regardless of the streaming mode.
enum MuxBuffer {
    Live(SharedPacketBuffer),
    File(FileBackedBuffer),
}

impl MuxBuffer {
    fn take_data(&mut self) -> Option<Bytes> {
        match self {
            Self::Live(buf) => buf.take_data(),
            Self::File(buf) => match buf.take_data() {
                Ok(data) => data,
                Err(e) => {
                    tracing::error!("Failed to read temp file data: {e}");
                    None
                },
            },
        }
    }
}

impl Write for MuxBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Live(b) => b.write(buf),
            Self::File(b) => b.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Live(b) => b.flush(),
            Self::File(b) => b.flush(),
        }
    }
}

impl Seek for MuxBuffer {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Live(_) => {
                // Live mode uses Writer::new_non_seek — seeking should never
                // happen.  Return an error to surface any unexpected code-path
                // changes in libwebm rather than silently producing corrupt output.
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Seek is not supported on the Live streaming buffer",
                ))
            },
            Self::File(b) => b.seek(pos),
        }
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
    /// Streaming mode: "live" for real-time streaming (no duration), "file" for complete files
    /// with duration (default)
    pub streaming_mode: WebMStreamingMode,
    /// Opus encoder lookahead in samples at 48 kHz, written to the OpusHead
    /// `pre_skip` field.  Decoders use this to trim encoder delay.
    /// Default: 312 (typical libopus default).
    pub opus_preskip_samples: u16,
}

impl Default for WebMMuxerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            video_width: 0,
            video_height: 0,
            streaming_mode: WebMStreamingMode::default(),
            opus_preskip_samples: OPUS_PRESKIP_SAMPLES,
        }
    }
}

/// Track handles resolved during segment setup.
struct MuxTracks {
    audio: Option<AudioTrack>,
    video: Option<VideoTrack>,
}

/// Builds the MIME content-type string based on which tracks are present.
///
/// When `video_is_av1` is `true` the video codec string is `"av1"` instead
/// of `"vp9"`.  This is needed for MSE consumers that initialise
/// SourceBuffers from the MIME type.
const fn webm_content_type(has_audio: bool, has_video: bool, video_is_av1: bool) -> &'static str {
    match (has_audio, has_video, video_is_av1) {
        (true, true, false) => "video/webm; codecs=\"vp9,opus\"",
        (true, true, true) => "video/webm; codecs=\"av1,opus\"",
        (false, true, false) => "video/webm; codecs=\"vp9\"",
        (false, true, true) => "video/webm; codecs=\"av1\"",
        (true, false, _) => "audio/webm; codecs=\"opus\"",
        // Shouldn't happen - at least one track is required - but provide a safe fallback.
        (false, false, _) => "video/webm",
    }
}

/// A node that muxes encoded Opus audio and/or VP9/AV1 video packets into a WebM container stream.
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
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
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
        // This static hint is used before the node runs (e.g. by the oneshot
        // backward walk to set the HTTP Content-Type header).  We can only
        // infer from config: if video dimensions are set, video is present.
        // Audio presence is unknown at this stage so we assume true — it is
        // safe to advertise "vp9,opus" even if only video is connected
        // (consumers simply won't find an Opus track), whereas advertising
        // only "vp9" when Opus IS present would break MSE consumers that
        // need to initialise an audio SourceBuffer.
        //
        // The video codec (VP9 vs AV1) is unknown at config time, so we
        // default to VP9 for the static hint.  The actual content_type is
        // resolved at runtime once the first video packet arrives.
        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        let has_audio = true;
        Some(webm_content_type(has_audio, has_video, false).to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("WebMMuxerNode starting");

        // --- Classify generic inputs as audio or video ---
        //
        // Inputs use generic pin names ("in", "in_1", …).  In oneshot/static
        // pipelines the graph builder populates `context.input_types` with the
        // upstream output's [`PacketType`] for each connected pin.
        //
        // In dynamic pipelines `input_types` is empty (connections are wired
        // after nodes are spawned).  In that case we fall back to first-packet
        // inspection: receive one packet from each channel and classify from
        // the packet's `content_type` field (e.g. "video/vp9" → video,
        // `None` → audio).  Inspected packets are buffered for replay after
        // segment setup.

        if context.inputs.is_empty() {
            let err_msg = "WebMMuxerNode requires at least one input (audio or video)".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        let mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;

        // Buffers for first packets consumed during classification (dynamic
        // pipeline path).  The video buffer is also used by the dimension
        // auto-detect step that follows.
        let mut first_video_packet: Option<(Bytes, Option<PacketMetadata>)> = None;
        let mut first_audio_packet: Option<(Bytes, Option<PacketMetadata>)> = None;

        // Detected video codec — determined from content_type (dynamic) or
        // connection type (oneshot).  Defaults to VP9 for backward compat.
        let mut video_is_av1 = false;

        let use_packet_inspection = context.input_types.is_empty();

        for (pin_name, mut rx) in context.inputs.drain() {
            let is_video = if use_packet_inspection {
                // Dynamic pipeline: classify from the first packet's content_type.
                match rx.recv().await {
                    Some(Packet::Binary { data, content_type, metadata }) => {
                        let ct_str = content_type.as_deref().unwrap_or("");
                        let video = ct_str.starts_with("video/");
                        if video {
                            video_is_av1 =
                            ct_str == "video/av1" || ct_str.starts_with("video/av1;");
                            first_video_packet = Some((data, metadata));
                        } else {
                            first_audio_packet = Some((data, metadata));
                        }
                        video
                    },
                    Some(_) => {
                        // Non-binary packet on a muxer input is unexpected;
                        // default to audio classification.
                        tracing::warn!(
                            "WebMMuxerNode: pin '{pin_name}' sent non-binary data, \
                             classifying as audio"
                        );
                        false
                    },
                    None => {
                        tracing::warn!(
                            "WebMMuxerNode: pin '{pin_name}' closed before sending any data"
                        );
                        continue;
                    },
                }
            } else {
                // Oneshot/static pipeline: classify from connection metadata.
                let pin_type = context.input_types.get(&pin_name);
                let is_vid = pin_type.is_some_and(|ty| {
                    matches!(ty, PacketType::EncodedVideo(_) | PacketType::RawVideo(_))
                });
                if is_vid {
                    // Detect AV1 from the connection's encoded video format.
                    if let Some(PacketType::EncodedVideo(fmt)) = pin_type {
                        if fmt.codec == VideoCodec::Av1 {
                            video_is_av1 = true;
                        }
                    }
                }
                is_vid
            };

            if is_video {
                if video_rx.is_some() {
                    let err_msg = format!(
                        "WebMMuxerNode: multiple video inputs detected (pin '{pin_name}'). \
                         Only one video track is supported."
                    );
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
                let source =
                    if use_packet_inspection { "packet inspection" } else { "connection type" };
                tracing::info!(
                    "WebMMuxerNode: pin '{pin_name}' classified as VIDEO (from {source})"
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
                let source =
                    if use_packet_inspection { "packet inspection" } else { "connection type" };
                tracing::info!(
                    "WebMMuxerNode: pin '{pin_name}' classified as AUDIO (from {source})"
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
            Cow::Borrowed(webm_content_type(has_audio, has_video, video_is_av1));

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // In Live mode we use a non-seek, in-memory streaming buffer so bytes
        // can be drained incrementally without keeping history.  In File mode
        // we use a temp file on disk so the muxer can seek/backpatch without
        // accumulating the entire output in memory.
        //
        // For Live mode we keep a cloned handle (`live_flush_handle`) so the
        // receive loop can drain bytes while the Writer owns the buffer.
        let (mux_buffer, live_flush_handle) = match self.config.streaming_mode {
            WebMStreamingMode::Live => {
                let spb = SharedPacketBuffer::new_streaming();
                let flush_handle = spb.clone();
                (MuxBuffer::Live(spb), Some(flush_handle))
            },
            WebMStreamingMode::File => {
                let fb = FileBackedBuffer::new().map_err(|e| {
                    let err_msg = format!("Failed to create temp file for WebM file mode: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    StreamKitError::Runtime(err_msg)
                })?;
                (MuxBuffer::File(fb), None)
            },
        };

        // Create writer with the unified buffer.
        //
        // Important: In `Live` mode we must avoid any backwards seeking/backpatching while
        // bytes are being streamed to the client. Using a non-seek writer forces libwebm to
        // produce a forward-only stream (unknown sizes/no cues), which is required for MSE
        // consumers like Firefox that are less tolerant of inconsistent metadata during
        // progressive append.  In File mode we use a seekable writer so libwebm can
        // back-patch segment sizes and write cues.
        let writer = match self.config.streaming_mode {
            WebMStreamingMode::Live => Writer::new_non_seek(mux_buffer),
            WebMStreamingMode::File => Writer::new(mux_buffer),
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
        // normal receive loop after the segment is built.  If the packet-
        // inspection classification path already consumed the first packet
        // (dynamic pipelines), we reuse it here instead of recv-ing again.

        let (video_width, video_height) = if has_video {
            let mut w = self.config.video_width;
            let mut h = self.config.video_height;

            if w == 0 || h == 0 {
                if video_is_av1 {
                    // AV1 dimension auto-detection is not supported — the VP9
                    // keyframe parser cannot parse AV1 OBUs.  Require explicit
                    // video_width/video_height in the config.
                    let err_msg = "WebMMuxerNode: video_width/video_height must be \
                         configured explicitly for AV1 (keyframe auto-detection \
                         is only supported for VP9)"
                        .to_string();
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }

                // Auto-detect: use the buffered first video packet if available
                // (from packet-inspection classification), otherwise recv one.
                let first_data = if let Some((data, meta)) = first_video_packet.take() {
                    tracing::info!(
                        "WebMMuxerNode: reusing buffered first video packet for \
                         dimension auto-detection"
                    );
                    Some((data, meta))
                } else {
                    tracing::info!(
                        "WebMMuxerNode: video_width/video_height not configured, \
                                    auto-detecting from first VP9 keyframe"
                    );
                    match video_rx.as_mut() {
                        Some(rx) => match rx.recv().await {
                            Some(Packet::Binary { data, metadata, .. }) => Some((data, metadata)),
                            _ => None,
                        },
                        None => None,
                    }
                };

                if let Some((data, metadata)) = first_data {
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
            let (codec_id, codec_private_bytes, codec_label): (VideoCodecId, &[u8], &str) =
                if video_is_av1 {
                    // See `AV1_CODEC_PRIVATE` doc comment for byte layout.
                    (VideoCodecId::AV1, &AV1_CODEC_PRIVATE, "AV1")
                } else {
                    // VP9 codec-private (VPCodecConfigurationRecord, 8 bytes).
                    // These constants must stay in sync with the VP9 encoder
                    // configuration in `crates/nodes/src/video/mod.rs`.
                    // Currently the encoder only supports profile 0 (I420/NV12
                    // at 8-bit), so the hardcoded values are correct.  If higher
                    // profiles are added (e.g. 10-bit, 4:4:4), these must be
                    // updated accordingly.
                    (VideoCodecId::VP9, &VP9_CODEC_PRIVATE as &[u8], "VP9")
                };

            let (builder, vt) = builder
                .add_video_track(video_width, video_height, codec_id, None)
                .map_err(|e| {
                    let err_msg = format!("Failed to add video track: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    StreamKitError::Runtime(err_msg)
                })?;

            let builder = builder.set_codec_private(vt, codec_private_bytes).map_err(|e| {
                let err_msg = format!("Failed to set {codec_label} codec private: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                StreamKitError::Runtime(err_msg)
            })?;

            tracks.video = Some(vt);
            tracing::info!(
                "Added {} video track ({}x{}) with codec private",
                codec_label,
                video_width,
                video_height
            );
            builder
        } else {
            builder
        };

        let builder = if has_audio {
            let opus_private = opus_head_codec_private(
                self.config.sample_rate,
                self.config.channels,
                self.config.opus_preskip_samples,
            )
            .map_err(|e| {
                let err_msg = format!("Failed to build OpusHead codec private: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                StreamKitError::Runtime(err_msg)
            })?;

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
        let mut mux_state = MuxState { header_sent: false, last_written_ns: 0, packet_count: 0 };

        tracing::info!("WebM segment built, entering receive loop to process incoming packets");

        // -- Receive loop: multiplex audio + video inputs --

        let mut audio_done = !has_audio;
        let mut video_done = !has_video;

        // Track whether we have received the first video keyframe.
        // WebM clusters must start with a keyframe, so non-keyframe video
        // frames received before the first keyframe cannot be muxed into a
        // valid stream — they are dropped immediately (no buffering) to
        // prevent unbounded memory growth when keyframes are infrequent.
        let mut video_keyframe_seen = !has_video;
        let mut dropped_video_frames: u64 = 0;

        // If we buffered the first video packet for dimension detection, replay
        // it through the normal mux path before entering the receive loop.
        if let Some((data, metadata)) = first_video_packet.take() {
            if let Some(video_track) = tracks.video {
                let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(true);
                if mux_frame(
                    &data,
                    metadata.as_ref(),
                    video_track,
                    is_keyframe,
                    DEFAULT_VIDEO_FRAME_DURATION_US,
                    &mut video_clock,
                    &mut mux_state,
                    &mut segment,
                    &mut context,
                    live_flush_handle.as_ref(),
                    &content_type_str,
                    &mut stats_tracker,
                    &node_name,
                )
                .await?
                {
                    video_done = true;
                }
                if is_keyframe {
                    video_keyframe_seen = true;
                }
            }
        }

        // If we buffered the first audio packet during packet-inspection
        // classification (dynamic pipelines), replay it before entering the
        // receive loop.
        if let Some((data, metadata)) = first_audio_packet.take() {
            if let Some(audio_track) = tracks.audio {
                // Audio frames are always keyframes.
                if mux_frame(
                    &data,
                    metadata.as_ref(),
                    audio_track,
                    true,
                    DEFAULT_FRAME_DURATION_US,
                    &mut audio_clock,
                    &mut mux_state,
                    &mut segment,
                    &mut context,
                    live_flush_handle.as_ref(),
                    &content_type_str,
                    &mut stats_tracker,
                    &node_name,
                )
                .await?
                {
                    audio_done = true;
                }
            }
        }

        while !audio_done || !video_done {
            enum MuxFrame {
                Audio(Bytes, Option<PacketMetadata>),
                Video(Bytes, Option<PacketMetadata>),
                AudioClosed,
                VideoClosed,
                Shutdown,
            }

            let frame = if audio_done {
                // Only video remains
                match video_rx.as_mut() {
                    Some(rx) => {
                        tokio::select! {
                            biased;
                            Some(msg) = context.control_rx.recv() => {
                                if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                    MuxFrame::Shutdown
                                } else {
                                    continue;
                                }
                            }
                            result = rx.recv() => match result {
                                Some(Packet::Binary { data, metadata, .. }) => {
                                    MuxFrame::Video(data, metadata)
                                },
                                Some(_) => continue,
                                None => MuxFrame::VideoClosed,
                            }
                        }
                    },
                    None => break,
                }
            } else if video_done {
                // Only audio remains
                match audio_rx.as_mut() {
                    Some(rx) => {
                        tokio::select! {
                            biased;
                            Some(msg) = context.control_rx.recv() => {
                                if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                    MuxFrame::Shutdown
                                } else {
                                    continue;
                                }
                            }
                            result = rx.recv() => match result {
                                Some(Packet::Binary { data, metadata, .. }) => {
                                    MuxFrame::Audio(data, metadata)
                                },
                                Some(_) => continue,
                                None => MuxFrame::AudioClosed,
                            }
                        }
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
                            biased; // prefer shutdown, then audio for stable ordering
                            Some(msg) = context.control_rx.recv() => {
                                if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                    MuxFrame::Shutdown
                                } else {
                                    continue;
                                }
                            }
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
                MuxFrame::Shutdown => {
                    tracing::info!("WebMMuxerNode received shutdown signal");
                    break;
                },
                MuxFrame::AudioClosed => {
                    tracing::info!("WebMMuxerNode audio input closed");
                    audio_done = true;
                },
                MuxFrame::VideoClosed => {
                    if dropped_video_frames > 0 && !video_keyframe_seen {
                        tracing::warn!(
                            "WebMMuxerNode: video input closed after dropping \
                             {dropped_video_frames} frames (no keyframe was ever received)"
                        );
                    }
                    tracing::info!("WebMMuxerNode video input closed");
                    video_done = true;
                },
                MuxFrame::Audio(data, metadata) => {
                    let Some(audio_track) = tracks.audio else {
                        continue;
                    };
                    // Audio frames are always keyframes.
                    if mux_frame(
                        &data,
                        metadata.as_ref(),
                        audio_track,
                        true,
                        DEFAULT_FRAME_DURATION_US,
                        &mut audio_clock,
                        &mut mux_state,
                        &mut segment,
                        &mut context,
                        live_flush_handle.as_ref(),
                        &content_type_str,
                        &mut stats_tracker,
                        &node_name,
                    )
                    .await?
                    {
                        break;
                    }
                },
                MuxFrame::Video(data, metadata) => {
                    let Some(video_track) = tracks.video else {
                        continue;
                    };
                    let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);

                    // Gate on the first keyframe so the WebM stream starts at
                    // a valid cluster boundary.  Non-keyframe frames received
                    // before the first keyframe are dropped immediately — they
                    // cannot be decoded without a preceding reference frame.
                    if !video_keyframe_seen {
                        if is_keyframe {
                            if dropped_video_frames > 0 {
                                tracing::info!(
                                    "WebMMuxerNode: first keyframe received after \
                                     dropping {dropped_video_frames} non-keyframe \
                                     video frames"
                                );
                            }
                            video_keyframe_seen = true;
                            // Fall through to mux this keyframe normally.
                        } else {
                            dropped_video_frames += 1;
                            if dropped_video_frames.is_multiple_of(300) {
                                tracing::warn!(
                                    "WebMMuxerNode: dropped {dropped_video_frames} \
                                     video frames while waiting for first keyframe"
                                );
                            }
                            continue;
                        }
                    }

                    if mux_frame(
                        &data,
                        metadata.as_ref(),
                        video_track,
                        is_keyframe,
                        DEFAULT_VIDEO_FRAME_DURATION_US,
                        &mut video_clock,
                        &mut mux_state,
                        &mut segment,
                        &mut context,
                        live_flush_handle.as_ref(),
                        &content_type_str,
                        &mut stats_tracker,
                        &node_name,
                    )
                    .await?
                    {
                        break;
                    }
                },
            }
        }

        tracing::info!(
            "WebMMuxerNode input streams closed, processed {} packets total",
            mux_state.packet_count
        );

        // Finalize the segment and recover the buffer.
        let writer = segment.finalize(None).map_err(|_e| {
            let err_msg = "Failed to finalize WebM segment".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            StreamKitError::Runtime(err_msg)
        })?;
        let mut mux_buffer = writer.into_inner();

        // Flush any remaining data from the buffer
        if let Some(data) = mux_buffer.take_data() {
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

/// Mutable state shared across the muxer receive loop.
///
/// Groups the monotonic timestamp guard, header-sent flag, and packet counter
/// into a single struct to reduce the number of loose parameters passed to
/// [`mux_frame`] and [`flush_output`].
///
/// Per-track clocks are kept separate so callers can borrow a clock and this
/// struct simultaneously without aliasing.
struct MuxState {
    /// Whether the WebM header has been flushed to the output.
    header_sent: bool,
    /// Monotonic timestamp guard: libwebm requires that timestamps across all
    /// tracks are non-decreasing.  We track the last written timestamp and
    /// clamp if needed.
    last_written_ns: u64,
    packet_count: u64,
}

/// Timestamps, clocks, and writes a single frame (audio or video) to the WebM
/// segment, then flushes any buffered output.
///
/// Returns `Ok(true)` if the output channel is closed (caller should stop),
/// `Ok(false)` to continue, or `Err` on fatal errors.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::ptr_arg)] // content_type is cloned as Cow<'static, str> for Packet; &str would force allocation
async fn mux_frame(
    data: &[u8],
    metadata: Option<&PacketMetadata>,
    track: impl Into<u64>,
    is_keyframe: bool,
    default_duration_us: u64,
    clock: &mut streamkit_core::timing::MediaClock,
    state: &mut MuxState,
    segment: &mut webm::mux::Segment<MuxBuffer>,
    context: &mut NodeContext,
    live_buffer: Option<&SharedPacketBuffer>,
    content_type: &Cow<'static, str>,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<bool, StreamKitError> {
    state.packet_count += 1;
    stats_tracker.received();

    let incoming_ts_us = metadata.and_then(|m| m.timestamp_us);
    let incoming_duration_us = metadata.and_then(|m| m.duration_us).or(Some(default_duration_us));

    if let Some(ts) = incoming_ts_us {
        clock.seed_from_timestamp_us(ts);
    } else if clock.timestamp_us() == 0 {
        clock.seed_from_timestamp_us(0);
    }

    let presentation_ts_us = incoming_ts_us.unwrap_or_else(|| clock.timestamp_us());
    clock.advance_by_duration_us(incoming_duration_us, default_duration_us);

    let mut timestamp_ns = presentation_ts_us.saturating_mul(1000);
    if timestamp_ns < state.last_written_ns {
        timestamp_ns = state.last_written_ns;
    }

    if let Err(e) = segment.add_frame(track, data, timestamp_ns, is_keyframe) {
        stats_tracker.errored();
        stats_tracker.maybe_send();
        let err_msg = format!("Failed to add frame to segment: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &err_msg);
        return Err(StreamKitError::Runtime(err_msg));
    }

    state.last_written_ns = timestamp_ns;

    let output_metadata = Some(PacketMetadata {
        timestamp_us: Some(presentation_ts_us),
        duration_us: incoming_duration_us,
        sequence: metadata.and_then(|m| m.sequence),
        keyframe: Some(is_keyframe),
    });

    let stopped = flush_output(
        context,
        live_buffer,
        content_type,
        output_metadata,
        &mut state.header_sent,
        stats_tracker,
        node_name,
    )
    .await?;

    stats_tracker.maybe_send();
    Ok(stopped)
}

/// Flushes buffered WebM data to the output sender.
///
/// In **Live** mode, bytes are drained incrementally after every frame to keep
/// memory bounded and enable real-time streaming.  In **File** mode the data
/// lives on disk in a temp file and is only read back once after finalization
/// (handled by the caller), so this function is a no-op.
///
/// `live_buffer` is `Some` only in Live mode — it is the cloned
/// `SharedPacketBuffer` handle that shares the same `Arc<Mutex<...>>` backing
/// store as the `MuxBuffer::Live` variant owned by the Writer.
///
/// Returns `Ok(true)` if the output channel is closed (node should stop),
/// `Ok(false)` to continue, or `Err` on fatal errors.
#[allow(clippy::ptr_arg)]
async fn flush_output(
    context: &mut NodeContext,
    live_buffer: Option<&SharedPacketBuffer>,
    content_type: &Cow<'static, str>,
    output_metadata: Option<PacketMetadata>,
    header_sent: &mut bool,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<bool, StreamKitError> {
    // In File mode there is no live buffer — skip all intermediate flushes.
    // The data will be read from the temp file after `segment.finalize()`.
    let Some(shared_buffer) = live_buffer else {
        return Ok(false);
    };

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
            "Muxes Opus audio and/or VP9/AV1 video into a WebM container. \
             Produces streamable WebM output compatible with web browsers. \
             Supports audio-only, video-only, or combined audio+video muxing.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamkit_core::ProcessorNode;

    /// Helper to build a `WebMMuxerNode` with the given video dimensions.
    fn muxer_with_dims(w: u32, h: u32) -> WebMMuxerNode {
        WebMMuxerNode::new(WebMMuxerConfig {
            video_width: w,
            video_height: h,
            ..WebMMuxerConfig::default()
        })
    }

    #[test]
    fn content_type_audio_only_when_no_video_dims() {
        let node = muxer_with_dims(0, 0);
        let Some(ct) = node.content_type() else {
            panic!("content_type should return Some");
        };
        assert_eq!(ct, "audio/webm; codecs=\"opus\"");
    }

    /// Regression test: when video dimensions are set, the static hint must
    /// include both codecs so that MSE consumers can initialise an audio
    /// SourceBuffer.  Previously `has_audio = !has_video` caused the hint
    /// to omit Opus for combined A+V pipelines.
    #[test]
    fn content_type_includes_opus_when_video_dims_set() {
        let node = muxer_with_dims(1280, 720);
        let Some(ct) = node.content_type() else {
            panic!("content_type should return Some");
        };
        assert_eq!(ct, "video/webm; codecs=\"vp9,opus\"");
    }

    #[test]
    fn webm_content_type_helper_covers_all_combinations() {
        // VP9 (video_is_av1 = false)
        assert_eq!(webm_content_type(true, true, false), "video/webm; codecs=\"vp9,opus\"");
        assert_eq!(webm_content_type(false, true, false), "video/webm; codecs=\"vp9\"");
        assert_eq!(webm_content_type(true, false, false), "audio/webm; codecs=\"opus\"");
        assert_eq!(webm_content_type(false, false, false), "video/webm");
        // AV1 (video_is_av1 = true)
        assert_eq!(webm_content_type(true, true, true), "video/webm; codecs=\"av1,opus\"");
        assert_eq!(webm_content_type(false, true, true), "video/webm; codecs=\"av1\"");
        assert_eq!(webm_content_type(true, false, true), "audio/webm; codecs=\"opus\"");
        assert_eq!(webm_content_type(false, false, true), "video/webm");
    }
}
