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
    /// Number of input pins to declare (1 or 2).
    ///
    /// Set to 2 for pipelines that feed both audio and video into the muxer
    /// (e.g. `needs: { in: opus_encoder, in_1: vp9_encoder }`).  Defaults
    /// to 1 for single-input (audio-only or video-only) pipelines.
    #[serde(default = "default_num_inputs")]
    #[schemars(range(min = 1, max = 2))]
    pub num_inputs: u32,
}

const fn default_num_inputs() -> u32 {
    1
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
            num_inputs: default_num_inputs(),
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
///
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
        // Pin count is driven by `num_inputs` (1 or 2).  Each pin accepts
        // audio or video — the actual media type is detected at runtime from
        // the packet's `content_type` field, not from the pin name.
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

        let mut pins = vec![InputPin {
            name: "in".to_string(),
            accepts_types: media_types.clone(),
            cardinality: PinCardinality::One,
        }];
        if self.config.num_inputs >= 2 {
            pins.push(InputPin {
                name: "in_1".to_string(),
                accepts_types: media_types,
                cardinality: PinCardinality::One,
            });
        }
        pins
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
        // backward walk to set the HTTP Content-Type header).  We infer from
        // config: if video dimensions are set, video is present.  Audio is
        // always assumed true — advertising "vp9,opus" when only video is
        // connected is safe (consumers won't find an Opus track), but
        // advertising "vp9" when Opus IS present would break MSE consumers
        // that need to initialise an audio SourceBuffer.
        //
        // The video codec (VP9 vs AV1) is unknown at config time, so we
        // default to VP9 for the static hint.  The actual content_type is
        // resolved at runtime once the first video packet arrives.
        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        Some(webm_content_type(true, has_video, false).to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("WebMMuxerNode starting");

        if context.inputs.is_empty() {
            let err_msg = "WebMMuxerNode requires at least one input (audio or video)".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        // Fast path: when num_inputs >= 2 and video dimensions are configured,
        // we know both audio + video will be present and don't need to classify
        // pins by inspecting packets.  This avoids a multi-second blocking
        // startup while waiting for the slower track.
        let skip_classification = self.config.num_inputs >= 2
            && self.config.video_width > 0
            && self.config.video_height > 0;

        let mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut first_video_packet: Option<(Bytes, Option<PacketMetadata>)> = None;
        let mut first_audio_packet: Option<(Bytes, Option<PacketMetadata>)> = None;
        let mut video_is_av1 = false;

        // Collect all input receivers.  When skipping classification, ALL
        // receivers go into a unified vec for on-the-fly routing in the
        // receive loop.  When classifying, this vec stays empty and the
        // receivers are assigned to audio_rx / video_rx.
        let mut all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>> = Vec::new();

        let use_packet_inspection = !skip_classification && context.input_types.is_empty();

        if skip_classification {
            // Fast path: collect all receivers into a unified vec.
            // We still need to detect the video codec (VP9 vs AV1) so the
            // segment header uses the correct codec ID and content-type.
            //
            // Detection priority:
            //   1. connection metadata (input_types — populated by the
            //      engine for static/oneshot pipelines)
            //   2. first video packet's content_type (needed for dynamic
            //      sessions where input_types is empty)
            for (pin_name, rx) in context.inputs.drain() {
                if let Some(PacketType::EncodedVideo(fmt)) = context.input_types.get(&pin_name) {
                    if fmt.codec == VideoCodec::Av1 {
                        video_is_av1 = true;
                    }
                }
                all_receivers.push(rx);
            }

            // If input_types didn't reveal the codec, peek at the first
            // packet from each receiver to detect AV1 from content_type.
            if !video_is_av1 {
                for rx in &mut all_receivers {
                    if let Some(Packet::Binary { data, content_type, metadata }) = rx.recv().await {
                        let ct_str = content_type.as_deref().unwrap_or("");
                        if ct_str == "video/av1" || ct_str.starts_with("video/av1;") {
                            video_is_av1 = true;
                            first_video_packet = Some((data, metadata));
                            break;
                        }
                        if ct_str.starts_with("video/") {
                            first_video_packet = Some((data, metadata));
                        } else {
                            first_audio_packet = Some((data, metadata));
                        }
                    }
                }
            }

            state_helpers::emit_running(&context.state_tx, &node_name);
            tracing::info!(
                "WebMMuxerNode: skipping classification (num_inputs={}, dims={}x{}, av1={})",
                self.config.num_inputs,
                self.config.video_width,
                self.config.video_height,
                video_is_av1,
            );
        } else if use_packet_inspection {
            state_helpers::emit_running(&context.state_tx, &node_name);
        }

        for (pin_name, mut rx) in context.inputs.drain().filter(|_| !skip_classification) {
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

        let has_audio = if skip_classification { true } else { audio_rx.is_some() };
        let has_video = if skip_classification { true } else { video_rx.is_some() };

        if !has_audio && !has_video {
            let err_msg =
                "WebMMuxerNode: no connected inputs could be classified as audio or video"
                    .to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        if !use_packet_inspection && !skip_classification {
            state_helpers::emit_running(&context.state_tx, &node_name);
        }

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

        // Per-track timestamp rebase offsets — computed lazily when each track's
        // first frame arrives.
        let mut audio_rebase_offset_ns: Option<i64> = None;
        let mut video_rebase_offset_ns: Option<i64> = None;

        // Per-track last-written timestamps for per-track monotonicity.
        let mut audio_last_ns: Option<u64> = None;
        let mut video_last_ns: Option<u64> = None;

        tracing::info!("WebM segment built, entering receive loop to process incoming packets");

        let mut audio_done = !has_audio;
        let mut video_done = !has_video;

        let mut video_keyframe_seen = !has_video;
        let mut dropped_video_frames: u64 = 0;
        let mux_start = std::time::Instant::now();
        let mut audio_frame_count = 0u64;
        let mut video_frame_count = 0u64;

        // Write first video packet (from auto-detection or packet inspection).
        if let Some((data, metadata)) = first_video_packet.take() {
            if let Some(video_track) = tracks.video {
                let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(true);
                if is_keyframe {
                    video_keyframe_seen = true;
                }
                mux_state.packet_count += 1;
                stats_tracker.received();
                let frame = stage_frame(
                    data,
                    metadata,
                    video_track.into(),
                    is_keyframe,
                    DEFAULT_VIDEO_FRAME_DURATION_US,
                    &mut video_clock,
                    &mut video_rebase_offset_ns,
                    mux_state.last_written_ns,
                    &mut video_last_ns,
                );
                if write_frame(
                    &frame,
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
            }
        }

        // Write first audio packet — but ONLY for single-input pipelines.
        // In dual-input (A+V) pipelines, the audio first-packet was consumed
        // during classification while video was already running.  Writing it
        // now would lock the audio rebase offset to last_written≈0, and then
        // all subsequent audio frames (arriving after seconds of video) would
        // map far behind the video position.  Dropping it lets the receive
        // loop compute the rebase from the current video position.
        if !has_video {
            if let Some((data, metadata)) = first_audio_packet.take() {
                if let Some(audio_track) = tracks.audio {
                    mux_state.packet_count += 1;
                    stats_tracker.received();
                    let frame = stage_frame(
                        data,
                        metadata,
                        audio_track.into(),
                        true,
                        DEFAULT_FRAME_DURATION_US,
                        &mut audio_clock,
                        &mut audio_rebase_offset_ns,
                        mux_state.last_written_ns,
                        &mut audio_last_ns,
                    );
                    if write_frame(
                        &frame,
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
        }

        // Drain frames that queued in both channels during the sequential
        // classification.  These are seconds old (stale) and would produce a
        // burst of WebM data that overwhelms the MSE SourceBuffer.
        // NOTE: in skip-classification mode audio_rx/video_rx are None, so
        // this block is a no-op — all receivers are in all_receivers instead.
        if has_audio && has_video {
            let mut drained_v = 0u64;
            let mut drained_a = 0u64;
            let mut drained_keyframes = 0u64;
            if let Some(ref mut rx) = video_rx {
                while let Ok(pkt) = rx.try_recv() {
                    drained_v += 1;
                    if let Packet::Binary { metadata, .. } = &pkt {
                        if metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false) {
                            drained_keyframes += 1;
                        }
                    }
                }
            }
            if let Some(ref mut rx) = audio_rx {
                while rx.try_recv().is_ok() {
                    drained_a += 1;
                }
            }
            if drained_v > 0 || drained_a > 0 {
                tracing::info!(
                    "WebMMuxerNode: drained {drained_v} video ({drained_keyframes} keyframes) \
                     + {drained_a} audio stale frames from channels"
                );
            }
        }

        // -- Main receive loop: write frames in arrival order --
        //
        // Frames are written immediately as they arrive from either track.
        // Per-track monotonicity is enforced in `stage_frame`.  Global
        // monotonicity for libwebm is enforced in `write_frame` via a soft
        // clamp (equality across tracks is allowed).
        //
        // In skip-classification mode, `all_receivers` holds every input
        // channel and frames are classified on-the-fly from `content_type`.
        // In the legacy path, `audio_rx`/`video_rx` are pre-classified.
        let mut inputs_open = if skip_classification { all_receivers.len() } else { 0 };
        while (skip_classification && inputs_open > 0)
            || (!skip_classification && (!audio_done || !video_done))
        {
            let frame = if skip_classification {
                // Unified receive: select on all input receivers + control.
                // Frames are classified on-the-fly from content_type below.
                let recv_result = if all_receivers.len() >= 2 {
                    let (first, rest) = all_receivers.split_at_mut(1);
                    let rx0 = &mut first[0];
                    let rx1 = &mut rest[0];
                    // SAFETY: `tokio::select!` executes exactly one branch, so
                    // at most one `remove()` runs per iteration.  After removing
                    // index 0 the vec shrinks to len()==1, and the next iteration
                    // takes the `len() == 1` path below — no stale-index panic.
                    tokio::select! {
                        biased;
                        Some(msg) = context.control_rx.recv() => {
                            if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                Some(MuxFrame::Shutdown)
                            } else {
                                None // non-shutdown control, loop again
                            }
                        }
                        r0 = rx0.recv() => {
                            r0.map_or_else(
                                || { all_receivers.remove(0); inputs_open -= 1; None },
                                classify_packet,
                            )
                        }
                        r1 = rx1.recv() => {
                            r1.map_or_else(
                                || { all_receivers.remove(1); inputs_open -= 1; None },
                                classify_packet,
                            )
                        }
                    }
                } else if all_receivers.len() == 1 {
                    let rx = &mut all_receivers[0];
                    tokio::select! {
                        biased;
                        Some(msg) = context.control_rx.recv() => {
                            if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                                Some(MuxFrame::Shutdown)
                            } else { None }
                        }
                        r = rx.recv() => r.map_or_else(
                            || { all_receivers.clear(); inputs_open = 0; None },
                            classify_packet,
                        )
                    }
                } else {
                    break;
                };
                match recv_result {
                    Some(f) => f,
                    None => continue,
                }
            } else if audio_done {
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
                let (Some(a_rx), Some(v_rx)) = (audio_rx.as_mut(), video_rx.as_mut()) else {
                    break;
                };
                tokio::select! {
                    biased;
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
                    tracing::info!("WebMMuxerNode video input closed");
                    video_done = true;
                },
                MuxFrame::Audio(data, metadata) => {
                    let Some(audio_track) = tracks.audio else {
                        continue;
                    };
                    mux_state.packet_count += 1;
                    stats_tracker.received();
                    audio_frame_count += 1;
                    if audio_frame_count <= 3 {
                        let ts = metadata.as_ref().and_then(|m| m.timestamp_us);
                        tracing::debug!(
                            "WebMMuxer AUDIO #{audio_frame_count}: \
                             incoming_ts={ts:?}us elapsed={:.0}ms",
                            mux_start.elapsed().as_secs_f64() * 1000.0,
                        );
                    }
                    let frame = stage_frame(
                        data,
                        metadata,
                        audio_track.into(),
                        true,
                        DEFAULT_FRAME_DURATION_US,
                        &mut audio_clock,
                        &mut audio_rebase_offset_ns,
                        mux_state.last_written_ns,
                        &mut audio_last_ns,
                    );
                    if write_frame(
                        &frame,
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

                    mux_state.packet_count += 1;
                    stats_tracker.received();
                    video_frame_count += 1;
                    if video_frame_count <= 3 {
                        let ts = metadata.as_ref().and_then(|m| m.timestamp_us);
                        tracing::debug!(
                            "WebMMuxer VIDEO #{video_frame_count}: \
                             incoming_ts={ts:?}us elapsed={:.0}ms keyframe={is_keyframe}",
                            mux_start.elapsed().as_secs_f64() * 1000.0,
                        );
                    }
                    let frame = stage_frame(
                        data,
                        metadata,
                        video_track.into(),
                        is_keyframe,
                        DEFAULT_VIDEO_FRAME_DURATION_US,
                        &mut video_clock,
                        &mut video_rebase_offset_ns,
                        mux_state.last_written_ns,
                        &mut video_last_ns,
                    );
                    if write_frame(
                        &frame,
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

            // Periodic pipeline health — every 150 packets (~2s).
            if mux_state.packet_count.is_multiple_of(150) {
                // Timestamps in ms are well within i64 range for any practical stream.
                #[allow(clippy::cast_possible_wrap)]
                let a_ms = audio_last_ns.map_or(-1i64, |ns| (ns / 1_000_000) as i64);
                #[allow(clippy::cast_possible_wrap)]
                let v_ms = video_last_ns.map_or(-1i64, |ns| (ns / 1_000_000) as i64);
                let delta_ms = if a_ms >= 0 && v_ms >= 0 { a_ms - v_ms } else { 0 };
                tracing::debug!(
                    "WebMMuxer health: pkt#{} elapsed={:.1}s a_ts={a_ms}ms v_ts={v_ms}ms \
                     av_delta={delta_ms}ms global_last={}ms",
                    mux_state.packet_count,
                    mux_start.elapsed().as_secs_f64(),
                    mux_state.last_written_ns / 1_000_000,
                );
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
/// Groups the timestamp guard, header-sent flag, and packet counter into a
/// single struct to reduce the number of loose parameters passed to
/// [`write_frame`] and [`flush_output`].
///
/// Per-track clocks are kept separate so callers can borrow a clock and this
/// struct simultaneously without aliasing.
struct MuxState {
    /// Whether the WebM header has been flushed to the output.
    header_sent: bool,
    /// Last timestamp (ns) written to the segment.
    last_written_ns: u64,
    packet_count: u64,
}

/// A frame produced by [`stage_frame`] ready for [`write_frame`].
///
/// Holds the frame data alongside its rebased, per-track-monotonic
/// timestamp.  `stage_frame` computes the timestamp without touching the
/// segment; `write_frame` applies the global max-clamp and writes to
/// libwebm.
struct StagedFrame {
    data: Bytes,
    metadata: Option<PacketMetadata>,
    track_id: u64,
    is_keyframe: bool,
    /// Rebased, per-track-monotonic timestamp in nanoseconds.
    timestamp_ns: u64,
    presentation_ts_us: u64,
    duration_us: Option<u64>,
}

/// Frame variant received from the audio/video input channels.
enum MuxFrame {
    Audio(Bytes, Option<PacketMetadata>),
    Video(Bytes, Option<PacketMetadata>),
    AudioClosed,
    VideoClosed,
    Shutdown,
}

/// Classify a [`Packet`] as audio or video from its `content_type` field.
///
/// Used in the skip-classification fast path where pins are not pre-assigned
/// to audio/video roles.  Returns `None` for non-`Binary` packets.
fn classify_packet(packet: Packet) -> Option<MuxFrame> {
    match packet {
        Packet::Binary { data, content_type, metadata } => {
            let is_video = content_type.as_deref().unwrap_or("").starts_with("video/");
            Some(if is_video {
                MuxFrame::Video(data, metadata)
            } else {
                MuxFrame::Audio(data, metadata)
            })
        },
        _ => None,
    }
}

/// Computes the rebased, per-track-monotonic timestamp for a frame.
///
/// Timestamp pipeline (each layer builds on the previous):
/// 1. **MoQ peer** normalises raw capture timestamps to start near 0.
/// 2. **Compositor** calibrates its running clock to the remote input's
///    domain so video timestamps share the MoQ epoch.
/// 3. **This function** applies a per-track rebase offset (aligning the
///    late-arriving track to the first track's position) and enforces
///    per-track monotonicity (+1 ms on backward jumps).  Large backward
///    jumps (> 500 ms) trigger a **rebase reset**: the offset is
///    recomputed from `last_written_ns` so the track re-aligns with the
///    current global position, preventing permanent A/V desync from
///    compositor calibration.
/// 4. **[`write_frame`]** applies a global max-clamp for libwebm's
///    non-decreasing requirement.
///
/// Does **not** touch the segment or `last_written_ns`.
#[allow(clippy::too_many_arguments)]
fn stage_frame(
    data: Bytes,
    metadata: Option<PacketMetadata>,
    track_id: u64,
    is_keyframe: bool,
    default_duration_us: u64,
    clock: &mut streamkit_core::timing::MediaClock,
    rebase_offset_ns: &mut Option<i64>,
    last_written_ns: u64,
    per_track_last_ns: &mut Option<u64>,
) -> StagedFrame {
    let incoming_ts_us = metadata.as_ref().and_then(|m| m.timestamp_us);
    let incoming_duration_us =
        metadata.as_ref().and_then(|m| m.duration_us).or(Some(default_duration_us));

    // Use incoming timestamps when available (normalized MoQ timestamps
    // from the peer node start near 0).  Fall back to a synthetic clock
    // for tracks without timestamps.
    if let Some(ts) = incoming_ts_us {
        clock.seed_from_timestamp_us(ts);
    } else if clock.timestamp_us() == 0 {
        clock.seed_from_timestamp_us(0);
    }
    let presentation_ts_us = incoming_ts_us.unwrap_or_else(|| clock.timestamp_us());
    clock.advance_by_duration_us(incoming_duration_us, default_duration_us);

    let raw_ns = presentation_ts_us.saturating_mul(1000);

    // Per-track rebase: when a track's first frame arrives, compute an offset
    // so it starts at the other track's current position.  This aligns tracks
    // that start at different wall-clock times (e.g. local compositor video
    // at t=0 and MoQ audio arriving seconds later).  Without this, MSE
    // consumers see timestamp gaps between tracks and can't play smoothly.
    let is_new_offset = rebase_offset_ns.is_none();
    // Media timestamps in nanoseconds are well within i64 range for practical streams.
    #[allow(clippy::cast_possible_wrap)]
    let offset = *rebase_offset_ns.get_or_insert_with(|| last_written_ns as i64 - raw_ns as i64);
    if is_new_offset {
        tracing::info!(
            "WebMMuxer track {track_id}: rebase offset={offset}ns \
             (raw={raw_ns}ns, last_written={last_written_ns}ns, \
             incoming_ts={incoming_ts_us:?}us)"
        );
    }
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
    // offset keeps the result non-negative via the clamp; raw_ns fits i64 for practical streams
    let mut timestamp_ns = (raw_ns as i64).saturating_add(offset).max(0) as u64;

    // Per-track monotonicity — ensure strictly increasing timestamps within
    // each track.  This replaces the old global clamp which distorted the
    // *other* track's timing when clamping across tracks.
    //
    // Large backward jumps (> 500 ms) indicate an upstream epoch reset —
    // most commonly the compositor calibrating its running clock to a
    // remote MoQ input.  Pre-calibration frames established the rebase
    // offset when the track started at t ≈ 0, but post-calibration
    // timestamps jump back to the MoQ epoch.  Simply bumping +1 ms per
    // frame would keep the track creeping forward at 1 ms/frame for
    // hundreds of frames, creating a permanent A/V offset equal to the
    // pipeline startup delay.
    //
    // Instead, recompute the rebase offset from `last_written_ns` so the
    // track re-aligns with the current global position (which is
    // dominated by the other track that has been flowing normally).
    // This leaves a small gap in the track's container timeline (from the
    // pre-calibration end to the new position), but MSE live-edge seeking
    // skips past it.
    if let Some(last) = *per_track_last_ns {
        if timestamp_ns <= last {
            // 500 ms — large enough to ignore jitter / micro-reorder,
            // small enough to catch compositor calibration jumps.
            const REBASE_RESET_THRESHOLD_NS: u64 = 500_000_000;
            let gap_ns = last - timestamp_ns;
            if gap_ns > REBASE_RESET_THRESHOLD_NS {
                // Media timestamps in nanoseconds are well within i64 range
                // for practical streams.
                #[allow(clippy::cast_possible_wrap)]
                let new_offset = last_written_ns as i64 - raw_ns as i64;
                tracing::info!(
                    "WebMMuxer track {track_id}: rebase reset (backward jump {gap_ns}ns > threshold) \
                     old_offset={offset}ns new_offset={new_offset}ns \
                     (raw={raw_ns}ns, last_written={last_written_ns}ns)"
                );
                *rebase_offset_ns = Some(new_offset);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
                {
                    timestamp_ns = (raw_ns as i64).saturating_add(new_offset).max(0) as u64;
                }
            }
            // After a potential re-rebase, still enforce monotonicity for
            // the (now small) remaining gap or normal jitter.
            if timestamp_ns <= last {
                timestamp_ns = last + 1_000_000; // +1ms (WebM timecode resolution)
            }
        }
    }
    *per_track_last_ns = Some(timestamp_ns);

    StagedFrame {
        data,
        metadata,
        track_id,
        is_keyframe,
        timestamp_ns,
        presentation_ts_us,
        duration_us: incoming_duration_us,
    }
}

/// Writes a [`StagedFrame`] to the WebM segment and flushes output.
///
/// Applies a global clamp to satisfy libwebm's non-decreasing timestamp
/// requirement.  Timestamps are forced **strictly increasing** (+1 ms
/// when a frame would otherwise equal the previous write) to prevent
/// cross-track timestamp equality.  Chrome's muxed-WebM MSE demuxer
/// stalls intermittently when multiple SimpleBlocks share the same
/// timecode, so unique timestamps per packet are essential for smooth
/// live playback.
///
/// Returns `Ok(true)` if the output channel is closed (caller should stop),
/// `Ok(false)` to continue, or `Err` on fatal errors.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::ptr_arg)]
async fn write_frame(
    frame: &StagedFrame,
    state: &mut MuxState,
    segment: &mut webm::mux::Segment<MuxBuffer>,
    context: &mut NodeContext,
    live_buffer: Option<&SharedPacketBuffer>,
    content_type: &Cow<'static, str>,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<bool, StreamKitError> {
    // Ensure strictly increasing write timestamps.  When a frame from
    // one track arrives after a frame from another track with a higher
    // (or equal) staged timestamp, bumping by 1 ms avoids cross-track
    // equality in the container.  The bump does not cascade because
    // per-track staged timestamps (computed by `stage_frame`) are
    // independent of `last_written_ns` for existing tracks.
    let write_ts = if frame.timestamp_ns > state.last_written_ns {
        frame.timestamp_ns
    } else {
        state.last_written_ns + 1_000_000 // +1 ms (WebM timecode resolution)
    };

    if let Err(e) = segment.add_frame(frame.track_id, &frame.data, write_ts, frame.is_keyframe) {
        stats_tracker.errored();
        stats_tracker.maybe_send();
        let err_msg = format!(
            "Failed to add frame to segment: {e} (track={}, write_ts={write_ts}ns, \
             frame_ts={}ns, last_written={}ns, keyframe={}, data_len={})",
            frame.track_id,
            frame.timestamp_ns,
            state.last_written_ns,
            frame.is_keyframe,
            frame.data.len()
        );
        state_helpers::emit_failed(&context.state_tx, node_name, &err_msg);
        return Err(StreamKitError::Runtime(err_msg));
    }

    state.last_written_ns = write_ts;

    let output_metadata = Some(PacketMetadata {
        timestamp_us: Some(frame.presentation_ts_us),
        duration_us: frame.duration_us,
        sequence: frame.metadata.as_ref().and_then(|m| m.sequence),
        keyframe: Some(frame.is_keyframe),
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use streamkit_core::ProcessorNode;

    /// Mirrors the strictly-increasing write logic from `write_frame`.
    fn strictly_increasing_ts(frame_ts: u64, last_written: u64) -> u64 {
        if frame_ts > last_written {
            frame_ts
        } else {
            last_written + 1_000_000
        }
    }

    /// Helper to build a `WebMMuxerNode` with the given video dimensions.
    fn muxer_with_dims(w: u32, h: u32) -> WebMMuxerNode {
        WebMMuxerNode::new(WebMMuxerConfig {
            video_width: w,
            video_height: h,
            ..WebMMuxerConfig::default()
        })
    }

    /// `content_type()` uses video dimensions to decide the static MIME hint:
    /// audio-only when no dimensions are set, video+audio otherwise.
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
    /// SourceBuffer.
    #[test]
    fn content_type_includes_opus_when_video_dims_set() {
        let node = muxer_with_dims(1280, 720);
        let Some(ct) = node.content_type() else {
            panic!("content_type should return Some");
        };
        assert_eq!(ct, "video/webm; codecs=\"vp9,opus\"");
    }

    #[test]
    fn input_pins_default_single() {
        let node = muxer_with_dims(0, 0);
        assert_eq!(node.input_pins().len(), 1);
    }

    #[test]
    fn input_pins_dual_with_num_inputs() {
        let node =
            WebMMuxerNode::new(WebMMuxerConfig { num_inputs: 2, ..WebMMuxerConfig::default() });
        let pins = node.input_pins();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "in");
        assert_eq!(pins[1].name, "in_1");
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

    /// Verify that the `webm` crate in Live mode with a non-seek writer
    /// produces Cluster elements (element ID `0x1F43B675`).
    ///
    /// The MSE spec requires SimpleBlock elements to be nested inside Clusters.
    /// If libwebm doesn't produce Clusters, the MSE SourceBuffer will reject
    /// the stream with `CHUNK_DEMUXER_ERROR_APPEND_FAILED`.
    #[test]
    fn webm_live_non_seek_produces_clusters() {
        use std::io::Cursor;
        use webm::mux::{SegmentBuilder, SegmentMode, VideoCodecId, Writer};

        let output = Vec::new();
        let writer = Writer::new_non_seek(Cursor::new(output));

        let builder = SegmentBuilder::new(writer).unwrap();
        let builder = builder.set_mode(SegmentMode::Live).unwrap();
        let (builder, video_track) =
            builder.add_video_track(1280, 720, VideoCodecId::VP9, None).unwrap();
        let mut segment = builder.build();

        // Add a few frames — the first one is a keyframe.
        let fake_frame = vec![0u8; 100];
        segment.add_frame(video_track, &fake_frame, 0, true).unwrap();
        segment.add_frame(video_track, &fake_frame, 33_000_000, false).unwrap();
        segment.add_frame(video_track, &fake_frame, 66_000_000, true).unwrap();

        // Finalize (may fail for non-seek, that's ok — we just want the
        // bytes written so far).
        let writer = segment.finalize(None).unwrap_or_else(|w| w);
        let data = writer.into_inner().into_inner();

        // Search for Cluster ID (0x1F43B675).
        let cluster_id: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];
        let has_cluster = data.windows(4).any(|w| w == cluster_id);

        assert!(
            has_cluster,
            "webm crate in Live mode with non-seek writer must produce at least one Cluster element. \
             Output size: {} bytes. First 200 bytes: {:02x?}",
            data.len(),
            &data[..data.len().min(200)]
        );
    }

    // ---------------------------------------------------------------
    // Timestamp logic unit tests
    // ---------------------------------------------------------------

    /// Helper: stage N frames on a single track via `stage_frame` and
    /// return their rebased timestamps in nanoseconds.
    #[allow(clippy::too_many_arguments)]
    fn stage_n(
        n: usize,
        track_id: u64,
        duration_us: u64,
        incoming_timestamps: &[Option<u64>],
        clock: &mut streamkit_core::timing::MediaClock,
        rebase_offset: &mut Option<i64>,
        last_written_ns: u64,
        per_track_last: &mut Option<u64>,
    ) -> Vec<u64> {
        let mut timestamps = Vec::with_capacity(n);
        for i in 0..n {
            let meta = incoming_timestamps.get(i).copied().flatten().map(|ts| PacketMetadata {
                timestamp_us: Some(ts),
                duration_us: Some(duration_us),
                sequence: None,
                keyframe: None,
            });
            let pf = stage_frame(
                Bytes::from_static(&[0]),
                meta,
                track_id,
                i == 0,
                duration_us,
                clock,
                rebase_offset,
                last_written_ns,
                per_track_last,
            );
            timestamps.push(pf.timestamp_ns);
        }
        timestamps
    }

    #[test]
    fn stage_frame_synthetic_clock_produces_clean_cadence() {
        let mut clock = streamkit_core::timing::MediaClock::new(0);
        let mut offset = None;
        let mut last = None;
        // Audio: 20ms frames, no incoming timestamps (synthetic clock)
        let ts = stage_n(
            5,
            2,
            20_000,
            &[None, None, None, None, None],
            &mut clock,
            &mut offset,
            0,
            &mut last,
        );
        // Expect 0, 20ms, 40ms, 60ms, 80ms in nanoseconds
        assert_eq!(ts, vec![0, 20_000_000, 40_000_000, 60_000_000, 80_000_000]);
    }

    #[test]
    fn stage_frame_incoming_timestamps_used_when_present() {
        let mut clock = streamkit_core::timing::MediaClock::new(0);
        let mut offset = None;
        let mut last = None;
        // Video with explicit timestamps (from compositor/MoQ)
        let ts = stage_n(
            3,
            1,
            33_333,
            &[Some(0), Some(33_333), Some(66_666)],
            &mut clock,
            &mut offset,
            0,
            &mut last,
        );
        assert_eq!(ts, vec![0, 33_333_000, 66_666_000]);
    }

    #[test]
    fn stage_frame_per_track_monotonicity_clamps_backward() {
        let mut clock = streamkit_core::timing::MediaClock::new(0);
        let mut offset = None;
        let mut last = None;
        // Timestamps go backward (jittery MoQ source).
        // Rebase: first ts=100000us, last_written=0 → offset=-100ms.
        // All timestamps are shifted by -100ms then clamped to ≥0.
        let ts = stage_n(
            4,
            1,
            33_333,
            &[Some(100_000), Some(90_000), Some(80_000), Some(200_000)],
            &mut clock,
            &mut offset,
            0,
            &mut last,
        );
        // Frame 1: 100ms - 100ms = 0ns
        assert_eq!(ts[0], 0);
        // Frame 2: 90ms - 100ms = -10ms → clamped to 0 → per-track: 0 ≤ 0 → +1ms
        assert_eq!(ts[1], 1_000_000);
        // Frame 3: 80ms - 100ms = -20ms → clamped to 0 → per-track: 0 ≤ 1ms → +1ms = 2ms
        assert_eq!(ts[2], 2_000_000);
        // Frame 4: 200ms - 100ms = 100ms → per-track: 100ms > 2ms → OK
        assert_eq!(ts[3], 100_000_000);
    }

    #[test]
    fn stage_frame_rebase_aligns_late_track() {
        // Video has been writing for 3 seconds
        let video_last_written = 3_000_000_000u64; // 3s in ns

        // Audio arrives late, synthetic clock starts at 0
        let mut audio_clock = streamkit_core::timing::MediaClock::new(0);
        let mut audio_offset = None;
        let mut audio_last = None;

        let ts = stage_n(
            3,
            2,
            20_000,
            &[None, None, None],
            &mut audio_clock,
            &mut audio_offset,
            video_last_written,
            &mut audio_last,
        );
        // Audio should start at ~3s (video's current position)
        assert_eq!(ts[0], 3_000_000_000);
        assert_eq!(ts[1], 3_020_000_000);
        assert_eq!(ts[2], 3_040_000_000);
        assert_eq!(audio_offset, Some(3_000_000_000));
    }

    /// Simulate 2 seconds of interleaved audio (50fps) + video (30fps)
    /// arriving in arrival order.  Verify container timestamps are
    /// well-formed: globally non-decreasing, no 1ms pairs (the original
    /// stuttering bug), no huge gaps.
    #[test]
    fn interleaved_av_no_large_gaps_or_1ms_pairs() {
        let mut audio_clock = streamkit_core::timing::MediaClock::new(0);
        let mut video_clock = streamkit_core::timing::MediaClock::new(0);
        let mut audio_offset: Option<i64> = Some(0);
        let mut video_offset: Option<i64> = Some(0);
        let mut audio_last: Option<u64> = None;
        let mut video_last: Option<u64> = None;
        let mut last_written_ns: u64 = 0;

        let mut audio_container_ts: Vec<u64> = Vec::new();
        let mut video_container_ts: Vec<u64> = Vec::new();

        // Generate 2s of frames: audio every 20ms, video every 33ms.
        let total_ms = 2000u64;
        let mut next_audio_ms = 0u64;
        let mut next_video_ms = 0u64;

        while next_audio_ms < total_ms || next_video_ms < total_ms {
            if next_audio_ms <= next_video_ms && next_audio_ms < total_ms {
                let pf = stage_frame(
                    Bytes::from_static(&[0]),
                    None,
                    2,
                    true,
                    20_000,
                    &mut audio_clock,
                    &mut audio_offset,
                    last_written_ns,
                    &mut audio_last,
                );
                let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
                audio_container_ts.push(write_ts);
                last_written_ns = write_ts;
                next_audio_ms += 20;
            } else if next_video_ms < total_ms {
                let pf = stage_frame(
                    Bytes::from_static(&[0]),
                    None,
                    1,
                    next_video_ms == 0,
                    33_333,
                    &mut video_clock,
                    &mut video_offset,
                    last_written_ns,
                    &mut video_last,
                );
                let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
                video_container_ts.push(write_ts);
                last_written_ns = write_ts;
                next_video_ms += 33;
            }
        }

        // Per-track monotonicity
        for w in audio_container_ts.windows(2) {
            assert!(w[1] >= w[0], "audio went backward: {} -> {}", w[0], w[1]);
        }
        for w in video_container_ts.windows(2) {
            assert!(w[1] >= w[0], "video went backward: {} -> {}", w[0], w[1]);
        }

        // No 1ms pairs on audio (the original stuttering bug)
        for w in audio_container_ts.windows(2) {
            let gap_ms = (w[1] - w[0]) / 1_000_000;
            assert!(
                gap_ms >= 5,
                "audio gap too small: {}ms (ts {} -> {}). Likely 1ms-pair regression.",
                gap_ms,
                w[0],
                w[1]
            );
        }

        // No huge gaps (> 2x frame duration) on either track
        for w in audio_container_ts.windows(2) {
            let gap_ms = (w[1] - w[0]) / 1_000_000;
            assert!(gap_ms <= 45, "audio gap too large: {}ms (ts {} -> {})", gap_ms, w[0], w[1]);
        }
        for w in video_container_ts.windows(2) {
            let gap_ms = (w[1] - w[0]) / 1_000_000;
            assert!(gap_ms <= 70, "video gap too large: {}ms (ts {} -> {})", gap_ms, w[0], w[1]);
        }

        // Reasonable frame counts
        assert!(audio_container_ts.len() >= 95, "too few audio: {}", audio_container_ts.len());
        assert!(video_container_ts.len() >= 55, "too few video: {}", video_container_ts.len());
    }

    /// Simulate arrival-order interleaving where video frames arrive before
    /// audio frames that have earlier staged timestamps.  Without strictly
    /// increasing write timestamps, the global max-clamp creates cross-track
    /// timestamp equality (e.g. video@40.032, audio@40.032, audio@40.032),
    /// which causes Chrome's muxed-WebM MSE demuxer to stall.
    #[test]
    fn arrival_order_produces_unique_global_timestamps() {
        let mut video_clock = streamkit_core::timing::MediaClock::new(0);
        let mut audio_clock = streamkit_core::timing::MediaClock::new(0);
        let mut video_offset: Option<i64> = None;
        let mut audio_offset: Option<i64> = None;
        let mut video_last: Option<u64> = None;
        let mut audio_last: Option<u64> = None;
        let mut last_written_ns: u64 = 0;

        // Collect all global write timestamps in order.
        let mut all_write_ts: Vec<u64> = Vec::new();

        // Simulate 2 seconds of arrival-order interleaving:
        // Video arrives at muxer first (compositor has lower latency),
        // then audio for the same time window arrives slightly later.
        // This matches the real pattern: V, V, A, A, V, A, A, V, ...
        let total_ms = 2000u64;
        let mut next_video_ms: u64 = 0;
        let mut next_audio_ms: u64 = 0;

        while next_video_ms < total_ms || next_audio_ms < total_ms {
            // Write video first (simulating lower-latency compositor path)
            if next_video_ms < total_ms && next_video_ms <= next_audio_ms + 10 {
                let pf = stage_frame(
                    Bytes::from_static(&[0]),
                    None,
                    1,
                    next_video_ms == 0,
                    33_333,
                    &mut video_clock,
                    &mut video_offset,
                    last_written_ns,
                    &mut video_last,
                );
                let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
                all_write_ts.push(write_ts);
                last_written_ns = write_ts;
                next_video_ms += 33;
            }

            // Then write any audio frames whose timestamps fall before
            // the next video frame (simulating later arrival).
            while next_audio_ms < total_ms && next_audio_ms < next_video_ms {
                let pf = stage_frame(
                    Bytes::from_static(&[0]),
                    None,
                    2,
                    true,
                    20_000,
                    &mut audio_clock,
                    &mut audio_offset,
                    last_written_ns,
                    &mut audio_last,
                );
                let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
                all_write_ts.push(write_ts);
                last_written_ns = write_ts;
                next_audio_ms += 20;
            }
        }

        // Every global write timestamp must be unique (strictly increasing).
        for w in all_write_ts.windows(2) {
            assert!(
                w[1] > w[0],
                "global timestamps must be strictly increasing: {} -> {} (equal = MSE stall risk)",
                w[0],
                w[1]
            );
        }

        // Sanity: reasonable frame counts.
        assert!(all_write_ts.len() >= 150, "too few frames: {}", all_write_ts.len());
    }

    #[test]
    fn rebase_with_incoming_moq_timestamps() {
        // Video track starts first at ts=0 (from compositor)
        let mut video_clock = streamkit_core::timing::MediaClock::new(0);
        let mut video_offset = None;
        let mut video_last = None;

        let v1 = stage_frame(
            Bytes::from_static(&[0]),
            Some(PacketMetadata {
                timestamp_us: Some(0),
                duration_us: Some(33_333),
                sequence: None,
                keyframe: Some(true),
            }),
            1,
            true,
            33_333,
            &mut video_clock,
            &mut video_offset,
            0,
            &mut video_last,
        );
        assert_eq!(v1.timestamp_ns, 0);

        // Simulate 3s of video written
        let last_video_ns = 3_000_000_000u64;

        // Audio arrives with normalized MoQ timestamp
        let mut audio_clock = streamkit_core::timing::MediaClock::new(0);
        let mut audio_offset = None;
        let mut audio_last = None;

        let a1 = stage_frame(
            Bytes::from_static(&[0]),
            Some(PacketMetadata {
                timestamp_us: Some(20_000),
                duration_us: Some(20_000),
                sequence: None,
                keyframe: None,
            }),
            2,
            true,
            20_000,
            &mut audio_clock,
            &mut audio_offset,
            last_video_ns,
            &mut audio_last,
        );

        // Audio should be rebased to near the video's current position
        let audio_start_ms = a1.timestamp_ns / 1_000_000;
        assert!(
            (2990..=3010).contains(&audio_start_ms),
            "audio should start near 3000ms, got {audio_start_ms}ms"
        );
    }

    /// Simulate the compositor calibration scenario that causes permanent
    /// A/V desync without the rebase-reset fix.
    ///
    /// Timeline:
    /// 1. Compositor outputs pre-calibration video at running-clock
    ///    timestamps (0, 33ms, 66ms, …) for 2 seconds.
    /// 2. Audio arrives ~2s later; its rebase aligns it to the current
    ///    video position (~2s).
    /// 3. Compositor calibrates to the MoQ epoch — output timestamps
    ///    jump backwards from ~2s to ~0.
    /// 4. Without the rebase-reset, per-track monotonicity bumps each
    ///    post-calibration video frame by +1ms, creating a permanent
    ///    ~2s A/V offset.  With the fix, the rebase recomputes from
    ///    `last_written_ns` and the tracks re-align.
    #[test]
    fn rebase_reset_on_compositor_calibration_prevents_av_desync() {
        let mut video_clock = streamkit_core::timing::MediaClock::new(0);
        let mut audio_clock = streamkit_core::timing::MediaClock::new(0);
        let mut video_offset: Option<i64> = None;
        let mut audio_offset: Option<i64> = None;
        let mut video_last: Option<u64> = None;
        let mut audio_last: Option<u64> = None;
        let mut last_written_ns: u64 = 0;

        // ── Phase 1: 60 pre-calibration video frames (≈2s at 30fps) ──
        for i in 0u64..60 {
            let ts_us = i * 33_333;
            let pf = stage_frame(
                Bytes::from_static(&[0]),
                Some(PacketMetadata {
                    timestamp_us: Some(ts_us),
                    duration_us: Some(33_333),
                    sequence: None,
                    keyframe: Some(i == 0),
                }),
                1,
                i == 0,
                33_333,
                &mut video_clock,
                &mut video_offset,
                last_written_ns,
                &mut video_last,
            );
            last_written_ns = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
        }
        // Video should be near 2s
        let video_pre_cal_ns = last_written_ns;
        assert!(
            video_pre_cal_ns > 1_900_000_000 && video_pre_cal_ns < 2_100_000_000,
            "pre-calibration video should be near 2s, got {}ms",
            video_pre_cal_ns / 1_000_000
        );

        // ── Phase 2: Audio arrives, rebased to current video position ──
        for i in 0u64..10 {
            let ts_us = i * 20_000; // MoQ-normalized, starts near 0
            let pf = stage_frame(
                Bytes::from_static(&[0]),
                Some(PacketMetadata {
                    timestamp_us: Some(ts_us),
                    duration_us: Some(20_000),
                    sequence: None,
                    keyframe: None,
                }),
                2,
                true,
                20_000,
                &mut audio_clock,
                &mut audio_offset,
                last_written_ns,
                &mut audio_last,
            );
            last_written_ns = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
        }

        // ── Phase 3: Compositor calibrates — video timestamps jump back
        //    to near 0 (MoQ epoch) ──
        // This simulates the compositor output after cal_offset is applied:
        // output_ts = running_clock + cal_offset ≈ 2s + (-2s) = 0.
        let mut post_cal_video_ts = Vec::new();
        for i in 0u64..30 {
            let ts_us = i * 33_333; // post-calibration: back near 0
            let pf = stage_frame(
                Bytes::from_static(&[0]),
                Some(PacketMetadata {
                    timestamp_us: Some(ts_us),
                    duration_us: Some(33_333),
                    sequence: None,
                    keyframe: Some(i == 0),
                }),
                1,
                i == 0,
                33_333,
                &mut video_clock,
                &mut video_offset,
                last_written_ns,
                &mut video_last,
            );
            let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
            post_cal_video_ts.push(write_ts);
            last_written_ns = write_ts;
        }

        // ── Phase 4: Continue audio for comparison ──
        let mut post_cal_audio_ts = Vec::new();
        for i in 10u64..40 {
            let ts_us = i * 20_000;
            let pf = stage_frame(
                Bytes::from_static(&[0]),
                Some(PacketMetadata {
                    timestamp_us: Some(ts_us),
                    duration_us: Some(20_000),
                    sequence: None,
                    keyframe: None,
                }),
                2,
                true,
                20_000,
                &mut audio_clock,
                &mut audio_offset,
                last_written_ns,
                &mut audio_last,
            );
            let write_ts = strictly_increasing_ts(pf.timestamp_ns, last_written_ns);
            post_cal_audio_ts.push(write_ts);
            last_written_ns = write_ts;
        }

        // ── Assertions ──
        // After the rebase reset, post-calibration video should quickly
        // re-align with audio.  The last video and audio timestamps
        // should be within 500ms of each other (not the ~2s permanent
        // offset that would exist without the fix).
        let last_video = *post_cal_video_ts.last().unwrap();
        let last_audio = *post_cal_audio_ts.last().unwrap();
        let offset_ms = last_audio.abs_diff(last_video) / 1_000_000;
        assert!(
            offset_ms < 500,
            "A/V offset after calibration should be < 500ms, got {offset_ms}ms \
             (audio={last_audio}ns, video={last_video}ns). \
             Rebase reset may not have fired."
        );

        // Post-calibration video timestamps must be strictly increasing
        for w in post_cal_video_ts.windows(2) {
            assert!(w[1] > w[0], "post-cal video went backward: {} -> {}", w[0], w[1]);
        }
    }

    /// Confirm libwebm rejects cross-track backward timestamps — this is
    /// why `write_frame` enforces strictly increasing timestamps with a
    /// +1 ms bump when a frame would otherwise equal or precede the
    /// previous write.
    #[test]
    fn libwebm_rejects_cross_track_non_monotonic() {
        use std::io::Cursor;
        use webm::mux::{AudioCodecId, SegmentBuilder, SegmentMode, VideoCodecId, Writer};

        let writer = Writer::new_non_seek(Cursor::new(Vec::new()));
        let builder = SegmentBuilder::new(writer).unwrap();
        let builder = builder.set_mode(SegmentMode::Live).unwrap();
        let (builder, video) = builder.add_video_track(64, 64, VideoCodecId::VP9, None).unwrap();
        let (builder, audio) = builder.add_audio_track(48000, 1, AudioCodecId::Opus, None).unwrap();
        let mut segment = builder.build();
        let f = vec![0u8; 10];

        segment.add_frame(video, &f, 0, true).unwrap();
        segment.add_frame(video, &f, 66_000_000, false).unwrap();
        // Audio at 60ms after video at 66ms — backward, should fail.
        let result = segment.add_frame(audio, &f, 60_000_000, false);
        assert!(result.is_err(), "libwebm must reject backward timestamps");
    }
}
