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
//! Codec support: H.264 (AVC) and AV1 video + AAC/Opus audio.  Additional
//! codecs (e.g. VP9) can be added by extending the sample-entry construction helpers.

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::Deserialize;
use shiguredo_mp4::boxes::{
    AudioSampleEntryFields, Av01Box, Av1cBox, Avc1Box, AvccBox, DopsBox, EsdsBox, Mp4aBox, OpusBox,
    SampleEntry, VisualSampleEntryFields,
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

/// Default audio frame duration when metadata is missing.
///
/// The correct value depends on the codec:
/// - Opus: 20 ms (960 samples at 48 kHz)
/// - AAC-LC: ~21.333 ms (1024 samples at 48 kHz)
///
/// Use [`default_audio_frame_duration_us`] to get the codec-aware value.
const DEFAULT_AUDIO_FRAME_DURATION_US_OPUS: u64 = 20_000;
const DEFAULT_AUDIO_FRAME_DURATION_US_AAC: u64 = 21_333;

/// Return the default audio frame duration for the given codec.
const fn default_audio_frame_duration_us(codec: AudioCodec) -> u64 {
    match codec {
        AudioCodec::Aac => DEFAULT_AUDIO_FRAME_DURATION_US_AAC,
        _ => DEFAULT_AUDIO_FRAME_DURATION_US_OPUS,
    }
}

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

/// Safety cap for the first-flush gate: if we accumulate this many samples
/// without all expected tracks registering, force a flush with a warning
/// rather than growing without bound.
const FMP4_FIRST_FLUSH_DEFER_CAP: usize = 10 * FMP4_SEGMENT_FLUSH_THRESHOLD;

// ---------------------------------------------------------------------------
// Sample entry construction helpers
// ---------------------------------------------------------------------------

/// Build an AVC1 (H.264) sample entry box from SPS/PPS NAL units.
///
/// If `codec_private` is available, it is expected to contain an AVCDecoderConfigurationRecord
/// or raw SPS/PPS NAL units.  Otherwise a minimal placeholder is used.
fn build_avc1_sample_entry(width: u16, height: u16, codec_private: Option<&[u8]>) -> SampleEntry {
    let (sps_list, pps_list, profile, compat, level) = codec_private.map_or_else(
        || (vec![vec![0x67, 0x42, 0xc0, 0x1f]], vec![vec![0x68, 0xce, 0x38, 0x80]], 66, 0, 31),
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
            sps_list.push(vec![0x67, 0x42, 0xc0, 0x1f]);
        }
        if pps_list.is_empty() {
            pps_list.push(vec![0x68, 0xce, 0x38, 0x80]);
        }

        (sps_list, pps_list, profile, compat, level)
    } else {
        // Fallback: treat as raw SPS data
        (vec![data.to_vec()], vec![vec![0x68, 0xce, 0x38, 0x80]], 66, 0, 31)
    }
}

// ---------------------------------------------------------------------------
// H.264 Annex B → AVCC conversion
// ---------------------------------------------------------------------------

/// NAL unit type bitmask (lower 5 bits of NAL header byte).
const H264_NAL_TYPE_MASK: u8 = 0x1F;
/// NAL unit type: Sequence Parameter Set.
const H264_NAL_SPS: u8 = 7;
/// NAL unit type: Picture Parameter Set.
const H264_NAL_PPS: u8 = 8;

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
        // Check for 3-byte or 4-byte start code.
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
            // End previous NAL unit (if any).
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

    // Last NAL unit extends to end of data.
    if let Some(start) = nal_start {
        if start < len {
            nals.push(&data[start..len]);
        }
    }

    nals
}

/// Result of converting an H.264 Annex B access unit to AVCC format.
struct H264AvccConversion {
    /// AVCC-formatted data (4-byte length-prefixed NAL units).
    data: Bytes,
    /// SPS NAL units found in this access unit (empty if none).
    sps_list: Vec<Vec<u8>>,
    /// PPS NAL units found in this access unit (empty if none).
    pps_list: Vec<Vec<u8>>,
}

/// Convert an H.264 Annex B bitstream to AVCC format.
///
/// Each NAL unit's start code is replaced with a 4-byte big-endian length
/// prefix.  SPS and PPS NAL units are extracted separately so the caller
/// can populate the `AvccBox` in the MP4 sample entry.
fn convert_annexb_to_avcc(data: &[u8]) -> H264AvccConversion {
    let nals = parse_annexb_nal_units(data);
    let mut out = Vec::with_capacity(data.len());
    let mut sps_list = Vec::new();
    let mut pps_list = Vec::new();

    for nal in nals {
        if nal.is_empty() {
            continue;
        }

        // 4-byte big-endian length prefix.
        let len = u32::try_from(nal.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);

        // Classify and extract parameter sets.
        let nal_type = nal[0] & H264_NAL_TYPE_MASK;
        if nal_type == H264_NAL_SPS {
            sps_list.push(nal.to_vec());
        } else if nal_type == H264_NAL_PPS {
            pps_list.push(nal.to_vec());
        }
    }

    H264AvccConversion { data: Bytes::from(out), sps_list, pps_list }
}

/// Rebuild an AVC1 sample entry using real SPS/PPS extracted from the bitstream.
///
/// This replaces the placeholder SPS/PPS in the initial sample entry with
/// actual parameter sets from the encoder, ensuring the `AvccBox` accurately
/// describes the stream for MSE and compliant demuxers.
fn rebuild_avc1_entry_from_params(
    width: u16,
    height: u16,
    sps_list: Vec<Vec<u8>>,
    pps_list: Vec<Vec<u8>>,
) -> SampleEntry {
    // Extract profile/constraints/level from the first SPS NAL unit.
    // SPS layout: [nal_header, profile_idc, constraint_flags, level_idc, ...]
    let (profile, compat, level) = sps_list
        .first()
        .filter(|sps| sps.len() >= 4)
        .map_or((66, 0, 31), |sps| (sps[1], sps[2], sps[3]));

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

/// Build an mp4a (AAC) sample entry box.
///
/// Constructs a minimal ESDS descriptor for AAC-LC with the given sample rate
/// and channel count.
///
/// Currently only reachable as a fallback when a future `AudioCodec` variant is
/// added.  Kept as scaffolding so AAC support can be enabled by simply wiring
/// up the new variant in `build_sample_entries`.
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

/// Build an AV1 (`av01`) sample entry box.
///
/// If `codec_private` is available it is stored as `config_obus` in the
/// AV1CodecConfigurationBox.  Otherwise a minimal Main-profile placeholder is
/// used.
fn build_av01_sample_entry(width: u16, height: u16, codec_private: Option<&[u8]>) -> SampleEntry {
    let config_obus = codec_private.unwrap_or(&[]).to_vec();

    SampleEntry::Av01(Av01Box {
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
        av1c_box: Av1cBox {
            seq_profile: Uint::new(0),     // Main profile
            seq_level_idx_0: Uint::new(4), // Level 3.0
            seq_tier_0: Uint::new(0),      // Main tier
            high_bitdepth: Uint::new(0),   // 8-bit
            twelve_bit: Uint::new(0),
            monochrome: Uint::new(0),
            chroma_subsampling_x: Uint::new(1),   // 4:2:0
            chroma_subsampling_y: Uint::new(1),   // 4:2:0
            chroma_sample_position: Uint::new(0), // Unknown
            initial_presentation_delay_minus_one: None,
            config_obus,
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

    /// Override the audio codec used for sample-entry construction and MIME
    /// content-type resolution.  Accepted values: `"opus"`, `"aac"`.
    /// When omitted the codec is auto-detected from the upstream
    /// `EncodedAudio` pin type; if detection fails it falls back to `Opus`.
    #[serde(default)]
    pub audio_codec: Option<String>,
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
            audio_codec: None,
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
            let is_video = content_type.as_deref().map_or_else(
                || {
                    tracing::warn!(
                        "Packet has no content_type — defaulting to audio; \
                         set content_type on upstream nodes to avoid misclassification"
                    );
                    false
                },
                |ct| ct.starts_with("video/"),
            );
            Some(if is_video {
                MuxFrame::Video(data, metadata)
            } else {
                MuxFrame::Audio(data, metadata)
            })
        },
        _ => None,
    }
}

/// Parse an optional audio codec config string into an [`AudioCodec`].
///
/// Returns `Opus` when the input is `None` or unrecognised.
/// The parsing logic is intentionally inlined here (rather than imported from
/// `transport::moq::constants`) so that the `mp4` feature does not depend on
/// the `moq` feature at compile time.
fn parse_mp4_audio_codec_config(s: Option<&str>) -> AudioCodec {
    s.map_or(AudioCodec::Opus, |v| {
        match v.to_ascii_lowercase().as_str() {
            "aac" => AudioCodec::Aac,
            "opus" => AudioCodec::Opus,
            other => {
                tracing::warn!(audio_codec = %other, "unrecognised audio_codec config — defaulting to Opus");
                AudioCodec::Opus
            },
        }
    })
}

/// Determine the MP4 MIME content-type string from optional codec info.
///
/// `audio` and `video` are `None` when the respective track is absent.
///
/// **Future-proofing note:** Any video codec that is not `Av1` currently maps
/// to `avc1` in the codecs parameter, and any audio codec that is not `Opus`
/// maps to `mp4a`.  When new codecs are added (e.g. VP9, HEVC), the match
/// arms below must be extended — the fallback will log a warning so the
/// mismatch is visible.
fn mp4_content_type(audio: Option<AudioCodec>, video: Option<VideoCodec>) -> &'static str {
    if let Some(vc) = &video {
        if !matches!(vc, VideoCodec::H264 | VideoCodec::Av1) {
            tracing::warn!(
                ?vc,
                "mp4_content_type: unrecognised video codec — codecs param will omit video codec"
            );
        }
    }

    // Match on (audio_codec, video_codec) to produce a precise MIME codecs string.
    match (audio, video) {
        // Audio + Video
        (Some(AudioCodec::Opus), Some(VideoCodec::Av1)) => "video/mp4; codecs=\"av01,opus\"",
        (Some(AudioCodec::Opus), Some(VideoCodec::H264)) => "video/mp4; codecs=\"avc1,opus\"",
        (Some(AudioCodec::Aac), Some(VideoCodec::Av1)) => "video/mp4; codecs=\"av01,mp4a\"",
        (Some(AudioCodec::Aac), Some(VideoCodec::H264)) => "video/mp4; codecs=\"avc1,mp4a\"",
        // Audio + unknown/future video codec
        (Some(AudioCodec::Opus), Some(_)) => "video/mp4; codecs=\"opus\"",
        (Some(AudioCodec::Aac), Some(_)) => "video/mp4; codecs=\"mp4a\"",
        // Audio-only
        (Some(AudioCodec::Opus), None) => "audio/mp4; codecs=\"opus\"",
        (Some(AudioCodec::Aac), None) => "audio/mp4; codecs=\"mp4a\"",
        // Future audio codec — warn and omit codecs param.
        (Some(_), Some(_)) => {
            tracing::warn!("mp4_content_type: unrecognised audio codec — omitting codecs param");
            "video/mp4"
        },
        (Some(_), None) => {
            tracing::warn!("mp4_content_type: unrecognised audio codec — omitting codecs param");
            "audio/mp4"
        },
        // Video-only
        (None, Some(VideoCodec::Av1)) => "video/mp4; codecs=\"av01\"",
        (None, Some(VideoCodec::H264)) => "video/mp4; codecs=\"avc1\"",
        // Fallback
        (None, Some(_) | None) => "video/mp4",
    }
}

/// A node that muxes encoded H.264/AV1 video and/or AAC/Opus audio into an MP4 container.
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

/// Shared immutable state for a muxing session, threaded through mode helpers.
struct MuxSession<'a> {
    config: &'a Mp4MuxerConfig,
    node_name: &'a str,
    content_type: &'static str,
    audio_codec: AudioCodec,
    video_codec: VideoCodec,
}

/// Owned input channels and track metadata, consumed by the mode entry points.
struct MuxInputs {
    audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    video_rx: Option<tokio::sync::mpsc::Receiver<Packet>>,
    all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>>,
    tp: TrackPresence,
    tg: TrackProgress,
}

/// Accumulated state for an in-progress fMP4 segment (stream mode).
///
/// The `video_sample_entry_sent` / `audio_sample_entry_sent` flags are
/// intentionally **not** reset across segments.  `shiguredo_mp4` stores
/// sample entries in its own `TrackEntry::sample_entries` vec on first
/// encounter and uses `current_sample_entry_index` for subsequent samples
/// that arrive with `sample_entry: None`.  Sending the entry only once
/// avoids unnecessary cloning.
struct Fmp4SegmentState {
    pending_samples: Vec<Sample>,
    pending_payloads: Vec<Bytes>,
    init_sent: bool,
    video_sample_entry_sent: bool,
    audio_sample_entry_sent: bool,
}

/// Muxer and file-backed state for regular MP4 file mode.
struct FileMuxState {
    muxer: Mp4FileMuxer,
    file_buf: FileBackedBuffer,
    video_sample_entry: SampleEntry,
    audio_sample_entry: SampleEntry,
    video_timescale: NonZeroU32,
    audio_timescale: NonZeroU32,
    video_keyframe_seen: bool,
    video_sample_entry_sent: bool,
    audio_sample_entry_sent: bool,
    packet_count: u64,
    /// Reusable scratch buffer for H.264 Annex B → AVCC conversion (file mode).
    h264_scratch: Bytes,
}

#[async_trait]
#[allow(clippy::too_many_lines)] // ProcessorNode::run orchestrates input classification, codec detection, and mode dispatch; further splitting would obscure the control flow.
impl ProcessorNode for Mp4MuxerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        let media_types = vec![
            PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
            PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Aac,
                codec_private: None,
            }),
            PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::H264,
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
        // Static pre-connection hint only — the runtime content type is resolved
        // from actual input codec types.  We hardcode AV1 here because it is the
        // most common video codec in StreamKit pipelines today.  If a pipeline
        // uses H.264 or VP9, the static hint will be slightly inaccurate but the
        // runtime MIME type on each output packet will be correct.  Consider
        // adding a `video_codec` config field if precise pre-connection
        // negotiation becomes important.
        let video = if self.config.video_width > 0 && self.config.video_height > 0 {
            Some(VideoCodec::Av1)
        } else {
            None
        };
        let audio = parse_mp4_audio_codec_config(self.config.audio_codec.as_deref());
        Some(mp4_content_type(Some(audio), video).to_string())
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
        let mut audio_codec = parse_mp4_audio_codec_config(self.config.audio_codec.as_deref());
        // Default video codec is AV1; only used when a video input is actually
        // connected.  For audio-only pipelines this value is never read.
        let mut video_codec = VideoCodec::Av1;
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

            // Detect video codec from type info.
            if let Some(PacketType::EncodedVideo(fmt)) = pin_type {
                video_codec = fmt.codec;
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

        let audio_arg = if has_audio { Some(audio_codec) } else { None };
        let video_arg = if has_video { Some(video_codec) } else { None };
        let content_type_str = mp4_content_type(audio_arg, video_arg);

        tracing::info!(
            "Mp4MuxerNode tracks: audio={has_audio} video={has_video} \
             mode={:?} content_type={content_type_str}",
            self.config.mode,
        );

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // ---- Dispatch to mode-specific muxing logic ----

        let session = MuxSession {
            config: &self.config,
            node_name: &node_name,
            content_type: content_type_str,
            audio_codec,
            video_codec,
        };

        let inputs = MuxInputs {
            audio_rx,
            video_rx,
            all_receivers,
            tp: TrackPresence { audio: has_audio, video: has_video, skip_classification },
            tg: TrackProgress { audio_done: false, video_done: false },
        };

        match self.config.mode {
            Mp4StreamingMode::Stream => {
                run_stream_mode(&session, &mut context, &mut stats_tracker, inputs).await?;
            },
            Mp4StreamingMode::File => {
                run_file_mode(&session, &mut context, &mut stats_tracker, inputs).await?;
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

/// Build codec-specific sample entries from config and detected codecs.
fn build_sample_entries(
    config: &Mp4MuxerConfig,
    audio_codec: AudioCodec,
    video_codec: VideoCodec,
) -> (SampleEntry, SampleEntry) {
    let video_entry = match video_codec {
        VideoCodec::Av1 => build_av01_sample_entry(config.video_width, config.video_height, None),
        VideoCodec::H264 => build_avc1_sample_entry(config.video_width, config.video_height, None),
        // VideoCodec is #[non_exhaustive]; warn and fall back to AVC1 for any
        // future variant so the muxer degrades visibly rather than silently.
        #[allow(unreachable_patterns)] // only H264, Av1, Vp9 exist today
        other => {
            tracing::warn!(
                ?other,
                "Unknown VideoCodec variant \u{2014} falling back to avc1 (H.264) sample entry"
            );
            build_avc1_sample_entry(config.video_width, config.video_height, None)
        },
    };
    let audio_entry = match audio_codec {
        AudioCodec::Opus => build_opus_sample_entry(config.sample_rate, config.channels),
        AudioCodec::Aac => build_mp4a_sample_entry(config.sample_rate, config.channels),
        // AudioCodec is #[non_exhaustive]; warn and fall back to mp4a (AAC) for
        // any future variant so the muxer degrades visibly rather than silently.
        #[allow(unreachable_patterns)]
        other => {
            tracing::warn!(
                ?other,
                "Unknown AudioCodec variant — falling back to mp4a (AAC) sample entry"
            );
            build_mp4a_sample_entry(config.sample_rate, config.channels)
        },
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

/// Partition interleaved samples+payloads by track so each track's data is
/// contiguous within the segment, then recompute `data_offset` values.
///
/// `shiguredo_mp4` requires that all data for a given track appears as a
/// contiguous byte range in the mdat box.  When audio and video arrive
/// interleaved, the naive arrival-order offsets break this invariant.
fn partition_samples_by_track(
    samples: Vec<Sample>,
    payloads: Vec<Bytes>,
) -> (Vec<Sample>, Vec<Bytes>) {
    debug_assert_eq!(samples.len(), payloads.len());

    let mut video_samples = Vec::new();
    let mut video_payloads = Vec::new();
    let mut audio_samples = Vec::new();
    let mut audio_payloads = Vec::new();

    for (sample, payload) in samples.into_iter().zip(payloads) {
        match sample.track_kind {
            TrackKind::Video => {
                video_samples.push(sample);
                video_payloads.push(payload);
            },
            TrackKind::Audio => {
                audio_samples.push(sample);
                audio_payloads.push(payload);
            },
        }
    }

    // Recompute data_offset: video first, then audio.
    let mut offset: u64 = 0;
    let mut sorted_samples = Vec::with_capacity(video_samples.len() + audio_samples.len());
    let mut sorted_payloads = Vec::with_capacity(video_payloads.len() + audio_payloads.len());

    for (mut sample, payload) in video_samples.into_iter().zip(video_payloads) {
        sample.data_offset = offset;
        offset += sample.data_size as u64;
        sorted_samples.push(sample);
        sorted_payloads.push(payload);
    }
    for (mut sample, payload) in audio_samples.into_iter().zip(audio_payloads) {
        sample.data_offset = offset;
        offset += sample.data_size as u64;
        sorted_samples.push(sample);
        sorted_payloads.push(payload);
    }

    (sorted_samples, sorted_payloads)
}

// ---------------------------------------------------------------------------
// Stream (fMP4) mode
// ---------------------------------------------------------------------------

/// Run the muxer in fragmented MP4 (fMP4) streaming mode.
///
/// Each batch of samples is turned into a media segment (moof + mdat) and
/// sent downstream immediately.  The init segment (ftyp + moov) is sent
/// once, either prepended to the first media segment or as a separate packet.
async fn run_stream_mode(
    session: &MuxSession<'_>,
    context: &mut NodeContext,
    stats_tracker: &mut NodeStatsTracker,
    mut inputs: MuxInputs,
) -> Result<(), StreamKitError> {
    let mut muxer = Fmp4SegmentMuxer::new().map_err(|e| {
        let msg = format!("Failed to create Fmp4SegmentMuxer: {e}");
        state_helpers::emit_failed(&context.state_tx, session.node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    let (video_timescale, audio_timescale) = resolve_timescales(session.config);
    let (mut video_sample_entry, audio_sample_entry) =
        build_sample_entries(session.config, session.audio_codec, session.video_codec);
    let is_h264 = session.video_codec == VideoCodec::H264;

    let mut seg = Fmp4SegmentState {
        pending_samples: Vec::new(),
        pending_payloads: Vec::new(),
        init_sent: false,
        video_sample_entry_sent: false,
        audio_sample_entry_sent: false,
    };
    let mut video_keyframe_seen = false;
    let mut packet_count: u64 = 0;

    let mut inputs_open =
        if inputs.tp.skip_classification { inputs.all_receivers.len() } else { 0 };

    while !all_inputs_done(inputs.tp, &inputs.tg, inputs_open) {
        let Some(frame) = receive_frame(
            context,
            &mut inputs.audio_rx,
            &mut inputs.video_rx,
            &mut inputs.all_receivers,
            &mut inputs_open,
            &inputs.tp,
            &inputs.tg,
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
                inputs.tg.audio_done = true;
            },
            MuxFrame::VideoClosed => {
                tracing::info!("Mp4MuxerNode video input closed");
                inputs.tg.video_done = true;
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

                // Convert Annex B → AVCC for H.264 streams so the mdat
                // contains length-prefixed NAL units matching the avc1 box.
                let data = if is_h264 {
                    let conv = convert_annexb_to_avcc(&data);
                    // On the first keyframe, update the sample entry with
                    // real SPS/PPS so the init segment (moov) describes the
                    // actual stream parameters instead of placeholders.
                    if !conv.sps_list.is_empty() && !seg.video_sample_entry_sent {
                        video_sample_entry = rebuild_avc1_entry_from_params(
                            session.config.video_width,
                            session.config.video_height,
                            conv.sps_list,
                            conv.pps_list,
                        );
                    }
                    conv.data
                } else {
                    data
                };

                let duration_us = metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .unwrap_or(DEFAULT_VIDEO_FRAME_DURATION_US);
                let duration_ticks = us_to_ticks(duration_us, video_timescale.get());

                let data_size = data.len();
                let entry = if seg.video_sample_entry_sent {
                    None
                } else {
                    seg.video_sample_entry_sent = true;
                    Some(video_sample_entry.clone())
                };
                seg.pending_samples.push(Sample {
                    track_kind: TrackKind::Video,
                    timescale: video_timescale,
                    sample_entry: entry,
                    duration: duration_ticks,
                    keyframe: is_keyframe,
                    composition_time_offset: None,
                    data_offset: 0, // placeholder; partition_samples_by_track recomputes
                    data_size,
                });
                seg.pending_payloads.push(data);
            },
            MuxFrame::Audio(data, metadata) => {
                packet_count += 1;
                stats_tracker.received();

                let duration_us = metadata
                    .as_ref()
                    .and_then(|m| m.duration_us)
                    .unwrap_or_else(|| default_audio_frame_duration_us(session.audio_codec));
                let duration_ticks = us_to_ticks(duration_us, audio_timescale.get());

                let data_size = data.len();
                let entry = if seg.audio_sample_entry_sent {
                    None
                } else {
                    seg.audio_sample_entry_sent = true;
                    Some(audio_sample_entry.clone())
                };
                seg.pending_samples.push(Sample {
                    track_kind: TrackKind::Audio,
                    timescale: audio_timescale,
                    sample_entry: entry,
                    duration: duration_ticks,
                    keyframe: true, // audio frames are always keyframes
                    composition_time_offset: None,
                    data_offset: 0, // placeholder; partition_samples_by_track recomputes
                    data_size,
                });
                seg.pending_payloads.push(data);
            },
        }

        // Flush segment when we have enough samples.
        //
        // Gate the very first flush on all expected tracks having registered
        // their sample entries so the init segment (moov) describes every
        // track — similar to how the WebM muxer pre-registers all tracks in
        // the segment builder before emitting the header.  Without this gate,
        // audio-only samples can trigger a flush before the first video
        // keyframe arrives, producing an init segment that omits the video
        // track entirely and silently breaking downstream playback.
        //
        // Once the init segment has been sent, subsequent flushes are
        // unconditional.  We also allow the flush if a missing track's input
        // has already closed (the stream truly is single-track).
        if seg.pending_samples.len() >= FMP4_SEGMENT_FLUSH_THRESHOLD
            && should_flush_fmp4_segment(&seg, &inputs)
        {
            let stopped =
                flush_fmp4_segment(session, &mut muxer, &mut seg, context, stats_tracker).await?;
            if stopped {
                return Ok(());
            }
        }
    }

    // Flush any remaining samples.
    if !seg.pending_samples.is_empty() {
        flush_fmp4_segment(session, &mut muxer, &mut seg, context, stats_tracker).await?;
    }

    tracing::info!("Mp4MuxerNode stream mode: processed {packet_count} packets");
    Ok(())
}

/// Decide whether the fMP4 segment should be flushed now.
///
/// After the init segment has been sent, flushes are unconditional.  Before
/// the init segment, we defer until all expected tracks have registered their
/// sample entries (so the moov describes every track).  A safety cap prevents
/// unbounded accumulation when a misconfigured pipeline never sends data for
/// an expected track.
///
/// Returns `true` when the caller should flush, `false` when it should
/// `continue` the receive loop.
fn should_flush_fmp4_segment(seg: &Fmp4SegmentState, inputs: &MuxInputs) -> bool {
    if seg.init_sent {
        return true;
    }

    let video_ready = !inputs.tp.video || seg.video_sample_entry_sent || inputs.tg.video_done;
    let audio_ready = !inputs.tp.audio || seg.audio_sample_entry_sent || inputs.tg.audio_done;

    if video_ready && audio_ready {
        return true;
    }

    // Safety cap: force flush with a warning rather than accumulating without
    // bound when an expected track never sends data.
    if seg.pending_samples.len() >= FMP4_FIRST_FLUSH_DEFER_CAP {
        tracing::warn!(
            "First fMP4 flush deferred too long ({} pending, cap={}). \
             Forcing flush with available tracks \
             (video_ready={video_ready}, audio_ready={audio_ready}).",
            seg.pending_samples.len(),
            FMP4_FIRST_FLUSH_DEFER_CAP,
        );
        return true;
    }

    tracing::debug!(
        "Deferring first fMP4 flush: waiting for all expected tracks \
         (video_ready={video_ready}, audio_ready={audio_ready}, \
         pending={})",
        seg.pending_samples.len(),
    );
    false
}

/// Flush accumulated samples as a single fMP4 media segment.
///
/// Before creating segment metadata, samples and payloads are partitioned by
/// track (all video first, then all audio) with data offsets recomputed so
/// each track's data is contiguous.  This is required by `shiguredo_mp4`
/// which expects per-track contiguous byte ranges within a segment.
///
/// Returns `true` if the output channel is closed (caller should stop).
async fn flush_fmp4_segment(
    session: &MuxSession<'_>,
    muxer: &mut Fmp4SegmentMuxer,
    seg: &mut Fmp4SegmentState,
    context: &mut NodeContext,
    stats_tracker: &mut NodeStatsTracker,
) -> Result<bool, StreamKitError> {
    // Partition samples+payloads by track so each track's data is contiguous.
    let (sorted_samples, sorted_payloads) = partition_samples_by_track(
        std::mem::take(&mut seg.pending_samples),
        std::mem::take(&mut seg.pending_payloads),
    );

    let segment_metadata = muxer.create_media_segment_metadata(&sorted_samples).map_err(|e| {
        let msg = format!("Failed to create fMP4 segment metadata: {e}");
        state_helpers::emit_failed(&context.state_tx, session.node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    // Build segment bytes: [moof+mdat header] + [payload data]
    let payload_size: usize = sorted_samples.iter().map(|s| s.data_size).sum();
    let mut segment_bytes = Vec::with_capacity(segment_metadata.len() + payload_size);
    segment_bytes.extend_from_slice(&segment_metadata);
    for payload in &sorted_payloads {
        segment_bytes.extend_from_slice(payload);
    }

    let ct = Some(session.content_type.into());

    if seg.init_sent {
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
            state_helpers::emit_failed(&context.state_tx, session.node_name, &msg);
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
        seg.init_sent = true;
    }

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
async fn run_file_mode(
    session: &MuxSession<'_>,
    context: &mut NodeContext,
    stats_tracker: &mut NodeStatsTracker,
    mut inputs: MuxInputs,
) -> Result<(), StreamKitError> {
    let muxer = Mp4FileMuxer::new().map_err(|e| {
        let msg = format!("Failed to create Mp4FileMuxer: {e}");
        state_helpers::emit_failed(&context.state_tx, session.node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    let mut file_buf = FileBackedBuffer::new().map_err(|e| {
        let msg = format!("Failed to create temp file for MP4 file mode: {e}");
        state_helpers::emit_failed(&context.state_tx, session.node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    // Write initial boxes (ftyp + placeholder moov) to temp file.
    let initial = muxer.initial_boxes_bytes();
    file_buf
        .write_all(initial)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write initial MP4 boxes: {e}")))?;

    let (video_timescale, audio_timescale) = resolve_timescales(session.config);
    let (video_sample_entry, audio_sample_entry) =
        build_sample_entries(session.config, session.audio_codec, session.video_codec);

    let mut state = FileMuxState {
        muxer,
        file_buf,
        video_sample_entry,
        audio_sample_entry,
        video_timescale,
        audio_timescale,
        video_keyframe_seen: false,
        video_sample_entry_sent: false,
        audio_sample_entry_sent: false,
        packet_count: 0,
        h264_scratch: Bytes::new(),
    };

    let mut inputs_open =
        if inputs.tp.skip_classification { inputs.all_receivers.len() } else { 0 };

    while !all_inputs_done(inputs.tp, &inputs.tg, inputs_open) {
        let Some(frame) = receive_frame(
            context,
            &mut inputs.audio_rx,
            &mut inputs.video_rx,
            &mut inputs.all_receivers,
            &mut inputs_open,
            &inputs.tp,
            &inputs.tg,
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
                inputs.tg.audio_done = true;
            },
            MuxFrame::VideoClosed => {
                tracing::info!("Mp4MuxerNode file mode: video closed");
                inputs.tg.video_done = true;
            },
            MuxFrame::Video(data, metadata) => {
                process_file_video_frame(
                    &data,
                    metadata.as_ref(),
                    &mut state,
                    stats_tracker,
                    session.video_codec,
                    session.config.video_width,
                    session.config.video_height,
                )?;
            },
            MuxFrame::Audio(data, metadata) => {
                process_file_audio_frame(
                    &data,
                    metadata.as_ref(),
                    session.audio_codec,
                    &mut state,
                    stats_tracker,
                )?;
            },
        }
    }

    tracing::info!(
        "Mp4MuxerNode file mode: all inputs closed, finalizing ({} packets)",
        state.packet_count,
    );

    finalize_file_mode(
        &mut state.muxer,
        &mut state.file_buf,
        context,
        session.content_type,
        stats_tracker,
        session.node_name,
    )
    .await
}

/// Process a single video frame in file mode.
fn process_file_video_frame(
    data: &Bytes,
    metadata: Option<&PacketMetadata>,
    state: &mut FileMuxState,
    stats_tracker: &mut NodeStatsTracker,
    video_codec: VideoCodec,
    video_width: u16,
    video_height: u16,
) -> Result<(), StreamKitError> {
    let is_keyframe = metadata.and_then(|m| m.keyframe).unwrap_or(false);

    if !state.video_keyframe_seen {
        if is_keyframe {
            state.video_keyframe_seen = true;
        } else {
            return Ok(());
        }
    }

    state.packet_count += 1;
    stats_tracker.received();

    let duration_us =
        metadata.and_then(|m| m.duration_us).unwrap_or(DEFAULT_VIDEO_FRAME_DURATION_US);
    let duration_ticks = us_to_ticks(duration_us, state.video_timescale.get());

    // Convert Annex B → AVCC for H.264 streams.
    let write_data: &[u8] = if video_codec == VideoCodec::H264 {
        let conv = convert_annexb_to_avcc(data);
        // On the first keyframe, update the sample entry with real SPS/PPS.
        if !conv.sps_list.is_empty() && !state.video_sample_entry_sent {
            state.video_sample_entry = rebuild_avc1_entry_from_params(
                video_width,
                video_height,
                conv.sps_list,
                conv.pps_list,
            );
        }
        // Store converted data in a temporary so we can borrow it below.
        state.h264_scratch = conv.data;
        &state.h264_scratch
    } else {
        data
    };

    let data_offset = state
        .file_buf
        .position()
        .map_err(|e| StreamKitError::Runtime(format!("Failed to get file position: {e}")))?;
    state
        .file_buf
        .write_all(write_data)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write video data: {e}")))?;

    let entry = if state.video_sample_entry_sent {
        None
    } else {
        state.video_sample_entry_sent = true;
        Some(state.video_sample_entry.clone())
    };

    state
        .muxer
        .append_sample(&Sample {
            track_kind: TrackKind::Video,
            timescale: state.video_timescale,
            sample_entry: entry,
            duration: duration_ticks,
            keyframe: is_keyframe,
            composition_time_offset: None,
            data_offset,
            data_size: write_data.len(),
        })
        .map_err(|e| StreamKitError::Runtime(format!("Failed to append video sample: {e}")))?;

    Ok(())
}

/// Process a single audio frame in file mode.
fn process_file_audio_frame(
    data: &Bytes,
    metadata: Option<&PacketMetadata>,
    audio_codec: AudioCodec,
    state: &mut FileMuxState,
    stats_tracker: &mut NodeStatsTracker,
) -> Result<(), StreamKitError> {
    state.packet_count += 1;
    stats_tracker.received();

    let duration_us = metadata
        .and_then(|m| m.duration_us)
        .unwrap_or_else(|| default_audio_frame_duration_us(audio_codec));
    let duration_ticks = us_to_ticks(duration_us, state.audio_timescale.get());

    let data_offset = state
        .file_buf
        .position()
        .map_err(|e| StreamKitError::Runtime(format!("Failed to get file position: {e}")))?;
    state
        .file_buf
        .write_all(data)
        .map_err(|e| StreamKitError::Runtime(format!("Failed to write audio data: {e}")))?;

    let entry = if state.audio_sample_entry_sent {
        None
    } else {
        state.audio_sample_entry_sent = true;
        Some(state.audio_sample_entry.clone())
    };

    state
        .muxer
        .append_sample(&Sample {
            track_kind: TrackKind::Audio,
            timescale: state.audio_timescale,
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
///
/// When a channel closes (`recv()` returns `None`), it is removed from
/// `all_receivers` via `Vec::remove(idx)`.  Because `tokio::select!` fires
/// at most one branch per invocation, only one removal can happen per call.
/// The `len() >= 2` guard at the top means we never re-enter the two-channel
/// path after a removal has shrunk the vec to 1, so the index arithmetic
/// stays correct despite the shifting caused by `remove(0)`.
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
        "Muxes H.264/AV1 video and/or AAC/Opus audio into an MP4 container. \
         Supports fragmented MP4 (fMP4) for DASH/HLS streaming and \
         regular MP4 file output with fast-start.",
    );
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
        // H.264 + Opus
        assert_eq!(
            mp4_content_type(Some(AudioCodec::Opus), Some(VideoCodec::H264)),
            "video/mp4; codecs=\"avc1,opus\""
        );
        // H.264 video only
        assert_eq!(mp4_content_type(None, Some(VideoCodec::H264)), "video/mp4; codecs=\"avc1\"");
        // AV1 + Opus
        assert_eq!(
            mp4_content_type(Some(AudioCodec::Opus), Some(VideoCodec::Av1)),
            "video/mp4; codecs=\"av01,opus\""
        );
        // AV1 video only
        assert_eq!(mp4_content_type(None, Some(VideoCodec::Av1)), "video/mp4; codecs=\"av01\"");
        // Audio-only (Opus)
        assert_eq!(mp4_content_type(Some(AudioCodec::Opus), None), "audio/mp4; codecs=\"opus\"");
        // No tracks
        assert_eq!(mp4_content_type(None, None), "video/mp4");
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
    fn build_av01_sample_entry_produces_av01() {
        let entry = build_av01_sample_entry(1280, 720, None);
        assert!(matches!(entry, SampleEntry::Av01(_)));
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
                sample_entry: if seg_idx == 0 { Some(sample_entry.clone()) } else { None },
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

    /// Verify `partition_samples_by_track` reorders interleaved A/V samples
    /// so that video data comes first with contiguous offsets, followed by
    /// audio data with contiguous offsets.
    #[test]
    fn partition_samples_by_track_reorders_interleaved() {
        let vts = NonZeroU32::new(90_000).unwrap();
        let ats = NonZeroU32::new(48_000).unwrap();
        let ve = build_avc1_sample_entry(320, 240, None);
        let ae = build_opus_sample_entry(48_000, 2);

        // Simulate interleaved arrival: V(1024) A(256) V(1024) A(256)
        let samples = vec![
            Sample {
                track_kind: TrackKind::Video,
                timescale: vts,
                sample_entry: Some(ve),
                duration: 3000,
                keyframe: true,
                composition_time_offset: None,
                data_offset: 0,
                data_size: 1024,
            },
            Sample {
                track_kind: TrackKind::Audio,
                timescale: ats,
                sample_entry: Some(ae),
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: 1024,
                data_size: 256,
            },
            Sample {
                track_kind: TrackKind::Video,
                timescale: vts,
                sample_entry: None,
                duration: 3000,
                keyframe: false,
                composition_time_offset: None,
                data_offset: 1280,
                data_size: 1024,
            },
            Sample {
                track_kind: TrackKind::Audio,
                timescale: ats,
                sample_entry: None,
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: 2304,
                data_size: 256,
            },
        ];
        let payloads: Vec<Bytes> = vec![
            Bytes::from(vec![0xAAu8; 1024]),
            Bytes::from(vec![0xBBu8; 256]),
            Bytes::from(vec![0xCCu8; 1024]),
            Bytes::from(vec![0xDDu8; 256]),
        ];

        let (sorted_s, sorted_p) = partition_samples_by_track(samples, payloads);

        // Expect: V, V, A, A
        assert_eq!(sorted_s.len(), 4);
        assert!(matches!(sorted_s[0].track_kind, TrackKind::Video));
        assert!(matches!(sorted_s[1].track_kind, TrackKind::Video));
        assert!(matches!(sorted_s[2].track_kind, TrackKind::Audio));
        assert!(matches!(sorted_s[3].track_kind, TrackKind::Audio));

        // Video offsets: 0, 1024; Audio offsets: 2048, 2048+256=2304
        assert_eq!(sorted_s[0].data_offset, 0);
        assert_eq!(sorted_s[1].data_offset, 1024);
        assert_eq!(sorted_s[2].data_offset, 2048);
        assert_eq!(sorted_s[3].data_offset, 2304);

        // Payloads reordered to match
        assert_eq!(sorted_p[0][0], 0xAA);
        assert_eq!(sorted_p[1][0], 0xCC);
        assert_eq!(sorted_p[2][0], 0xBB);
        assert_eq!(sorted_p[3][0], 0xDD);
    }

    /// Round-trip test: interleaved A/V samples in fMP4 mode.
    ///
    /// This test sends multiple interleaved video+audio samples per segment
    /// (the common real-world case) and verifies that partitioning produces
    /// a valid segment that can be demuxed.
    #[test]
    fn fmp4_round_trip_interleaved_audio_video() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let audio_timescale = NonZeroU32::new(48_000).unwrap();
        let video_entry = build_avc1_sample_entry(320, 240, None);
        let audio_entry = build_opus_sample_entry(48_000, 2);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        // Create interleaved samples: V A V A V A (6 samples, common real-world pattern)
        let mut samples = Vec::new();
        let mut payloads: Vec<Bytes> = Vec::new();
        let mut offset: u64 = 0;
        for i in 0..3u8 {
            let vdata = vec![0x10 + i; 512];
            let adata = vec![0x80 + i; 128];

            samples.push(Sample {
                track_kind: TrackKind::Video,
                timescale: video_timescale,
                sample_entry: if i == 0 { Some(video_entry.clone()) } else { None },
                duration: 3000,
                keyframe: i == 0,
                composition_time_offset: None,
                data_offset: offset,
                data_size: vdata.len(),
            });
            offset += vdata.len() as u64;
            payloads.push(Bytes::from(vdata));

            samples.push(Sample {
                track_kind: TrackKind::Audio,
                timescale: audio_timescale,
                sample_entry: if i == 0 { Some(audio_entry.clone()) } else { None },
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: offset,
                data_size: adata.len(),
            });
            offset += adata.len() as u64;
            payloads.push(Bytes::from(adata));
        }

        // Without partition, create_media_segment_metadata would fail because
        // video offsets are non-contiguous (audio data sits between them).
        let (sorted_samples, sorted_payloads) = partition_samples_by_track(samples, payloads);

        let metadata = muxer.create_media_segment_metadata(&sorted_samples).unwrap();
        let mut segment = metadata;
        for p in &sorted_payloads {
            segment.extend_from_slice(p);
        }

        let init = muxer.init_segment_bytes().unwrap();

        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();

        let tracks = demuxer.tracks().unwrap();
        assert_eq!(tracks.len(), 2, "Should have video + audio tracks");

        let segment_result = demuxer.handle_media_segment(&segment).unwrap();
        assert_eq!(segment_result.len(), 6, "Should have 6 samples (3 video + 3 audio)");
    }

    /// Round-trip test: audio-only fMP4 (matches the mp4_mux_audio.yml pipeline path).
    #[test]
    fn fmp4_round_trip_audio_only() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let audio_timescale = NonZeroU32::new(48_000).unwrap();
        let audio_entry = build_opus_sample_entry(48_000, 2);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        let mut samples = Vec::new();
        let mut payloads: Vec<Bytes> = Vec::new();
        let mut offset: u64 = 0;
        for i in 0..5u8 {
            let data = vec![0xA0 + i; 128];
            samples.push(Sample {
                track_kind: TrackKind::Audio,
                timescale: audio_timescale,
                sample_entry: if i == 0 { Some(audio_entry.clone()) } else { None },
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: offset,
                data_size: data.len(),
            });
            offset += data.len() as u64;
            payloads.push(Bytes::from(data));
        }

        // Audio-only: partition is a no-op but should still work correctly.
        let (sorted_samples, sorted_payloads) = partition_samples_by_track(samples, payloads);

        let metadata = muxer.create_media_segment_metadata(&sorted_samples).unwrap();
        let mut segment = metadata;
        for p in &sorted_payloads {
            segment.extend_from_slice(p);
        }

        let init = muxer.init_segment_bytes().unwrap();

        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();

        let tracks = demuxer.tracks().unwrap();
        assert_eq!(tracks.len(), 1, "Should have audio track only");

        let segment_result = demuxer.handle_media_segment(&segment).unwrap();
        assert_eq!(segment_result.len(), 5, "Should have 5 audio samples");
    }

    /// Multi-segment round-trip: verifies that sample_entry dedup flags work
    /// correctly across multiple fMP4 segments.
    ///
    /// The sample entry is sent only with the first segment; subsequent segments
    /// pass `sample_entry: None`.  `shiguredo_mp4` remembers the entry in its
    /// internal `TrackEntry::sample_entries` vec and reuses it via
    /// `current_sample_entry_index`.  This test proves the dedup is safe by
    /// creating three segments and demuxing all of them.
    #[test]
    fn fmp4_multi_segment_sample_entry_dedup() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let audio_timescale = NonZeroU32::new(48_000).unwrap();
        let video_entry = build_avc1_sample_entry(320, 240, None);
        let audio_entry = build_opus_sample_entry(48_000, 2);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        // Build 3 segments.  Only the first segment carries sample entries.
        let mut segments: Vec<Vec<u8>> = Vec::new();
        for seg_idx in 0u8..3 {
            let vdata = vec![0x10 + seg_idx; 512];
            let adata = vec![0x80 + seg_idx; 128];

            let samples = vec![
                Sample {
                    track_kind: TrackKind::Video,
                    timescale: video_timescale,
                    sample_entry: if seg_idx == 0 { Some(video_entry.clone()) } else { None },
                    duration: 3000,
                    keyframe: true,
                    composition_time_offset: None,
                    data_offset: 0,
                    data_size: vdata.len(),
                },
                Sample {
                    track_kind: TrackKind::Audio,
                    timescale: audio_timescale,
                    sample_entry: if seg_idx == 0 { Some(audio_entry.clone()) } else { None },
                    duration: 960,
                    keyframe: true,
                    composition_time_offset: None,
                    data_offset: vdata.len() as u64,
                    data_size: adata.len(),
                },
            ];

            let metadata = muxer
                .create_media_segment_metadata(&samples)
                .unwrap_or_else(|e| panic!("segment {seg_idx} metadata failed: {e}"));
            let mut seg_bytes = metadata;
            seg_bytes.extend_from_slice(&vdata);
            seg_bytes.extend_from_slice(&adata);
            segments.push(seg_bytes);
        }

        let init = muxer.init_segment_bytes().unwrap();

        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();

        let tracks = demuxer.tracks().unwrap();
        assert_eq!(tracks.len(), 2, "Init segment should describe 2 tracks");

        // Demux all 3 segments — each should produce 2 samples.
        for (i, seg) in segments.iter().enumerate() {
            let result = demuxer
                .handle_media_segment(seg)
                .unwrap_or_else(|e| panic!("segment {i} demux failed: {e}"));
            assert_eq!(result.len(), 2, "Segment {i} should have 2 samples (1 video + 1 audio)");
        }
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

    /// Verify that the fMP4 init segment includes both tracks even when the
    /// first `create_media_segment_metadata` call contains only audio samples.
    ///
    /// This simulates the real-world scenario where audio frames arrive before
    /// the first video keyframe.  Without the flush-gating fix in
    /// `run_stream_mode`, the init segment would only describe the audio
    /// track, silently breaking video playback.
    #[test]
    fn fmp4_init_segment_includes_both_tracks_when_audio_arrives_first() {
        use shiguredo_mp4::demux::Fmp4SegmentDemuxer;

        let video_timescale = NonZeroU32::new(90_000).unwrap();
        let audio_timescale = NonZeroU32::new(48_000).unwrap();
        let video_entry = build_avc1_sample_entry(640, 480, None);
        let audio_entry = build_opus_sample_entry(48_000, 2);

        let mut muxer = Fmp4SegmentMuxer::new().unwrap();

        // --- Segment 1: audio-only (simulates audio arriving before video keyframe) ---
        // NOTE: We intentionally include the video sample_entry in this first
        // segment even though the video data hasn't arrived yet.  This mirrors
        // what the flush-gating fix does: it defers the first flush until both
        // track entries are present, so by the time we call
        // `create_media_segment_metadata` the accumulated samples include at
        // least one from each expected track.
        //
        // To properly test the gate, we include one video sample alongside the
        // audio samples in the first segment — representing the deferred flush
        // that finally fires once the video keyframe arrives.
        let video_data = vec![0xFFu8; 1024];

        let mut samples = Vec::new();
        let mut payloads: Vec<Bytes> = Vec::new();

        // 30 audio samples (simulating accumulation while waiting for video)
        let mut offset: u64 = 0;
        for i in 0..30u8 {
            let data = vec![0xA0 + (i % 16); 128];
            samples.push(Sample {
                track_kind: TrackKind::Audio,
                timescale: audio_timescale,
                sample_entry: if i == 0 { Some(audio_entry.clone()) } else { None },
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: offset,
                data_size: data.len(),
            });
            offset += data.len() as u64;
            payloads.push(Bytes::from(data));
        }

        // 1 video keyframe (the gate lifts and flush fires)
        samples.push(Sample {
            track_kind: TrackKind::Video,
            timescale: video_timescale,
            sample_entry: Some(video_entry),
            duration: 3000,
            keyframe: true,
            composition_time_offset: None,
            data_offset: offset,
            data_size: video_data.len(),
        });
        payloads.push(Bytes::from(video_data.clone()));

        // Partition by track (video first, then audio) as the muxer does.
        let (sorted_samples, sorted_payloads) = partition_samples_by_track(samples, payloads);

        let metadata = muxer.create_media_segment_metadata(&sorted_samples).unwrap();
        let mut segment = metadata;
        for p in &sorted_payloads {
            segment.extend_from_slice(p);
        }

        // The init segment should now describe BOTH tracks.
        let init = muxer.init_segment_bytes().unwrap();

        let mut demuxer = Fmp4SegmentDemuxer::new();
        demuxer.handle_init_segment(&init).unwrap();

        let tracks = demuxer.tracks().unwrap();
        assert_eq!(
            tracks.len(),
            2,
            "Init segment must include both video and audio tracks \
             even when audio samples arrived first"
        );

        let segment_result = demuxer.handle_media_segment(&segment).unwrap();
        assert_eq!(segment_result.len(), 31, "Should have 30 audio + 1 video sample");

        // --- Segment 2: subsequent A/V segment (no sample entries needed) ---
        let mut samples2 = Vec::new();
        let mut payloads2: Vec<Bytes> = Vec::new();
        let mut offset2: u64 = 0;

        for _ in 0..5u8 {
            let data = vec![0xBBu8; 128];
            samples2.push(Sample {
                track_kind: TrackKind::Audio,
                timescale: audio_timescale,
                sample_entry: None,
                duration: 960,
                keyframe: true,
                composition_time_offset: None,
                data_offset: offset2,
                data_size: data.len(),
            });
            offset2 += data.len() as u64;
            payloads2.push(Bytes::from(data));
        }
        samples2.push(Sample {
            track_kind: TrackKind::Video,
            timescale: video_timescale,
            sample_entry: None,
            duration: 3000,
            keyframe: false,
            composition_time_offset: None,
            data_offset: offset2,
            data_size: video_data.len(),
        });
        payloads2.push(Bytes::from(video_data));

        let (sorted2, sorted_p2) = partition_samples_by_track(samples2, payloads2);
        let meta2 = muxer.create_media_segment_metadata(&sorted2).unwrap();
        let mut seg2 = meta2;
        for p in &sorted_p2 {
            seg2.extend_from_slice(p);
        }
        let result2 = demuxer.handle_media_segment(&seg2).unwrap();
        assert_eq!(result2.len(), 6, "Subsequent segment should have 5 audio + 1 video");
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

    // -----------------------------------------------------------------------
    // Annex B → AVCC conversion tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_annexb_single_nal_4byte_sc() {
        // 4-byte start code + 3-byte NAL payload
        let data = [0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB];
        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 1);
        assert_eq!(nals[0], &[0x65, 0xAA, 0xBB]);
    }

    #[test]
    fn parse_annexb_single_nal_3byte_sc() {
        // 3-byte start code + 2-byte NAL payload
        let data = [0x00, 0x00, 0x01, 0x41, 0xCC];
        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 1);
        assert_eq!(nals[0], &[0x41, 0xCC]);
    }

    #[test]
    fn parse_annexb_multiple_nals() {
        // SPS + PPS + IDR slice with 4-byte start codes (typical OpenH264 output)
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // sc
        data.extend_from_slice(&[0x67, 0x42, 0xC0, 0x1F]); // SPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // sc
        data.extend_from_slice(&[0x68, 0xCE, 0x38, 0x80]); // PPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // sc
        data.extend_from_slice(&[0x65, 0x11, 0x22]); // IDR slice

        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(nals[0], &[0x67, 0x42, 0xC0, 0x1F]); // SPS
        assert_eq!(nals[1], &[0x68, 0xCE, 0x38, 0x80]); // PPS
        assert_eq!(nals[2], &[0x65, 0x11, 0x22]); // IDR
    }

    #[test]
    fn parse_annexb_mixed_start_codes() {
        // Mix of 3-byte and 4-byte start codes
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // 4-byte sc
        data.extend_from_slice(&[0x67, 0x42]); // SPS (short)
        data.extend_from_slice(&[0x00, 0x00, 0x01]); // 3-byte sc
        data.extend_from_slice(&[0x68, 0xCE]); // PPS (short)

        let nals = parse_annexb_nal_units(&data);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0], &[0x67, 0x42]);
        assert_eq!(nals[1], &[0x68, 0xCE]);
    }

    #[test]
    fn parse_annexb_empty_input() {
        let nals = parse_annexb_nal_units(&[]);
        assert!(nals.is_empty());
    }

    #[test]
    fn parse_annexb_no_start_code() {
        let data = [0x01, 0x02, 0x03];
        let nals = parse_annexb_nal_units(&data);
        assert!(nals.is_empty());
    }

    #[test]
    fn convert_annexb_to_avcc_basic() {
        // SPS + PPS + IDR
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let sps = [0x67, 0x42, 0xC0, 0x1F];
        annexb.extend_from_slice(&sps);
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let pps = [0x68, 0xCE, 0x38, 0x80];
        annexb.extend_from_slice(&pps);
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let idr = [0x65, 0x11, 0x22, 0x33];
        annexb.extend_from_slice(&idr);

        let result = convert_annexb_to_avcc(&annexb);

        // Check SPS/PPS extraction
        assert_eq!(result.sps_list.len(), 1);
        assert_eq!(result.pps_list.len(), 1);
        assert_eq!(result.sps_list[0], sps);
        assert_eq!(result.pps_list[0], pps);

        // Check AVCC output: each NAL prefixed with 4-byte BE length
        let avcc = &result.data[..];
        let mut offset = 0;
        for expected_nal in &[&sps[..], &pps[..], &idr[..]] {
            let len = u32::from_be_bytes([
                avcc[offset],
                avcc[offset + 1],
                avcc[offset + 2],
                avcc[offset + 3],
            ]) as usize;
            offset += 4;
            assert_eq!(len, expected_nal.len());
            assert_eq!(&avcc[offset..offset + len], *expected_nal);
            offset += len;
        }
        assert_eq!(offset, avcc.len());
    }

    #[test]
    fn convert_annexb_to_avcc_non_idr_has_no_params() {
        // A non-IDR P-frame has no SPS/PPS
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annexb.extend_from_slice(&[0x41, 0xAA, 0xBB]); // NAL type 1 (non-IDR slice)

        let result = convert_annexb_to_avcc(&annexb);
        assert!(result.sps_list.is_empty());
        assert!(result.pps_list.is_empty());

        // AVCC output should have one length-prefixed NAL
        assert_eq!(result.data.len(), 4 + 3);
        let len =
            u32::from_be_bytes([result.data[0], result.data[1], result.data[2], result.data[3]]);
        assert_eq!(len, 3);
    }

    #[test]
    fn rebuild_avc1_entry_extracts_profile_level() {
        let sps = vec![0x67, 0x42, 0xC0, 0x1F]; // profile=66, compat=0xC0, level=31
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let entry = rebuild_avc1_entry_from_params(640, 480, vec![sps.clone()], vec![pps.clone()]);
        match entry {
            SampleEntry::Avc1(avc1) => {
                assert_eq!(avc1.avcc_box.avc_profile_indication, 66);
                assert_eq!(avc1.avcc_box.profile_compatibility, 0xC0);
                assert_eq!(avc1.avcc_box.avc_level_indication, 31);
                assert_eq!(avc1.avcc_box.sps_list, vec![sps]);
                assert_eq!(avc1.avcc_box.pps_list, vec![pps]);
                assert_eq!(avc1.visual.width, 640);
                assert_eq!(avc1.visual.height, 480);
            },
            other => panic!("Expected Avc1, got {other:?}"),
        }
    }
}
