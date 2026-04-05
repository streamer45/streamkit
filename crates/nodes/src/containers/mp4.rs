// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MP4 container muxer node.
//!
//! Supports two modes:
//! - **Stream** (fMP4): Uses [`shiguredo_mp4::mux::Fmp4SegmentMuxer`] to produce
//!   fragmented MP4 segments that are sent downstream immediately.  Memory stays
//!   bounded because each segment is drained after creation.
//! - **File** (regular MP4): Uses [`shiguredo_mp4::mux::Mp4FileMuxer`] to write
//!   media data to a temporary file on disk.  Only metadata (sample tables, chunk
//!   offsets) is kept in memory.  At finalization the file is patched with the moov
//!   box and read back for a single downstream send.
//!
//! Codec support: H.264 (AVC) video + AAC/Opus audio.  Additional codecs (AV1,
//! VP9) can be added by extending the sample-entry construction helpers.

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::Deserialize;
use shiguredo_mp4::boxes::{
    AudioSampleEntryFields, Avc1Box, AvccBox, DopsBox, EsdsBox, Mp4aBox, OpusBox, SampleEntry,
    VisualSampleEntryFields,
};
use shiguredo_mp4::descriptors::{
    DecoderConfigDescriptor, DecoderSpecificInfo, EsDescriptor, SlConfigDescriptor,
};
use shiguredo_mp4::mux::{Fmp4SegmentMuxer, Mp4FileMuxer, Sample};
use shiguredo_mp4::{FixedPointNumber, TrackKind, Uint};
use std::io::{BufWriter, Read as _, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketMetadata, PacketType,
    VideoCodec,
};
use streamkit_core::{
    state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};

use crate::video::DEFAULT_VIDEO_FRAME_DURATION_US;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default audio frame duration when metadata is missing (20 ms Opus frame).
const DEFAULT_AUDIO_FRAME_DURATION_US: u64 = 20_000;

/// Default video timescale (90 kHz — standard for MPEG transport streams / MP4).
const DEFAULT_VIDEO_TIMESCALE: NonZeroU32 = match NonZeroU32::new(90_000) {
    Some(v) => v,
    None => unreachable!(),
};

/// Default audio timescale for Opus (48 kHz).
const DEFAULT_AUDIO_TIMESCALE_OPUS: NonZeroU32 = match NonZeroU32::new(48_000) {
    Some(v) => v,
    None => unreachable!(),
};

/// Number of samples per fMP4 segment before flushing downstream.
const FMP4_SEGMENT_FLUSH_THRESHOLD: usize = 30;

// ---------------------------------------------------------------------------
// Sample entry construction helpers
// ---------------------------------------------------------------------------

/// Build an AVC1 (H.264) sample entry box from SPS/PPS NAL units.
///
/// If `codec_private` is available, it is expected to contain an AVCDecoderConfigurationRecord
/// or raw SPS/PPS NAL units.  Otherwise a minimal placeholder is used.
fn build_avc1_sample_entry(width: u16, height: u16, codec_private: Option<&[u8]>) -> SampleEntry {
    let (sps_list, pps_list, profile, compat, level) = codec_private.map_or_else(
        || (vec![vec![0x67, 0x42, 0xc0, 0x1e]], vec![vec![0x68, 0xce, 0x38, 0x80]], 66, 0, 30),
        parse_avcc_codec_private,
    );

    SampleEntry::Avc1(Avc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        avcc_box: AvccBox {
            avc_profile_indication: profile,
            profile_compatibility: compat,
            avc_level_indication: level,
            length_size_minus_one: Uint::new(3),
            sps_list,
            pps_list,
            chroma_format: None,
            bit_depth_luma_minus8: None,
            bit_depth_chroma_minus8: None,
            sps_ext_list: vec![],
        },
        unknown_boxes: vec![],
    })
}

/// Parse an AVCDecoderConfigurationRecord (or raw SPS+PPS) from `codec_private`.
///
/// Returns `(sps_list, pps_list, profile_idc, profile_compat, level_idc)`.
fn parse_avcc_codec_private(data: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u8, u8, u8) {
    // AVCDecoderConfigurationRecord starts with version=1
    if data.len() >= 7 && data[0] == 1 {
        let profile = data[1];
        let compat = data[2];
        let level = data[3];
        let mut offset = 5;

        // SPS
        let sps_count = (data.get(offset).copied().unwrap_or(0)) & 0x1F;
        offset += 1;
        let mut sps_list = Vec::new();
        for _ in 0..sps_count {
            if offset + 2 > data.len() {
                break;
            }
            let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
            if offset + len > data.len() {
                break;
            }
            sps_list.push(data[offset..offset + len].to_vec());
            offset += len;
        }

        // PPS
        let pps_count = data.get(offset).copied().unwrap_or(0);
        offset += 1;
        let mut pps_list = Vec::new();
        for _ in 0..pps_count {
            if offset + 2 > data.len() {
                break;
            }
            let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
            if offset + len > data.len() {
                break;
            }
            pps_list.push(data[offset..offset + len].to_vec());
            offset += len;
        }

        if sps_list.is_empty() {
            sps_list.push(vec![0x67, 0x42, 0xc0, 0x1e]);
        }
        if pps_list.is_empty() {
            pps_list.push(vec![0x68, 0xce, 0x38, 0x80]);
        }

        (sps_list, pps_list, profile, compat, level)
    } else {
        // Fallback: treat as raw SPS data
        (vec![data.to_vec()], vec![vec![0x68, 0xce, 0x38, 0x80]], 66, 0, 30)
    }
}

/// Build an mp4a (AAC) sample entry box.
///
/// Constructs a minimal ESDS descriptor for AAC-LC with the given sample rate
/// and channel count.
fn build_mp4a_sample_entry(sample_rate: u32, channels: u16) -> SampleEntry {
    // Build a minimal AudioSpecificConfig for AAC-LC.
    // Format: 5 bits object type (2 = AAC-LC) + 4 bits freq index + 4 bits channel config
    let freq_index: u8 = match sample_rate {
        96_000 => 0,
        88_200 => 1,
        64_000 => 2,
        44_100 => 4,
        32_000 => 5,
        24_000 => 6,
        22_050 => 7,
        16_000 => 8,
        12_000 => 9,
        11_025 => 10,
        8_000 => 11,
        // 48 kHz and all other rates default to index 3
        _ => 3,
    };
    let channel_config = u8::try_from(channels).unwrap_or(2);
    // AudioSpecificConfig: objectType=2 (AAC-LC), frequencyIndex, channelConfiguration
    // 5 bits + 4 bits + 4 bits = 13 bits → 2 bytes
    let byte0 = (2u8 << 3) | (freq_index >> 1);
    let byte1 = (freq_index << 7) | (channel_config << 3);
    let audio_specific_config = vec![byte0, byte1];

    SampleEntry::Mp4a(Mp4aBox {
        audio: AudioSampleEntryFields {
            data_reference_index: AudioSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            channelcount: channels,
            samplesize: AudioSampleEntryFields::DEFAULT_SAMPLESIZE,
            samplerate: FixedPointNumber::new(u16::try_from(sample_rate).unwrap_or(48000), 0),
        },
        esds_box: EsdsBox {
            es: EsDescriptor {
                es_id: EsDescriptor::MIN_ES_ID,
                stream_priority: EsDescriptor::LOWEST_STREAM_PRIORITY,
                depends_on_es_id: None,
                url_string: None,
                ocr_es_id: None,
                dec_config_descr: DecoderConfigDescriptor {
                    object_type_indication:
                        DecoderConfigDescriptor::OBJECT_TYPE_INDICATION_AUDIO_ISO_IEC_14496_3,
                    stream_type: DecoderConfigDescriptor::STREAM_TYPE_AUDIO,
                    up_stream: DecoderConfigDescriptor::UP_STREAM_FALSE,
                    buffer_size_db: Uint::new(0),
                    max_bitrate: 0,
                    avg_bitrate: 0,
                    dec_specific_info: Some(DecoderSpecificInfo { payload: audio_specific_config }),
                },
                sl_config_descr: SlConfigDescriptor,
            },
        },
        unknown_boxes: vec![],
    })
}

/// Build an Opus sample entry box.
fn build_opus_sample_entry(sample_rate: u32, channels: u16) -> SampleEntry {
    SampleEntry::Opus(OpusBox {
        audio: AudioSampleEntryFields {
            data_reference_index: AudioSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            channelcount: channels,
            samplesize: AudioSampleEntryFields::DEFAULT_SAMPLESIZE,
            samplerate: FixedPointNumber::new(u16::try_from(sample_rate).unwrap_or(48000), 0),
        },
        dops_box: DopsBox {
            output_channel_count: u8::try_from(channels).unwrap_or(2),
            pre_skip: 312,
            input_sample_rate: sample_rate,
            output_gain: 0,
        },
        unknown_boxes: vec![],
    })
}

// ---------------------------------------------------------------------------
// File-backed buffer (reused pattern from WebM muxer)
// ---------------------------------------------------------------------------

/// A file-backed buffer for **File** mode MP4 muxing.
///
/// All writes go to an anonymous temporary file on disk so the muxer can
/// seek/backpatch without accumulating the entire output in memory.
struct FileBackedBuffer {
    inner: BufWriter<std::fs::File>,
}

impl FileBackedBuffer {
    fn new() -> std::io::Result<Self> {
        let file = tempfile::tempfile()?;
        Ok(Self { inner: BufWriter::new(file) })
    }

    /// Read the entire temp file contents as `Bytes`.
    fn take_data(&mut self) -> std::io::Result<Option<Bytes>> {
        self.inner.flush()?;
        let file = self.inner.get_mut();
        let len = file.seek(SeekFrom::End(0))?;
        if len == 0 {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(0))?;
        let len_usize = usize::try_from(len).map_err(std::io::Error::other)?;
        let mut buf = vec![0u8; len_usize];
        file.read_exact(&mut buf)?;
        Ok(Some(Bytes::from(buf)))
    }

    /// Current write position in the file.
    fn position(&mut self) -> std::io::Result<u64> {
        self.inner.flush()?;
        self.inner.get_mut().stream_position()
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

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// MP4 muxer streaming mode.
#[derive(Deserialize, Debug, Default, Clone, Copy, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mp4StreamingMode {
    /// Fragmented MP4 (fMP4) mode — produces segments suitable for DASH/HLS
    /// streaming.  Each segment is sent downstream immediately.
    #[default]
    Stream,
    /// Regular MP4 file mode — writes to a temp file and sends the complete
    /// file after finalization.  Supports fast-start (moov before mdat).
    File,
}

/// Configuration for the MP4 muxer node.
#[derive(Deserialize, Debug, JsonSchema)]
#[serde(default)]
pub struct Mp4MuxerConfig {
    /// Streaming mode: `"stream"` for fMP4 segments, `"file"` for regular MP4.
    pub mode: Mp4StreamingMode,

    /// Video width in pixels (used for sample entry construction).
    pub video_width: u16,

    /// Video height in pixels (used for sample entry construction).
    pub video_height: u16,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u16,

    /// Video timescale (ticks per second).  Default: 90000.
    pub video_timescale: u32,

    /// Audio timescale (ticks per second).  Default: 48000.
    pub audio_timescale: u32,

    /// Number of input pins (1 or 2).
    #[serde(default = "default_num_inputs")]
    #[schemars(range(min = 1, max = 2))]
    pub num_inputs: u32,
}

const fn default_num_inputs() -> u32 {
    1
}

impl Default for Mp4MuxerConfig {
    fn default() -> Self {
        Self {
            mode: Mp4StreamingMode::default(),
            video_width: 0,
            video_height: 0,
            sample_rate: 48_000,
            channels: 2,
            video_timescale: DEFAULT_VIDEO_TIMESCALE.get(),
            audio_timescale: DEFAULT_AUDIO_TIMESCALE_OPUS.get(),
            num_inputs: default_num_inputs(),
        }
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// Frame variant received from input channels.
enum MuxFrame {
    Audio(Bytes, Option<PacketMetadata>),
    Video(Bytes, Option<PacketMetadata>),
    AudioClosed,
    VideoClosed,
    Shutdown,
}

/// Classify a [`Packet`] as audio or video from its `content_type` field.
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

/// Determine the MP4 MIME content-type string.
const fn mp4_content_type(has_audio: bool, has_video: bool, audio_is_opus: bool) -> &'static str {
    match (has_audio, has_video, audio_is_opus) {
        (true, true, false) => "video/mp4; codecs=\"avc1,mp4a\"",
        (true, true, true) => "video/mp4; codecs=\"avc1,opus\"",
        (false, true, _) => "video/mp4; codecs=\"avc1\"",
        (true, false, false) => "audio/mp4; codecs=\"mp4a\"",
        (true, false, true) => "audio/mp4; codecs=\"opus\"",
        (false, false, _) => "video/mp4",
    }
}

/// A node that muxes encoded H.264 video and/or AAC/Opus audio into an MP4 container.
///
/// Supports two modes:
/// - **Stream** (fMP4): produces fragmented segments sent downstream immediately.
/// - **File**: writes to a temp file on disk and sends the finalized MP4 once.
pub struct Mp4MuxerNode {
    config: Mp4MuxerConfig,
}

impl Mp4MuxerNode {
    pub const fn new(config: Mp4MuxerConfig) -> Self {
        Self { config }
    }
}

/// Which tracks are present and whether classification was skipped.
#[derive(Clone, Copy)]
struct TrackPresence {
    audio: bool,
    video: bool,
    skip_classification: bool,
}

/// Mutable per-track completion flags.
struct TrackProgress {
    audio_done: bool,
    video_done: bool,
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl ProcessorNode for Mp4MuxerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        let media_types = vec![
            PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::H264,
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
        let has_video = self.config.video_width > 0 && self.config.video_height > 0;
        Some(mp4_content_type(true, has_video, false).to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!("Mp4MuxerNode starting");

        if context.inputs.is_empty() {
            let err_msg = "Mp4MuxerNode requires at least one input (audio or video)".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        // ---- Classify inputs (same pattern as WebM muxer) ----

        let skip_classification = self.config.num_inputs >= 2
            && self.config.video_width > 0
            && self.config.video_height > 0;

        let mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut audio_codec = AudioCodec::Opus;
        let mut all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>> = Vec::new();

        // Resolve input types from engine or pin management messages.
        let mut input_types = std::mem::take(&mut context.input_types);
        let num_inputs = context.inputs.len();
        if input_types.is_empty() {
            if let Some(ref mut pin_mgmt_rx) = context.pin_management_rx {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                while input_types.len() < num_inputs {
                    tokio::select! {
                        msg = pin_mgmt_rx.recv() => {
                            match msg {
                                Some(PinManagementMessage::InputTypeResolved {
                                    pin_name,
                                    packet_type,
                                }) => {
                                    input_types.insert(pin_name, packet_type);
                                },
                                Some(_) => {},
                                None => break,
                            }
                        }
                        () = tokio::time::sleep_until(deadline) => {
                            tracing::warn!(
                                "Mp4MuxerNode: timed out waiting for InputTypeResolved \
                                 ({}/{} resolved)",
                                input_types.len(),
                                num_inputs,
                            );
                            break;
                        }
                    }
                }
            }
        }

        for (pin_name, rx) in context.inputs.drain() {
            let pin_type = input_types.get(&pin_name);
            let is_video = pin_type.is_some_and(|ty| {
                matches!(ty, PacketType::EncodedVideo(_) | PacketType::RawVideo(_))
            });

            if pin_type.is_none() && !skip_classification {
                tracing::warn!(
                    "Mp4MuxerNode: pin '{pin_name}' has no resolved type, \
                     defaulting to audio"
                );
            }

            // Detect audio codec from type info.
            if let Some(PacketType::EncodedAudio(fmt)) = pin_type {
                audio_codec = fmt.codec;
            }

            if skip_classification {
                all_receivers.push(rx);
            } else if is_video {
                if video_rx.is_some() {
                    let err_msg = format!("Mp4MuxerNode: multiple video inputs (pin '{pin_name}')");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
                tracing::info!("Mp4MuxerNode: pin '{pin_name}' classified as VIDEO");
                video_rx = Some(rx);
            } else {
                if audio_rx.is_some() {
                    let err_msg = format!("Mp4MuxerNode: multiple audio inputs (pin '{pin_name}')");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }
                tracing::info!("Mp4MuxerNode: pin '{pin_name}' classified as AUDIO");
                audio_rx = Some(rx);
            }
        }

        let has_audio = if skip_classification { true } else { audio_rx.is_some() };
        let has_video = if skip_classification { true } else { video_rx.is_some() };

        if !has_audio && !has_video {
            let err_msg = "Mp4MuxerNode: no inputs classified as audio or video".to_string();
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        state_helpers::emit_running(&context.state_tx, &node_name);

        let audio_is_opus = matches!(audio_codec, AudioCodec::Opus);
        let content_type_str = mp4_content_type(has_audio, has_video, audio_is_opus);

        tracing::info!(
            "Mp4MuxerNode tracks: audio={has_audio} video={has_video} \
             mode={:?} content_type={content_type_str}",
            self.config.mode,
        );

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // ---- Dispatch to mode-specific muxing logic ----

        match self.config.mode {
            Mp4StreamingMode::Stream => {
                run_stream_mode(
                    &self.config,
                    &mut context,
                    &node_name,
                    content_type_str,
                    &mut stats_tracker,
                    audio_codec,
                    audio_rx,
                    video_rx,
                    all_receivers,
                    TrackPresence { audio: has_audio, video: has_video, skip_classification },
                    TrackProgress { audio_done: false, video_done: false },
                )
                .await?;
            },
            Mp4StreamingMode::File => {
                run_file_mode(
                    &self.config,
                    &mut context,
                    &node_name,
                    content_type_str,
                    &mut stats_tracker,
                    audio_codec,
                    audio_rx,
                    video_rx,
                    all_receivers,
                    TrackPresence { audio: has_audio, video: has_video, skip_classification },
                    TrackProgress { audio_done: false, video_done: false },
                )
                .await?;
            },
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("Mp4MuxerNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve timescale values from config, falling back to compile-time constants.
fn resolve_timescales(config: &Mp4MuxerConfig) -> (NonZeroU32, NonZeroU32) {
    let video_ts = NonZeroU32::new(config.video_timescale).unwrap_or(DEFAULT_VIDEO_TIMESCALE);
    let audio_ts = NonZeroU32::new(config.audio_timescale).unwrap_or(DEFAULT_AUDIO_TIMESCALE_OPUS);
    (video_ts, audio_ts)
}

/// Build codec-specific sample entries from config and detected audio codec.
fn build_sample_entries(
    config: &Mp4MuxerConfig,
    audio_codec: AudioCodec,
) -> (SampleEntry, SampleEntry) {
    let video_entry = build_avc1_sample_entry(config.video_width, config.video_height, None);
    let audio_entry = if matches!(audio_codec, AudioCodec::Opus) {
        build_opus_sample_entry(config.sample_rate, config.channels)
    } else {
        build_mp4a_sample_entry(config.sample_rate, config.channels)
    };
    (video_entry, audio_entry)
}

/// Check whether all inputs are done.
const fn all_inputs_done(tp: TrackPresence, tg: &TrackProgress, inputs_open: usize) -> bool {
    if tp.skip_classification {
        inputs_open == 0
    } else {
        (tg.audio_done || !tp.audio) && (tg.video_done || !tp.video)
    }
}

// ---------------------------------------------------------------------------
// Stream (fMP4) mode
// ---------------------------------------------------------------------------

/// Run the muxer in fragmented MP4 (fMP4) streaming mode.
///
/// Each batch of samples is turned into a media segment (moof + mdat) and
/// sent downstream immediately.  The init segment (ftyp + moov) is sent
/// once, either prepended to the first media segment or as a separate packet.
#[allow(clippy::too_many_arguments)]
async fn run_stream_mode(
    config: &Mp4MuxerConfig,
    context: &mut NodeContext,
    node_name: &str,
    content_type: &'static str,
    stats_tracker: &mut NodeStatsTracker,
    audio_codec: AudioCodec,
    mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    mut all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>>,
    tp: TrackPresence,
    mut tg: TrackProgress,
) -> Result<(), StreamKitError> {
    let mut muxer = Fmp4SegmentMuxer::new().map_err(|e| {
        let msg = format!("Failed to create Fmp4SegmentMuxer: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    let (video_timescale, audio_timescale) = resolve_timescales(config);
    let (video_sample_entry, audio_sample_entry) = build_sample_entries(config, audio_codec);

    let mut pending_samples: Vec<Sample> = Vec::new();
    let mut pending_payloads: Vec<Bytes> = Vec::new();
    let mut init_sent = false;
    let mut video_keyframe_seen = false;
    let mut packet_count: u64 = 0;
    // Running data offset within the current segment.
    let mut segment_data_offset: u64 = 0;

    let mut inputs_open = if tp.skip_classification { all_receivers.len() } else { 0 };

    while !all_inputs_done(tp, &tg, inputs_open) {
        let Some(frame) = receive_frame(
            context,
            &mut audio_rx,
            &mut video_rx,
            &mut all_receivers,
            &mut inputs_open,
            &tp,
            &tg,
        )
        .await
        else {
            continue;
        };

        match frame {
            MuxFrame::Shutdown => {
                tracing::info!("Mp4MuxerNode received shutdown signal");
                break;
            },
            MuxFrame::AudioClosed => {
                tracing::info!("Mp4MuxerNode audio input closed");
                tg.audio_done = true;
            },
            MuxFrame::VideoClosed => {
                tracing::info!("Mp4MuxerNode video input closed");
                tg.video_done = true;
            },
            MuxFrame::Video(data, metadata) => {
                let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);

                if !video_keyframe_seen {
                    if is_keyframe {
                        video_keyframe_seen = true;
                    } else {
                        continue;
                    }
                }

                packet_count += 1;
                stats_tracker.received();

                let duration_us = metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .unwrap_or(DEFAULT_VIDEO_FRAME_DURATION_US);
                let duration_ticks = us_to_ticks(duration_us, video_timescale.get());

                let data_size = data.len();
                pending_samples.push(Sample {
                    track_kind: TrackKind::Video,
                    timescale: video_timescale,
                    sample_entry: Some(video_sample_entry.clone()),
                    duration: duration_ticks,
                    keyframe: is_keyframe,
                    composition_time_offset: None,
                    data_offset: segment_data_offset,
                    data_size,
                });
                segment_data_offset += data_size as u64;
                pending_payloads.push(data);
            },
            MuxFrame::Audio(data, metadata) => {
                packet_count += 1;
                stats_tracker.received();

                let duration_us = metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .unwrap_or(DEFAULT_AUDIO_FRAME_DURATION_US);
                let duration_ticks = us_to_ticks(duration_us, audio_timescale.get());

                let data_size = data.len();
                pending_samples.push(Sample {
                    track_kind: TrackKind::Audio,
                    timescale: audio_timescale,
                    sample_entry: Some(audio_sample_entry.clone()),
                    duration: duration_ticks,
                    keyframe: true, // audio frames are always keyframes
                    composition_time_offset: None,
                    data_offset: segment_data_offset,
                    data_size,
                });
                segment_data_offset += data_size as u64;
                pending_payloads.push(data);
            },
        }

        // Flush segment when we have enough samples.
        if pending_samples.len() >= FMP4_SEGMENT_FLUSH_THRESHOLD {
            let stopped = flush_fmp4_segment(
                &mut muxer,
                &mut pending_samples,
                &mut pending_payloads,
                &mut segment_data_offset,
                &mut init_sent,
                context,
                content_type,
                stats_tracker,
                node_name,
            )
            .await?;
            if stopped {
                return Ok(());
            }
        }
    }

    // Flush any remaining samples.
    if !pending_samples.is_empty() {
        flush_fmp4_segment(
            &mut muxer,
            &mut pending_samples,
            &mut pending_payloads,
            &mut segment_data_offset,
            &mut init_sent,
            context,
            content_type,
            stats_tracker,
            node_name,
        )
        .await?;
    }

    tracing::info!("Mp4MuxerNode stream mode: processed {packet_count} packets");
    Ok(())
}

/// Flush accumulated samples as a single fMP4 media segment.
///
/// Returns `true` if the output channel is closed (caller should stop).
#[allow(clippy::too_many_arguments)]
async fn flush_fmp4_segment(
    muxer: &mut Fmp4SegmentMuxer,
    pending_samples: &mut Vec<Sample>,
    pending_payloads: &mut Vec<Bytes>,
    segment_data_offset: &mut u64,
    init_sent: &mut bool,
    context: &mut NodeContext,
    content_type: &'static str,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<bool, StreamKitError> {
    let segment_metadata = muxer.create_media_segment_metadata(pending_samples).map_err(|e| {
        let msg = format!("Failed to create fMP4 segment metadata: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    // Build segment bytes: [moof+mdat header] + [payload data]
    let payload_size: usize = pending_samples.iter().map(|s| s.data_size).sum();
    let mut segment_bytes = Vec::with_capacity(segment_metadata.len() + payload_size);
    segment_bytes.extend_from_slice(&segment_metadata);
    for payload in pending_payloads.drain(..) {
        segment_bytes.extend_from_slice(&payload);
    }

    let ct = Some(content_type.into());

    if *init_sent {
        // Subsequent segment — send media segment only.
        tracing::trace!("Sending fMP4 media segment ({} bytes)", segment_bytes.len());
        if context
            .output_sender
            .send(
                "out",
                Packet::Binary {
                    data: Bytes::from(segment_bytes),
                    content_type: ct,
                    metadata: None,
                },
            )
            .await
            .is_err()
        {
            tracing::debug!("Output channel closed");
            return Ok(true);
        }
        stats_tracker.sent();
    } else {
        // First segment — prepend init segment (ftyp + moov).
        let init = muxer.init_segment_bytes().map_err(|e| {
            let msg = format!("Failed to create fMP4 init segment: {e}");
            state_helpers::emit_failed(&context.state_tx, node_name, &msg);
            StreamKitError::Runtime(msg)
        })?;

        tracing::info!(
            "Sending fMP4 init segment ({} bytes) + first media segment ({} bytes)",
            init.len(),
            segment_bytes.len(),
        );

        let mut combined = Vec::with_capacity(init.len() + segment_bytes.len());
        combined.extend_from_slice(&init);
        combined.extend_from_slice(&segment_bytes);

        if context
            .output_sender
            .send(
                "out",
                Packet::Binary { data: Bytes::from(combined), content_type: ct, metadata: None },
            )
            .await
            .is_err()
        {
            tracing::debug!("Output channel closed");
            return Ok(true);
        }
        stats_tracker.sent();
        *init_sent = true;
    }

    pending_samples.clear();
    *segment_data_offset = 0;

    stats_tracker.maybe_send();
    Ok(false)
}

// ---------------------------------------------------------------------------
// File (regular MP4) mode
// ---------------------------------------------------------------------------

/// Run the muxer in regular MP4 file mode.
///
/// Media data is written to a temporary file on disk.  The muxer tracks
/// metadata (sample tables, chunk offsets) in memory.  At finalization the
/// file is patched with the moov box (via `offset_and_bytes_pairs()`) and
/// read back for a single downstream send.
#[allow(clippy::too_many_arguments)]
async fn run_file_mode(
    config: &Mp4MuxerConfig,
    context: &mut NodeContext,
    node_name: &str,
    content_type: &'static str,
    stats_tracker: &mut NodeStatsTracker,
    audio_codec: AudioCodec,
    mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    mut all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>>,
    tp: TrackPresence,
    mut tg: TrackProgress,
) -> Result<(), StreamKitError> {
    let mut muxer = Mp4FileMuxer::new().map_err(|e| {
        let msg = format!("Failed to create Mp4FileMuxer: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    let mut file_buf = FileBackedBuffer::new().map_err(|e| {
        let msg = format!("Failed to create temp file for MP4 file mode: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    // Write initial boxes (ftyp + placeholder moov) to temp file.
    let initial = muxer.initial_boxes_bytes();
    file_buf
        .write_all(initial)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write initial MP4 boxes: {e}")))?;

    let (video_timescale, audio_timescale) = resolve_timescales(config);
    let (video_sample_entry, audio_sample_entry) = build_sample_entries(config, audio_codec);

    let mut video_keyframe_seen = false;
    let mut packet_count: u64 = 0;
    let mut video_sample_entry_sent = false;
    let mut audio_sample_entry_sent = false;
    let mut inputs_open = if tp.skip_classification { all_receivers.len() } else { 0 };

    while !all_inputs_done(tp, &tg, inputs_open) {
        let Some(frame) = receive_frame(
            context,
            &mut audio_rx,
            &mut video_rx,
            &mut all_receivers,
            &mut inputs_open,
            &tp,
            &tg,
        )
        .await
        else {
            continue;
        };

        match frame {
            MuxFrame::Shutdown => {
                tracing::info!("Mp4MuxerNode file mode received shutdown");
                break;
            },
            MuxFrame::AudioClosed => {
                tracing::info!("Mp4MuxerNode file mode: audio closed");
                tg.audio_done = true;
            },
            MuxFrame::VideoClosed => {
                tracing::info!("Mp4MuxerNode file mode: video closed");
                tg.video_done = true;
            },
            MuxFrame::Video(data, metadata) => {
                process_file_video_frame(
                    &data,
                    metadata.as_ref(),
                    &mut muxer,
                    &mut file_buf,
                    video_timescale,
                    &video_sample_entry,
                    &mut video_keyframe_seen,
                    &mut video_sample_entry_sent,
                    &mut packet_count,
                    stats_tracker,
                )?;
            },
            MuxFrame::Audio(data, metadata) => {
                process_file_audio_frame(
                    &data,
                    metadata.as_ref(),
                    &mut muxer,
                    &mut file_buf,
                    audio_timescale,
                    &audio_sample_entry,
                    &mut audio_sample_entry_sent,
                    &mut packet_count,
                    stats_tracker,
                )?;
            },
        }
    }

    tracing::info!(
        "Mp4MuxerNode file mode: all inputs closed, finalizing ({packet_count} packets)"
    );

    finalize_file_mode(&mut muxer, &mut file_buf, context, content_type, stats_tracker, node_name)
        .await
}

/// Process a single video frame in file mode.
#[allow(clippy::too_many_arguments)]
fn process_file_video_frame(
    data: &Bytes,
    metadata: Option<&PacketMetadata>,
    muxer: &mut Mp4FileMuxer,
    file_buf: &mut FileBackedBuffer,
    video_timescale: NonZeroU32,
    video_sample_entry: &SampleEntry,
    video_keyframe_seen: &mut bool,
    video_sample_entry_sent: &mut bool,
    packet_count: &mut u64,
    stats_tracker: &mut NodeStatsTracker,
) -> Result<(), StreamKitError> {
    let is_keyframe = metadata.and_then(|m| m.keyframe).unwrap_or(false);

    if !*video_keyframe_seen {
        if is_keyframe {
            *video_keyframe_seen = true;
        } else {
            return Ok(());
        }
    }

    *packet_count += 1;
    stats_tracker.received();

    let duration_us =
        metadata.and_then(|m| m.duration_us).unwrap_or(DEFAULT_VIDEO_FRAME_DURATION_US);
    let duration_ticks = us_to_ticks(duration_us, video_timescale.get());

    let data_offset = file_buf
        .position()
        .map_err(|e| StreamKitError::Runtime(format!("Failed to get file position: {e}")))?;
    file_buf
        .write_all(data)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write video data: {e}")))?;

    let entry = if *video_sample_entry_sent {
        None
    } else {
        *video_sample_entry_sent = true;
        Some(video_sample_entry.clone())
    };

    muxer
        .append_sample(&Sample {
            track_kind: TrackKind::Video,
            timescale: video_timescale,
            sample_entry: entry,
            duration: duration_ticks,
            keyframe: is_keyframe,
            composition_time_offset: None,
            data_offset,
            data_size: data.len(),
        })
        .map_err(|e| StreamKitError::Runtime(format!("Failed to append video sample: {e}")))?;

    Ok(())
}

/// Process a single audio frame in file mode.
#[allow(clippy::too_many_arguments)]
fn process_file_audio_frame(
    data: &Bytes,
    metadata: Option<&PacketMetadata>,
    muxer: &mut Mp4FileMuxer,
    file_buf: &mut FileBackedBuffer,
    audio_timescale: NonZeroU32,
    audio_sample_entry: &SampleEntry,
    audio_sample_entry_sent: &mut bool,
    packet_count: &mut u64,
    stats_tracker: &mut NodeStatsTracker,
) -> Result<(), StreamKitError> {
    *packet_count += 1;
    stats_tracker.received();

    let duration_us =
        metadata.and_then(|m| m.duration_us).unwrap_or(DEFAULT_AUDIO_FRAME_DURATION_US);
    let duration_ticks = us_to_ticks(duration_us, audio_timescale.get());

    let data_offset = file_buf
        .position()
        .map_err(|e| StreamKitError::Runtime(format!("Failed to get file position: {e}")))?;
    file_buf
        .write_all(data)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write audio data: {e}")))?;

    let entry = if *audio_sample_entry_sent {
        None
    } else {
        *audio_sample_entry_sent = true;
        Some(audio_sample_entry.clone())
    };

    muxer
        .append_sample(&Sample {
            track_kind: TrackKind::Audio,
            timescale: audio_timescale,
            sample_entry: entry,
            duration: duration_ticks,
            keyframe: true,
            composition_time_offset: None,
            data_offset,
            data_size: data.len(),
        })
        .map_err(|e| StreamKitError::Runtime(format!("Failed to append audio sample: {e}")))?;

    Ok(())
}

/// Finalize the MP4 file: patch moov box, read back, and send downstream.
async fn finalize_file_mode(
    muxer: &mut Mp4FileMuxer,
    file_buf: &mut FileBackedBuffer,
    context: &mut NodeContext,
    content_type: &'static str,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<(), StreamKitError> {
    let finalized = muxer.finalize().map_err(|e| {
        let msg = format!("Failed to finalize MP4: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    for (offset, bytes) in finalized.offset_and_bytes_pairs() {
        file_buf.seek(SeekFrom::Start(offset)).map_err(|e| {
            StreamKitError::Runtime(format!("Failed to seek in MP4 temp file: {e}"))
        })?;
        file_buf
            .write_all(bytes)
            .map_err(|e| StreamKitError::Runtime(format!("Failed to patch MP4 temp file: {e}")))?;
    }

    if let Some(data) = file_buf
        .take_data()
        .map_err(|e| StreamKitError::Runtime(format!("Failed to read back MP4 file: {e}")))?
    {
        tracing::info!("Sending finalized MP4 file ({} bytes)", data.len());
        if context
            .output_sender
            .send(
                "out",
                Packet::Binary { data, content_type: Some(content_type.into()), metadata: None },
            )
            .await
            .is_err()
        {
            tracing::debug!("Output channel closed during final send");
        } else {
            stats_tracker.sent();
        }
        stats_tracker.force_send();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared receive helper
// ---------------------------------------------------------------------------

/// Receive the next frame from audio/video inputs or the control channel.
///
/// Returns `None` when a non-shutdown control message arrives or a
/// non-Binary packet is received (caller should `continue`).
async fn receive_frame(
    context: &mut NodeContext,
    audio_rx: &mut Option<tokio::sync::mpsc::Receiver<Packet>>,
    video_rx: &mut Option<tokio::sync::mpsc::Receiver<Packet>>,
    all_receivers: &mut Vec<tokio::sync::mpsc::Receiver<Packet>>,
    inputs_open: &mut usize,
    tp: &TrackPresence,
    tg: &TrackProgress,
) -> Option<MuxFrame> {
    if tp.skip_classification {
        receive_unified(context, all_receivers, inputs_open).await
    } else if (tg.audio_done || !tp.audio) && !tg.video_done && tp.video {
        receive_single_track(context, video_rx, true).await
    } else if (tg.video_done || !tp.video) && !tg.audio_done && tp.audio {
        receive_single_track(context, audio_rx, false).await
    } else if tp.audio && tp.video && !tg.audio_done && !tg.video_done {
        receive_dual_track(context, audio_rx, video_rx).await
    } else {
        None
    }
}

/// Receive from all input channels in unified (skip-classification) mode.
async fn receive_unified(
    context: &mut NodeContext,
    all_receivers: &mut Vec<tokio::sync::mpsc::Receiver<Packet>>,
    inputs_open: &mut usize,
) -> Option<MuxFrame> {
    if all_receivers.len() >= 2 {
        let (first, rest) = all_receivers.split_at_mut(1);
        let rx0 = &mut first[0];
        let rx1 = &mut rest[0];
        tokio::select! {
            biased;
            Some(msg) = context.control_rx.recv() => {
                if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                    return Some(MuxFrame::Shutdown);
                }
                None
            }
            r0 = rx0.recv() => {
                r0.map_or_else(
                    || { all_receivers.remove(0); *inputs_open -= 1; None },
                    classify_packet,
                )
            }
            r1 = rx1.recv() => {
                r1.map_or_else(
                    || { all_receivers.remove(1); *inputs_open -= 1; None },
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
                    return Some(MuxFrame::Shutdown);
                }
                None
            }
            r = rx.recv() => r.map_or_else(
                || { all_receivers.clear(); *inputs_open = 0; None },
                classify_packet,
            )
        }
    } else {
        None
    }
}

/// Receive from a single remaining track (audio or video).
async fn receive_single_track(
    context: &mut NodeContext,
    rx_opt: &mut Option<tokio::sync::mpsc::Receiver<Packet>>,
    is_video: bool,
) -> Option<MuxFrame> {
    let rx = rx_opt.as_mut()?;
    tokio::select! {
        biased;
        Some(msg) = context.control_rx.recv() => {
            if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                return Some(MuxFrame::Shutdown);
            }
            None
        }
        result = rx.recv() => {
            match result {
                Some(Packet::Binary { data, metadata, .. }) => {
                    Some(if is_video {
                        MuxFrame::Video(data, metadata)
                    } else {
                        MuxFrame::Audio(data, metadata)
                    })
                },
                Some(_) => None,
                None => Some(if is_video { MuxFrame::VideoClosed } else { MuxFrame::AudioClosed }),
            }
        }
    }
}

/// Receive from both audio and video tracks.
async fn receive_dual_track(
    context: &mut NodeContext,
    audio_rx: &mut Option<tokio::sync::mpsc::Receiver<Packet>>,
    video_rx: &mut Option<tokio::sync::mpsc::Receiver<Packet>>,
) -> Option<MuxFrame> {
    let (Some(a_rx), Some(v_rx)) = (audio_rx.as_mut(), video_rx.as_mut()) else {
        return None;
    };
    tokio::select! {
        biased;
        Some(msg) = context.control_rx.recv() => {
            if matches!(msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                return Some(MuxFrame::Shutdown);
            }
            None
        }
        maybe_audio = a_rx.recv() => {
            match maybe_audio {
                Some(Packet::Binary { data, metadata, .. }) => Some(MuxFrame::Audio(data, metadata)),
                Some(_) => None,
                None => Some(MuxFrame::AudioClosed),
            }
        }
        maybe_video = v_rx.recv() => {
            match maybe_video {
                Some(Packet::Binary { data, metadata, .. }) => Some(MuxFrame::Video(data, metadata)),
                Some(_) => None,
                None => Some(MuxFrame::VideoClosed),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Convert a duration in microseconds to timescale ticks.
fn us_to_ticks(duration_us: u64, timescale: u32) -> u32 {
    // duration_ticks = duration_us * timescale / 1_000_000
    // Use u64 intermediate to avoid overflow.
    let ticks = duration_us.saturating_mul(u64::from(timescale)) / 1_000_000;
    u32::try_from(ticks).unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::{config_helpers, registry::StaticPins};

/// Registers the MP4 container nodes.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON.
#[allow(clippy::expect_used)]
pub fn register_mp4_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "mp4")]
    {
        let default_muxer = Mp4MuxerNode::new(Mp4MuxerConfig::default());
        registry.register_static_with_description(
            "containers::mp4::muxer",
            |params| {
                let config = config_helpers::parse_config_with_context(params, "Mp4Muxer")?;
                Ok(Box::new(Mp4MuxerNode::new(config)))
            },
            serde_json::to_value(schema_for!(Mp4MuxerConfig))
                .expect("Mp4MuxerConfig schema should serialize to JSON"),
            StaticPins { inputs: default_muxer.input_pins(), outputs: default_muxer.output_pins() },
            vec!["containers".to_string(), "mp4".to_string()],
            false,
            "Muxes H.264 video and/or AAC/Opus audio into an MP4 container. \
             Supports fragmented MP4 (fMP4) for DASH/HLS streaming and \
             regular MP4 file output with fast-start.",
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_stream_mode() {
        let config = Mp4MuxerConfig::default();
        assert!(matches!(config.mode, Mp4StreamingMode::Stream));
    }

    #[test]
    fn content_type_combinations() {
        assert_eq!(mp4_content_type(true, true, false), "video/mp4; codecs=\"avc1,mp4a\"");
        assert_eq!(mp4_content_type(true, true, true), "video/mp4; codecs=\"avc1,opus\"");
        assert_eq!(mp4_content_type(false, true, false), "video/mp4; codecs=\"avc1\"");
        assert_eq!(mp4_content_type(true, false, false), "audio/mp4; codecs=\"mp4a\"");
        assert_eq!(mp4_content_type(true, false, true), "audio/mp4; codecs=\"opus\"");
        assert_eq!(mp4_content_type(false, false, false), "video/mp4");
    }

    #[test]
    fn us_to_ticks_basic() {
        // 33333 us at 90000 Hz = 2999 ticks (≈30fps)
        assert_eq!(us_to_ticks(33_333, 90_000), 2999);
        // 20000 us at 48000 Hz = 960 ticks
        assert_eq!(us_to_ticks(20_000, 48_000), 960);
    }

    #[test]
    fn input_pins_default_single() {
        let node = Mp4MuxerNode::new(Mp4MuxerConfig::default());
        assert_eq!(node.input_pins().len(), 1);
    }

    #[test]
    fn input_pins_dual() {
        let node = Mp4MuxerNode::new(Mp4MuxerConfig { num_inputs: 2, ..Mp4MuxerConfig::default() });
        let pins = node.input_pins();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "in");
        assert_eq!(pins[1].name, "in_1");
    }

    #[test]
    fn output_pin_is_binary() {
        let node = Mp4MuxerNode::new(Mp4MuxerConfig::default());
        let pins = node.output_pins();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, "out");
    }

    #[test]
    fn build_avc1_sample_entry_produces_avc1() {
        let entry = build_avc1_sample_entry(1280, 720, None);
        assert!(matches!(entry, SampleEntry::Avc1(_)));
    }

    #[test]
    fn build_opus_sample_entry_produces_opus() {
        let entry = build_opus_sample_entry(48000, 2);
        assert!(matches!(entry, SampleEntry::Opus(_)));
    }

    #[test]
    fn build_mp4a_sample_entry_produces_mp4a() {
        let entry = build_mp4a_sample_entry(48000, 2);
        assert!(matches!(entry, SampleEntry::Mp4a(_)));
    }

    /// Round-trip test: mux a few fMP4 segments and demux them back.
    #[test]
    fn fmp4_round_trip_video_only() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let sample_entry = build_avc1_sample_entry(1280, 720, None);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        // Create 3 segments with 1 video frame each.
        let mut all_segments = Vec::new();
        for seg_idx in 0..3u32 {
            let frame_data = vec![0u8; 512];
            let samples = vec![Sample {
                track_kind: TrackKind::Video,
                timescale: video_timescale,
                sample_entry: Some(sample_entry.clone()),
                duration: 3000, // 90000/30 = 3000
                keyframe: seg_idx == 0,
                composition_time_offset: None,
                data_offset: 0,
                data_size: frame_data.len(),
            }];
            let metadata = muxer.create_media_segment_metadata(&samples).unwrap();
            let mut segment = metadata;
            segment.extend_from_slice(&frame_data);
            all_segments.push(segment);
        }

        let init = muxer.init_segment_bytes().unwrap();

        // Demux
        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();
        let tracks = demuxer.tracks().unwrap();
        assert!(!tracks.is_empty(), "Should have at least one track");

        let mut total_samples = 0;
        for segment in &all_segments {
            let samples = demuxer.handle_media_segment(segment).unwrap();
            total_samples += samples.len();
        }
        assert_eq!(total_samples, 3, "Should have 3 demuxed samples");
    }

    /// Round-trip test: mux video + audio fMP4 and verify both tracks.
    #[test]
    fn fmp4_round_trip_audio_video() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let audio_timescale = NonZeroU32::new(48_000).unwrap();
        let video_entry = build_avc1_sample_entry(640, 480, None);
        let audio_entry = build_opus_sample_entry(48_000, 2);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        let video_data = vec![0u8; 1024];
        let audio_data = vec![0u8; 256];

        let samples = vec![
            Sample {
                track_kind: TrackKind::Video,
                timescale: video_timescale,
                sample_entry: Some(video_entry),
                duration: 3000,
                keyframe: true,
                composition_time_offset: None,
                data_offset: 0,
                data_size: video_data.len(),
            },
            Sample {
                track_kind: TrackKind::Audio,
                timescale: audio_timescale,
                sample_entry: Some(audio_entry),
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: video_data.len() as u64,
                data_size: audio_data.len(),
            },
        ];

        let metadata = muxer.create_media_segment_metadata(&samples).unwrap();
        let mut segment = metadata;
        segment.extend_from_slice(&video_data);
        segment.extend_from_slice(&audio_data);

        let init = muxer.init_segment_bytes().unwrap();

        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();

        let tracks = demuxer.tracks().unwrap();
        assert_eq!(tracks.len(), 2, "Should have video + audio tracks");

        let segment_result = demuxer.handle_media_segment(&segment).unwrap();
        assert_eq!(segment_result.len(), 2, "Should have 2 samples (1 video + 1 audio)");
    }

    /// File mode round-trip: mux and verify file is valid MP4.
    #[test]
    fn file_mode_round_trip() {
        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let sample_entry = build_avc1_sample_entry(320, 240, None);

        let mut muxer = Mp4FileMuxer::new().unwrap();
        let mut output = Vec::new();

        // Write initial boxes.
        output.extend_from_slice(muxer.initial_boxes_bytes());

        // Write 5 frames.
        for i in 0..5u32 {
            let frame = vec![0xABu8; 256];
            let data_offset = output.len() as u64;
            output.extend_from_slice(&frame);

            muxer
                .append_sample(&Sample {
                    track_kind: TrackKind::Video,
                    timescale: video_timescale,
                    sample_entry: if i == 0 { Some(sample_entry.clone()) } else { None },
                    duration: 3000,
                    keyframe: i == 0,
                    composition_time_offset: None,
                    data_offset,
                    data_size: frame.len(),
                })
                .unwrap();
        }

        // Finalize — patch output.
        let finalized = muxer.finalize().unwrap();
        for (offset, bytes) in finalized.offset_and_bytes_pairs() {
            let off = usize::try_from(offset).expect("offset exceeds usize");
            // Extend if needed.
            if off + bytes.len() > output.len() {
                output.resize(off + bytes.len(), 0);
            }
            output[off..off + bytes.len()].copy_from_slice(bytes);
        }

        // Verify: output should start with 'ftyp' box.
        assert!(output.len() > 8, "Output too small");
        // ftyp box type at bytes 4..8
        assert_eq!(&output[4..8], b"ftyp", "MP4 should start with ftyp box");
    }

    /// Verify AVCC codec_private parsing.
    #[test]
    fn parse_avcc_codec_private_basic() {
        // Minimal AVCDecoderConfigurationRecord
        let mut data = vec![
            1,    // version
            66,   // profile_idc (baseline)
            0,    // constraint
            30,   // level 3.0
            0xFF, // length_size_minus_one=3 (0b111111_11)
        ];
        // 1 SPS
        data.push(0xE1); // 0b111_00001
        let sps = vec![0x67, 0x42, 0x00, 0x1E];
        data.extend_from_slice(&u16::try_from(sps.len()).expect("SPS too large").to_be_bytes());
        data.extend_from_slice(&sps);
        // 1 PPS
        data.push(0x01);
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        data.extend_from_slice(&u16::try_from(pps.len()).expect("PPS too large").to_be_bytes());
        data.extend_from_slice(&pps);

        let (sps_list, pps_list, profile, compat, level) = parse_avcc_codec_private(&data);
        assert_eq!(sps_list.len(), 1);
        assert_eq!(pps_list.len(), 1);
        assert_eq!(profile, 66);
        assert_eq!(compat, 0);
        assert_eq!(level, 30);
        assert_eq!(sps_list[0], sps);
        assert_eq!(pps_list[0], pps);
    }
}
