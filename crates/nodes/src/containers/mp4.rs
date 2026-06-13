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
use std::io::{Seek, SeekFrom, Write};
use std::num::{NonZeroU32, NonZeroUsize};
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

use super::file_stream::{emit_file_in_chunks, resolve_finalize_chunk_size, FileBackedBuffer};
use crate::video::DEFAULT_VIDEO_FRAME_DURATION_US;

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

/// Hard upper bound for skip-classification deferral.  Even when inputs are
/// still open, force-flush after this many pending samples to prevent
/// unbounded memory growth from pathological misconfiguration.
/// 10× the normal cap = 3,000 samples ≈ 1 minute of audio at typical
/// AAC frame rates (~47 frames/sec).
const FMP4_SKIP_CLASS_HARD_CAP: usize = 10 * FMP4_FIRST_FLUSH_DEFER_CAP;

/// Build an AVC1 (H.264) sample entry box from SPS/PPS NAL units.
///
/// If `codec_private` is available, it is expected to contain an AVCDecoderConfigurationRecord
/// or raw SPS/PPS NAL units.  Otherwise a minimal placeholder is used.
fn build_avc1_sample_entry(width: u16, height: u16, codec_private: Option<&[u8]>) -> SampleEntry {
    let (sps_list, pps_list, profile, compat, level) = codec_private.map_or_else(
        || (vec![vec![0x67, 0x42, 0xc0, 0x1f]], vec![vec![0x68, 0xce, 0x38, 0x80]], 66, 0xC0, 31),
        parse_avcc_codec_private,
    );

    // For High profile and above (anything other than Baseline 66,
    // Main 77, Extended 88), the avcC box requires chroma_format and
    // bit-depth fields.  Default to 4:2:0 / 8-bit which matches NV12.
    let needs_chroma_fields = !matches!(profile, 66 | 77 | 88);
    let chroma_format = if needs_chroma_fields { Some(Uint::new(1)) } else { None };
    let bit_depth_luma_minus8 = if needs_chroma_fields { Some(Uint::new(0)) } else { None };
    let bit_depth_chroma_minus8 = if needs_chroma_fields { Some(Uint::new(0)) } else { None };

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
            chroma_format,
            bit_depth_luma_minus8,
            bit_depth_chroma_minus8,
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

    // For High profile and above (anything other than Baseline 66,
    // Main 77, Extended 88), the avcC box requires chroma_format and
    // bit-depth fields.  Default to 4:2:0 / 8-bit which matches NV12
    // input — the standard output of HW encoders.
    let needs_chroma_fields = !matches!(profile, 66 | 77 | 88);
    let chroma_format = if needs_chroma_fields { Some(Uint::new(1)) } else { None };
    let bit_depth_luma_minus8 = if needs_chroma_fields { Some(Uint::new(0)) } else { None };
    let bit_depth_chroma_minus8 = if needs_chroma_fields { Some(Uint::new(0)) } else { None };

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
            chroma_format,
            bit_depth_luma_minus8,
            bit_depth_chroma_minus8,
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
#[serde(default, deny_unknown_fields)]
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
    /// content-type resolution.  When omitted the codec is auto-detected from
    /// the upstream `EncodedAudio` pin type; if detection fails it falls back
    /// to `Opus`.
    #[serde(default)]
    pub audio_codec: Option<AudioCodec>,

    /// Override the video codec used for the pre-connection MIME content-type
    /// hint.  When omitted, the hint defaults to AV1 (if video dimensions
    /// are set).  The runtime MIME type is always resolved from the actual
    /// input codec.
    #[serde(default)]
    pub video_codec: Option<VideoCodec>,

    /// Size (in bytes) of each `Packet::Binary` chunk emitted when a File-mode
    /// output is streamed downstream at finalization.  Larger values cut
    /// per-packet overhead; smaller values lower peak memory and can be aligned
    /// to a downstream sink's preferred unit (e.g. an object-store multipart
    /// part size).  Defaults to 256 KiB when unset.
    #[serde(default)]
    pub finalize_chunk_size: Option<NonZeroUsize>,
}

impl Mp4MuxerConfig {
    /// File-mode finalize chunk size, falling back to the shared default.
    fn finalize_chunk_size(&self) -> usize {
        resolve_finalize_chunk_size(self.finalize_chunk_size)
    }
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
            video_codec: None,
            finalize_chunk_size: None,
        }
    }
}

enum MuxFrame {
    Audio(Bytes, Option<PacketMetadata>),
    Video(Bytes, Option<PacketMetadata>),
    AudioClosed,
    VideoClosed,
    Shutdown,
}

/// Classify a [`Packet`] as audio or video.
///
/// Handles `Binary` packets (from both native plugins and core encoder
/// nodes) by inspecting the `content_type` field.  `Audio` and `Video`
/// frame packets are classified intrinsically.
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

/// Muxes H.264/AV1 video and/or AAC/Opus audio into an MP4 container
/// (fMP4 streaming or regular file mode).
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
            // Accept Binary packets from native plugins whose C ABI does not
            // yet support EncodedAudio/EncodedVideo discriminants.  The muxer
            // resolves the actual codec from content_type metadata at runtime.
            PacketType::Binary,
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
        // Static pre-connection hint — the runtime content type is resolved
        // from actual input codec types.  The video codec defaults to AV1 when
        // no `video_codec` config is set; this can be overridden for H.264 or
        // VP9 pipelines.
        let video = if self.config.video_width > 0 && self.config.video_height > 0 {
            Some(self.config.video_codec.unwrap_or(VideoCodec::Av1))
        } else {
            None
        };
        let audio = self.config.audio_codec.unwrap_or(AudioCodec::Opus);
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

        let skip_classification = self.config.num_inputs >= 2
            && self.config.video_width > 0
            && self.config.video_height > 0;

        let mut audio_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut video_rx: Option<tokio::sync::mpsc::Receiver<Packet>> = None;
        let mut audio_codec = self.config.audio_codec.unwrap_or(AudioCodec::Opus);
        // Initialise video codec from config (matching audio_codec above).
        // Type resolution from upstream encoders will override this when
        // available, but the config-based default ensures the correct codec
        // is used even when type resolution is unavailable or times out.
        let mut video_codec = self.config.video_codec.unwrap_or(VideoCodec::Av1);
        let mut all_receivers: Vec<tokio::sync::mpsc::Receiver<Packet>> = Vec::new();

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

            if let Some(PacketType::EncodedAudio(fmt)) = pin_type {
                audio_codec = fmt.codec;
            }

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

        tracing::info!(
            "Mp4MuxerNode codec detection complete: audio={audio_codec:?} video={video_codec:?} \
             skip_classification={skip_classification}",
        );

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

/// Accumulate a single video frame into the pending segment state.
///
/// Handles Annex B → AVCC conversion for H.264, sample-entry tracking,
/// and duration calculation.
#[allow(clippy::too_many_arguments)]
fn accumulate_video_sample(
    data: &Bytes,
    metadata: Option<&PacketMetadata>,
    is_keyframe: bool,
    is_h264: bool,
    config: &Mp4MuxerConfig,
    video_timescale: NonZeroU32,
    video_sample_entry: &mut SampleEntry,
    seg: &mut Fmp4SegmentState,
) {
    // Convert Annex B → AVCC for H.264 streams so the mdat
    // contains length-prefixed NAL units matching the avc1 box.
    let data = if is_h264 {
        let conv = convert_annexb_to_avcc(data);
        // On the first keyframe, update the sample entry with
        // real SPS/PPS so the init segment (moov) describes the
        // actual stream parameters instead of placeholders.
        if !conv.sps_list.is_empty() && !seg.video_sample_entry_sent {
            *video_sample_entry = rebuild_avc1_entry_from_params(
                config.video_width,
                config.video_height,
                conv.sps_list,
                conv.pps_list,
            );
        }
        conv.data
    } else {
        data.clone()
    };

    let duration_us =
        metadata.as_ref().and_then(|m| m.duration_us).unwrap_or(DEFAULT_VIDEO_FRAME_DURATION_US);
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
}

/// Accumulate a single audio frame into the pending segment state.
fn accumulate_audio_sample(
    data: Bytes,
    metadata: Option<&PacketMetadata>,
    audio_codec: AudioCodec,
    audio_timescale: NonZeroU32,
    audio_sample_entry: &SampleEntry,
    seg: &mut Fmp4SegmentState,
) {
    let duration_us = metadata
        .and_then(|m| m.duration_us)
        .unwrap_or_else(|| audio_codec.default_frame_duration_us());
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
}

/// Check the keyframe gate for video frames.
///
/// Returns `true` if the frame should be processed, `false` if it should be
/// skipped (still waiting for the first keyframe).
fn check_video_keyframe_gate(is_keyframe: bool, video_keyframe_seen: &mut bool) -> bool {
    if *video_keyframe_seen {
        return true;
    }
    if !is_keyframe {
        tracing::debug!("Mp4MuxerNode: skipping non-keyframe video (waiting for first keyframe)");
        return false;
    }
    tracing::debug!("Mp4MuxerNode: first video keyframe received");
    *video_keyframe_seen = true;
    true
}

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
                if !check_video_keyframe_gate(is_keyframe, &mut video_keyframe_seen) {
                    continue;
                }

                packet_count += 1;
                stats_tracker.received();
                accumulate_video_sample(
                    &data,
                    metadata.as_ref(),
                    is_keyframe,
                    is_h264,
                    session.config,
                    video_timescale,
                    &mut video_sample_entry,
                    &mut seg,
                );
            },
            MuxFrame::Audio(data, metadata) => {
                packet_count += 1;
                stats_tracker.received();
                accumulate_audio_sample(
                    data,
                    metadata.as_ref(),
                    session.audio_codec,
                    audio_timescale,
                    &audio_sample_entry,
                    &mut seg,
                );
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
            && should_flush_fmp4_segment(&seg, &inputs, inputs_open)
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
fn should_flush_fmp4_segment(
    seg: &Fmp4SegmentState,
    inputs: &MuxInputs,
    inputs_open: usize,
) -> bool {
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
        // In skip-classification mode, input channels are not split into
        // separate audio/video receivers so the per-track `*_done` flags
        // are never set during the receive loop.  Instead, check whether
        // any input channels are still open: as long as they are, a
        // slow-starting track (e.g. a video generator initialising fonts)
        // may still produce data.  Force-flushing an init segment that
        // omits an expected track would cause the browser to reject it
        // ("Initialization segment misses expected … track").
        if inputs.tp.skip_classification && inputs_open > 0 {
            // Secondary hard cap: prevent truly unbounded growth from
            // pathological misconfiguration even in skip-classification mode.
            if seg.pending_samples.len() >= FMP4_SKIP_CLASS_HARD_CAP {
                tracing::warn!(
                    "Skip-classification deferral hard cap reached \
                     ({} pending, cap={}). Forcing flush \
                     (video_ready={video_ready}, audio_ready={audio_ready}).",
                    seg.pending_samples.len(),
                    FMP4_SKIP_CLASS_HARD_CAP,
                );
                return true;
            }
            if seg.pending_samples.len().is_multiple_of(FMP4_FIRST_FLUSH_DEFER_CAP) {
                tracing::debug!(
                    "First fMP4 flush deferred: {} pending samples, \
                     {} input(s) still open — waiting for all expected tracks \
                     (video_ready={video_ready}, audio_ready={audio_ready})",
                    seg.pending_samples.len(),
                    inputs_open,
                );
            }
            return false;
        }

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
        session.config.finalize_chunk_size(),
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
        .unwrap_or_else(|| audio_codec.default_frame_duration_us());
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
    chunk_size: usize,
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

    let file = file_buf.finalized_file().map_err(|e| {
        let msg = format!("Failed to read back MP4 file: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    })?;

    emit_file_in_chunks(context, file, chunk_size, content_type.into(), stats_tracker, node_name)
        .await
}

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

/// Convert a duration in microseconds to timescale ticks.
fn us_to_ticks(duration_us: u64, timescale: u32) -> u32 {
    // duration_ticks = duration_us * timescale / 1_000_000
    // Use u64 intermediate to avoid overflow.
    let ticks = duration_us.saturating_mul(u64::from(timescale)) / 1_000_000;
    u32::try_from(ticks).unwrap_or(u32::MAX)
}

use streamkit_core::{config_helpers, registry::StaticPins};

pub fn register_mp4_nodes(registry: &mut NodeRegistry) {
    let default_muxer = Mp4MuxerNode::new(Mp4MuxerConfig::default());
    register_static_node!(
        registry,
        "containers::mp4::muxer",
        |params| {
            let config = config_helpers::parse_config_with_context(params, "Mp4Muxer")?;
            Ok(Box::new(Mp4MuxerNode::new(config)))
        },
        Mp4MuxerConfig,
        StaticPins { inputs: default_muxer.input_pins(), outputs: default_muxer.output_pins() },
        ["containers", "mp4"],
        "Muxes H.264/AV1 video and/or AAC/Opus audio into an MP4 container. \
         Supports fragmented MP4 (fMP4) for DASH/HLS streaming and \
         regular MP4 file output with fast-start.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_utils::{create_test_context, MockOutputSender};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[test]
    fn default_config_is_stream_mode() {
        let config = Mp4MuxerConfig::default();
        assert!(matches!(config.mode, Mp4StreamingMode::Stream));
    }

    #[test]
    fn content_type_combinations() {
        use AudioCodec::{Aac, Opus};
        use VideoCodec::{Av1, Vp9, H264};

        // Vp9 stands in for an unrecognised/future video codec: it is neither
        // Av1 nor H264, so it is dropped from the codecs param.
        let cases: &[(Option<AudioCodec>, Option<VideoCodec>, &str)] = &[
            (Some(Opus), Some(H264), "video/mp4; codecs=\"avc1,opus\""),
            (Some(Aac), Some(H264), "video/mp4; codecs=\"avc1,mp4a\""),
            (Some(Opus), Some(Av1), "video/mp4; codecs=\"av01,opus\""),
            (Some(Aac), Some(Av1), "video/mp4; codecs=\"av01,mp4a\""),
            (Some(Opus), Some(Vp9), "video/mp4; codecs=\"opus\""),
            (Some(Aac), Some(Vp9), "video/mp4; codecs=\"mp4a\""),
            (None, Some(H264), "video/mp4; codecs=\"avc1\""),
            (None, Some(Av1), "video/mp4; codecs=\"av01\""),
            (None, Some(Vp9), "video/mp4"),
            (Some(Opus), None, "audio/mp4; codecs=\"opus\""),
            (Some(Aac), None, "audio/mp4; codecs=\"mp4a\""),
            (None, None, "video/mp4"),
        ];
        for (audio, video, expected) in cases {
            assert_eq!(
                mp4_content_type(*audio, *video),
                *expected,
                "audio={audio:?} video={video:?}"
            );
        }
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

    /// Regression test: the placeholder AVC1 sample entry must have
    /// `profile_compatibility` matching the SPS constraint flags (0xC0),
    /// not zero.  A zero value can cause compliant demuxers to reject
    /// the init segment.
    #[test]
    fn build_avc1_placeholder_has_correct_profile_compat() {
        let entry = build_avc1_sample_entry(640, 480, None);
        match entry {
            SampleEntry::Avc1(avc1) => {
                assert_eq!(
                    avc1.avcc_box.profile_compatibility, 0xC0,
                    "Placeholder profile_compatibility must match SPS constraint flags"
                );
            },
            other => panic!("Expected Avc1, got {other:?}"),
        }
    }

    /// Regression test: serde deserializes `"h264"`, `"avc1"`, `"avc"`, and
    /// `"H264"` as `VideoCodec::H264` thanks to `rename_all = "lowercase"`
    /// and serde aliases on the enum variant.
    ///
    /// Previously `video_codec` was hardcoded to `Av1` regardless of config,
    /// which caused the init segment to contain an `av01` track instead of
    /// `avc1` when type resolution was unavailable.
    #[test]
    fn video_codec_serde_aliases() {
        #[derive(serde::Deserialize)]
        struct Cfg {
            video_codec: VideoCodec,
        }
        // lowercase (rename_all canonical form)
        let h264: Cfg = serde_json::from_str(r#"{"video_codec":"h264"}"#).unwrap();
        assert_eq!(h264.video_codec, VideoCodec::H264);
        // aliases
        let avc1: Cfg = serde_json::from_str(r#"{"video_codec":"avc1"}"#).unwrap();
        assert_eq!(avc1.video_codec, VideoCodec::H264);
        let avc: Cfg = serde_json::from_str(r#"{"video_codec":"avc"}"#).unwrap();
        assert_eq!(avc.video_codec, VideoCodec::H264);
        // PascalCase alias (backward compat with old serialization)
        let pascal: Cfg = serde_json::from_str(r#"{"video_codec":"H264"}"#).unwrap();
        assert_eq!(pascal.video_codec, VideoCodec::H264);
        // Other codecs: case-insensitive via aliases
        let vp9: Cfg = serde_json::from_str(r#"{"video_codec":"VP9"}"#).unwrap();
        assert_eq!(vp9.video_codec, VideoCodec::Vp9);
        let av1_upper: Cfg = serde_json::from_str(r#"{"video_codec":"AV1"}"#).unwrap();
        assert_eq!(av1_upper.video_codec, VideoCodec::Av1);
    }

    /// AudioCodec serde roundtrip: lowercase canonical form plus PascalCase
    /// and uppercase aliases all deserialize correctly.
    #[test]
    fn audio_codec_serde_aliases() {
        #[derive(serde::Deserialize)]
        struct Cfg {
            audio_codec: AudioCodec,
        }
        // lowercase (canonical)
        let opus: Cfg = serde_json::from_str(r#"{"audio_codec":"opus"}"#).unwrap();
        assert_eq!(opus.audio_codec, AudioCodec::Opus);
        let aac: Cfg = serde_json::from_str(r#"{"audio_codec":"aac"}"#).unwrap();
        assert_eq!(aac.audio_codec, AudioCodec::Aac);
        // PascalCase alias (backward compat with old serialization)
        let opus_pascal: Cfg = serde_json::from_str(r#"{"audio_codec":"Opus"}"#).unwrap();
        assert_eq!(opus_pascal.audio_codec, AudioCodec::Opus);
        let aac_pascal: Cfg = serde_json::from_str(r#"{"audio_codec":"Aac"}"#).unwrap();
        assert_eq!(aac_pascal.audio_codec, AudioCodec::Aac);
        // Uppercase alias
        let aac_upper: Cfg = serde_json::from_str(r#"{"audio_codec":"AAC"}"#).unwrap();
        assert_eq!(aac_upper.audio_codec, AudioCodec::Aac);
    }

    /// Verify that `build_sample_entries` produces AVC1 + MP4A when configured
    /// with H264 video and AAC audio — the exact combination used by the
    /// `mp4_mux_aac_h264` oneshot pipeline.
    #[test]
    fn build_sample_entries_h264_aac() {
        let config = Mp4MuxerConfig {
            video_width: 640,
            video_height: 480,
            video_codec: Some(VideoCodec::H264),
            audio_codec: Some(AudioCodec::Aac),
            ..Mp4MuxerConfig::default()
        };
        let audio = config.audio_codec.unwrap_or(AudioCodec::Opus);
        let video = config.video_codec.unwrap_or(VideoCodec::Av1);
        let (video_entry, audio_entry) = build_sample_entries(&config, audio, video);
        assert!(
            matches!(video_entry, SampleEntry::Avc1(_)),
            "Expected AVC1 video entry for H264 config, got {video_entry:?}"
        );
        assert!(
            matches!(audio_entry, SampleEntry::Mp4a(_)),
            "Expected MP4A audio entry for AAC config, got {audio_entry:?}"
        );
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

        // Segment 1: audio-only (audio arrives before video keyframe)
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

        // Segment 2: A/V (no sample entries needed)
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

    fn h264_keyframe_annexb() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        d.extend_from_slice(&[0x67, 0x42, 0xC0, 0x1F]); // SPS
        d.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        d.extend_from_slice(&[0x68, 0xCE, 0x38, 0x80]); // PPS
        d.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        d.extend_from_slice(&[0x65, 0x11, 0x22, 0x33]); // IDR slice
        d
    }

    fn h264_pframe_annexb(byte: u8) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        d.extend_from_slice(&[0x41, byte, byte.wrapping_add(1)]); // non-IDR slice
        d
    }

    fn new_seg_state() -> Fmp4SegmentState {
        Fmp4SegmentState {
            pending_samples: Vec::new(),
            pending_payloads: Vec::new(),
            init_sent: false,
            video_sample_entry_sent: false,
            audio_sample_entry_sent: false,
        }
    }

    fn new_inputs(audio: bool, video: bool, skip: bool) -> MuxInputs {
        MuxInputs {
            audio_rx: None,
            video_rx: None,
            all_receivers: Vec::new(),
            tp: TrackPresence { audio, video, skip_classification: skip },
            tg: TrackProgress { audio_done: false, video_done: false },
        }
    }

    fn n_video_samples(n: usize) -> Vec<Sample> {
        (0..n)
            .map(|_| Sample {
                track_kind: TrackKind::Video,
                timescale: NonZeroU32::new(90_000).unwrap(),
                sample_entry: None,
                duration: 3000,
                keyframe: true,
                composition_time_offset: None,
                data_offset: 0,
                data_size: 10,
            })
            .collect()
    }

    fn meta(duration_us: Option<u64>, keyframe: bool) -> PacketMetadata {
        PacketMetadata { timestamp_us: None, duration_us, sequence: None, keyframe: Some(keyframe) }
    }

    #[test]
    fn classify_packet_audio_video_and_non_av() {
        // Binary with an audio content_type → Audio.
        let audio = classify_packet(Packet::Binary {
            data: Bytes::from_static(b"a"),
            content_type: Some("audio/opus".into()),
            metadata: None,
        });
        assert!(matches!(audio, Some(MuxFrame::Audio(ref d, _)) if &d[..] == b"a"));

        // Binary with a video content_type → Video.
        let video = classify_packet(Packet::Binary {
            data: Bytes::from_static(b"v"),
            content_type: Some("video/h264".into()),
            metadata: None,
        });
        assert!(matches!(video, Some(MuxFrame::Video(ref d, _)) if &d[..] == b"v"));

        // Binary without a content_type → defaults to Audio (with a warning).
        let defaulted = classify_packet(Packet::Binary {
            data: Bytes::from_static(b"x"),
            content_type: None,
            metadata: None,
        });
        assert!(matches!(defaulted, Some(MuxFrame::Audio(_, _))));

        // A non-Binary packet is not classifiable → None.
        assert!(classify_packet(Packet::Text(std::sync::Arc::from("hi"))).is_none());
    }

    #[test]
    fn node_content_type_hint() {
        let av =
            Mp4MuxerNode::new(Mp4MuxerConfig { video_width: 640, video_height: 480, ..default() });
        // No explicit codecs → AV1 video hint + Opus audio default.
        assert_eq!(av.content_type().unwrap(), "video/mp4; codecs=\"av01,opus\"");

        let h264_aac = Mp4MuxerNode::new(Mp4MuxerConfig {
            video_width: 640,
            video_height: 480,
            video_codec: Some(VideoCodec::H264),
            audio_codec: Some(AudioCodec::Aac),
            ..default()
        });
        assert_eq!(h264_aac.content_type().unwrap(), "video/mp4; codecs=\"avc1,mp4a\"");

        // No video dimensions → audio-only hint.
        let audio_only = Mp4MuxerNode::new(Mp4MuxerConfig::default());
        assert_eq!(audio_only.content_type().unwrap(), "audio/mp4; codecs=\"opus\"");
    }

    fn default() -> Mp4MuxerConfig {
        Mp4MuxerConfig::default()
    }

    #[test]
    fn resolve_timescales_defaults_and_overrides() {
        let (v, a) = resolve_timescales(&Mp4MuxerConfig::default());
        assert_eq!(v.get(), 90_000);
        assert_eq!(a.get(), 48_000);

        // Zero is invalid → falls back to the compile-time defaults.
        let zeroed = Mp4MuxerConfig { video_timescale: 0, audio_timescale: 0, ..default() };
        let (v, a) = resolve_timescales(&zeroed);
        assert_eq!(v.get(), 90_000);
        assert_eq!(a.get(), 48_000);

        // Explicit overrides are honoured.
        let custom =
            Mp4MuxerConfig { video_timescale: 30_000, audio_timescale: 44_100, ..default() };
        let (v, a) = resolve_timescales(&custom);
        assert_eq!(v.get(), 30_000);
        assert_eq!(a.get(), 44_100);
    }

    #[test]
    fn finalize_chunk_size_default_and_custom() {
        assert_eq!(Mp4MuxerConfig::default().finalize_chunk_size(), 256 * 1024);
        let custom = Mp4MuxerConfig { finalize_chunk_size: NonZeroUsize::new(4096), ..default() };
        assert_eq!(custom.finalize_chunk_size(), 4096);
    }

    #[test]
    fn keyframe_gate_stays_closed_until_first_keyframe() {
        let mut seen = false;
        // Non-keyframes are dropped while the gate is closed.
        assert!(!check_video_keyframe_gate(false, &mut seen));
        assert!(!seen);
        assert!(!check_video_keyframe_gate(false, &mut seen));
        assert!(!seen);
        // The first keyframe opens the gate.
        assert!(check_video_keyframe_gate(true, &mut seen));
        assert!(seen);
        // Once open, everything passes — including subsequent non-keyframes.
        assert!(check_video_keyframe_gate(false, &mut seen));
        assert!(check_video_keyframe_gate(true, &mut seen));
    }

    #[test]
    fn accumulate_video_sample_non_h264_bookkeeping() {
        let config = Mp4MuxerConfig { video_width: 320, video_height: 240, ..default() };
        let (vts, _) = resolve_timescales(&config);
        let mut entry = build_av01_sample_entry(320, 240, None);
        let mut seg = new_seg_state();
        let data = Bytes::from(vec![0u8; 128]);

        accumulate_video_sample(
            &data,
            Some(&meta(Some(33_333), true)),
            true,
            false,
            &config,
            vts,
            &mut entry,
            &mut seg,
        );

        assert_eq!(seg.pending_samples.len(), 1);
        assert_eq!(seg.pending_payloads.len(), 1);
        let s = &seg.pending_samples[0];
        assert!(matches!(s.track_kind, TrackKind::Video));
        assert_eq!(s.data_size, 128, "non-H.264 payload is stored verbatim");
        assert!(s.keyframe);
        assert_eq!(s.duration, us_to_ticks(33_333, vts.get()));
        assert!(seg.video_sample_entry_sent);
        assert!(s.sample_entry.is_some(), "first sample carries the entry");

        // Second sample: entry already sent → None, and missing duration metadata
        // falls back to the default video frame duration.
        accumulate_video_sample(&data, None, false, false, &config, vts, &mut entry, &mut seg);
        assert_eq!(seg.pending_samples.len(), 2);
        assert!(seg.pending_samples[1].sample_entry.is_none());
        assert!(!seg.pending_samples[1].keyframe);
        assert_eq!(
            seg.pending_samples[1].duration,
            us_to_ticks(DEFAULT_VIDEO_FRAME_DURATION_US, vts.get())
        );
    }

    #[test]
    fn accumulate_video_sample_h264_rebuilds_entry_from_sps() {
        let config = Mp4MuxerConfig { video_width: 320, video_height: 240, ..default() };
        let (vts, _) = resolve_timescales(&config);
        // Start from the placeholder entry; the first keyframe should replace it
        // with one built from the real SPS/PPS in the access unit.
        let mut entry = build_avc1_sample_entry(320, 240, None);
        let mut seg = new_seg_state();
        let data = Bytes::from(h264_keyframe_annexb());

        accumulate_video_sample(
            &data,
            Some(&meta(Some(33_333), true)),
            true,
            true,
            &config,
            vts,
            &mut entry,
            &mut seg,
        );

        match &entry {
            SampleEntry::Avc1(avc1) => {
                assert_eq!(avc1.avcc_box.sps_list.len(), 1);
                assert_eq!(avc1.avcc_box.pps_list.len(), 1);
                assert_eq!(avc1.avcc_box.avc_profile_indication, 0x42);
            },
            other => panic!("expected Avc1 entry, got {other:?}"),
        }
        // The stored payload is the AVCC (length-prefixed) form, not Annex B.
        let stored = &seg.pending_payloads[0];
        assert_eq!(&stored[0..4], &[0x00, 0x00, 0x00, 0x04], "4-byte length prefix for the SPS");
        assert_eq!(seg.pending_samples[0].data_size, stored.len());
    }

    #[test]
    fn accumulate_audio_sample_bookkeeping() {
        let config = Mp4MuxerConfig::default();
        let (_, ats) = resolve_timescales(&config);
        let entry = build_opus_sample_entry(48_000, 2);
        let mut seg = new_seg_state();
        let data = Bytes::from(vec![0u8; 64]);

        accumulate_audio_sample(
            data.clone(),
            Some(&meta(Some(20_000), true)),
            AudioCodec::Opus,
            ats,
            &entry,
            &mut seg,
        );
        assert_eq!(seg.pending_samples.len(), 1);
        let s = &seg.pending_samples[0];
        assert!(matches!(s.track_kind, TrackKind::Audio));
        assert!(s.keyframe, "audio frames are always keyframes");
        assert_eq!(s.data_size, 64);
        assert_eq!(s.duration, us_to_ticks(20_000, ats.get()));
        assert!(s.sample_entry.is_some());
        assert!(seg.audio_sample_entry_sent);

        // No metadata → fall back to the codec's default frame duration; entry
        // is no longer re-sent.
        accumulate_audio_sample(data, None, AudioCodec::Aac, ats, &entry, &mut seg);
        assert!(seg.pending_samples[1].sample_entry.is_none());
        assert_eq!(
            seg.pending_samples[1].duration,
            us_to_ticks(AudioCodec::Aac.default_frame_duration_us(), ats.get())
        );
    }

    #[test]
    fn should_flush_after_init_is_unconditional() {
        let mut seg = new_seg_state();
        seg.init_sent = true;
        let inputs = new_inputs(true, true, false);
        assert!(should_flush_fmp4_segment(&seg, &inputs, 0));
    }

    #[test]
    fn should_flush_when_all_present_tracks_ready() {
        // Both tracks present and both sample entries registered → flush.
        let mut seg = new_seg_state();
        seg.video_sample_entry_sent = true;
        seg.audio_sample_entry_sent = true;
        let inputs = new_inputs(true, true, false);
        assert!(should_flush_fmp4_segment(&seg, &inputs, 0));

        // Audio-only stream (video absent): the missing video track counts as
        // ready, so the first audio entry is enough.
        let mut seg = new_seg_state();
        seg.audio_sample_entry_sent = true;
        let inputs = new_inputs(true, false, false);
        assert!(should_flush_fmp4_segment(&seg, &inputs, 0));
    }

    #[test]
    fn should_not_flush_while_a_track_is_still_pending() {
        // Audio ready, but video present without its entry and not closed.
        let mut seg = new_seg_state();
        seg.audio_sample_entry_sent = true;
        seg.pending_samples = n_video_samples(5);
        let inputs = new_inputs(true, true, false);
        assert!(!should_flush_fmp4_segment(&seg, &inputs, 0));
    }

    #[test]
    fn should_force_flush_past_defer_cap_when_not_skip_classifying() {
        let mut seg = new_seg_state();
        seg.audio_sample_entry_sent = true; // video still not ready
        seg.pending_samples = n_video_samples(FMP4_FIRST_FLUSH_DEFER_CAP);
        let inputs = new_inputs(true, true, false);
        assert!(should_flush_fmp4_segment(&seg, &inputs, 0));
    }

    #[test]
    fn skip_classification_defers_until_hard_cap() {
        // Skip-classification mode with open inputs defers at the soft cap...
        let mut seg = new_seg_state();
        seg.pending_samples = n_video_samples(FMP4_FIRST_FLUSH_DEFER_CAP);
        let inputs = new_inputs(true, true, true);
        assert!(!should_flush_fmp4_segment(&seg, &inputs, 2));

        // ...but force-flushes once the hard cap is reached.
        seg.pending_samples = n_video_samples(FMP4_SKIP_CLASS_HARD_CAP);
        assert!(should_flush_fmp4_segment(&seg, &inputs, 2));
    }

    #[test]
    fn partition_samples_empty_input() {
        let (s, p) = partition_samples_by_track(Vec::new(), Vec::new());
        assert!(s.is_empty());
        assert!(p.is_empty());
    }

    #[test]
    fn partition_samples_single_track_recomputes_offsets() {
        let mut samples = n_video_samples(2);
        samples[0].data_size = 10;
        samples[1].data_size = 20;
        // Arrival offsets are deliberately wrong; partition must recompute them.
        samples[0].data_offset = 999;
        samples[1].data_offset = 999;
        let payloads = vec![Bytes::from(vec![1u8; 10]), Bytes::from(vec![2u8; 20])];

        let (s, p) = partition_samples_by_track(samples, payloads);
        assert_eq!(s.len(), 2);
        assert_eq!(p.len(), 2);
        assert_eq!(s[0].data_offset, 0);
        assert_eq!(s[1].data_offset, 10);
        assert!(s.iter().all(|x| matches!(x.track_kind, TrackKind::Video)));
    }

    #[test]
    fn us_to_ticks_boundaries() {
        assert_eq!(us_to_ticks(0, 90_000), 0);
        // Exactly one second of ticks.
        assert_eq!(us_to_ticks(1_000_000, 90_000), 90_000);
        // A zero timescale yields zero ticks (no panic).
        assert_eq!(us_to_ticks(1_000_000, 0), 0);
        // Pathologically large durations saturate at u32::MAX rather than wrap.
        assert_eq!(us_to_ticks(u64::MAX, 90_000), u32::MAX);
    }

    fn vtype(codec: VideoCodec) -> PacketType {
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        })
    }

    fn atype(codec: AudioCodec) -> PacketType {
        PacketType::EncodedAudio(EncodedAudioFormat { codec, codec_private: None })
    }

    fn av_binary(
        data: Vec<u8>,
        ct: Option<&'static str>,
        keyframe: bool,
        duration_us: u64,
    ) -> Packet {
        Packet::Binary {
            data: Bytes::from(data),
            content_type: ct.map(std::borrow::Cow::Borrowed),
            metadata: Some(meta(Some(duration_us), keyframe)),
        }
    }

    fn first_binary_bytes(packets: &[Packet]) -> Bytes {
        match packets.first().expect("expected at least one output packet") {
            Packet::Binary { data, .. } => data.clone(),
            other => panic!("expected a Binary packet, got {other:?}"),
        }
    }

    fn concat_binary(packets: &[Packet]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in packets {
            if let Packet::Binary { data, .. } = p {
                out.extend_from_slice(data);
            }
        }
        out
    }

    type MuxerHandle = tokio::task::JoinHandle<Result<(), StreamKitError>>;

    fn spawn_muxer(
        config: Mp4MuxerConfig,
        inputs: HashMap<String, mpsc::Receiver<Packet>>,
        input_types: &[(&str, PacketType)],
    ) -> (MockOutputSender, MuxerHandle) {
        let (mut ctx, mock, _state_rx) = create_test_context(inputs, 1);
        for (pin, ty) in input_types {
            ctx.input_types.insert((*pin).to_string(), ty.clone());
        }
        let node = Box::new(Mp4MuxerNode::new(config));
        let handle = tokio::spawn(async move { node.run(ctx).await });
        (mock, handle)
    }

    #[tokio::test]
    async fn stream_mode_video_only_h264_end_to_end() {
        let (tx, rx) = mpsc::channel(64);
        let inputs = HashMap::from([("in".to_string(), rx)]);
        let config = Mp4MuxerConfig {
            mode: Mp4StreamingMode::Stream,
            video_width: 320,
            video_height: 240,
            video_codec: Some(VideoCodec::H264),
            ..default()
        };
        let (mock, handle) = spawn_muxer(config, inputs, &[("in", vtype(VideoCodec::H264))]);

        // A leading non-keyframe is dropped by the keyframe gate; the first
        // keyframe opens it and the P-frames that follow are muxed.
        tx.send(av_binary(h264_pframe_annexb(9), None, false, 33_333)).await.unwrap();
        tx.send(av_binary(h264_keyframe_annexb(), None, true, 33_333)).await.unwrap();
        for i in 0..5u8 {
            tx.send(av_binary(h264_pframe_annexb(i), None, false, 33_333)).await.unwrap();
        }
        drop(tx);
        handle.await.unwrap().unwrap();

        let out = mock.get_packets_for_pin("out").await;
        let first = first_binary_bytes(&out);
        assert!(first.len() > 8);
        assert_eq!(&first[4..8], b"ftyp", "first fMP4 packet starts with the init segment");
    }

    #[tokio::test]
    async fn stream_mode_skip_classification_multi_segment() {
        // Two inputs + video dimensions trigger skip-classification (unified
        // receive loop, content_type-driven classification).
        let (vtx, vrx) = mpsc::channel(256);
        let (atx, arx) = mpsc::channel(256);
        let inputs = HashMap::from([("in".to_string(), vrx), ("in_1".to_string(), arx)]);
        let config = Mp4MuxerConfig {
            mode: Mp4StreamingMode::Stream,
            video_width: 320,
            video_height: 240,
            num_inputs: 2,
            video_codec: Some(VideoCodec::H264),
            audio_codec: Some(AudioCodec::Aac),
            ..default()
        };
        let (mock, handle) = spawn_muxer(config, inputs, &[]);

        // Interleave enough samples to cross the 30-sample flush threshold
        // several times, exercising both the init and subsequent-segment paths.
        vtx.send(av_binary(h264_keyframe_annexb(), Some("video/h264"), true, 33_333))
            .await
            .unwrap();
        for i in 0..40u8 {
            vtx.send(av_binary(h264_pframe_annexb(i), Some("video/h264"), false, 33_333))
                .await
                .unwrap();
            atx.send(av_binary(vec![0xAA; 64], Some("audio/aac"), true, 21_333)).await.unwrap();
        }
        drop(vtx);
        drop(atx);
        handle.await.unwrap().unwrap();

        let out = mock.get_packets_for_pin("out").await;
        assert!(out.len() >= 2, "multiple segments expected, got {}", out.len());
        let first = first_binary_bytes(&out);
        assert_eq!(&first[4..8], b"ftyp");
    }

    #[tokio::test]
    async fn stream_mode_video_only_no_keyframe_produces_no_output() {
        let (tx, rx) = mpsc::channel(64);
        let inputs = HashMap::from([("in".to_string(), rx)]);
        let config = Mp4MuxerConfig {
            mode: Mp4StreamingMode::Stream,
            video_width: 320,
            video_height: 240,
            video_codec: Some(VideoCodec::H264),
            ..default()
        };
        let (mock, handle) = spawn_muxer(config, inputs, &[("in", vtype(VideoCodec::H264))]);

        // Never a keyframe — every frame is gated out, nothing is muxed.
        for i in 0..5u8 {
            tx.send(av_binary(h264_pframe_annexb(i), None, false, 33_333)).await.unwrap();
        }
        drop(tx);
        handle.await.unwrap().unwrap();

        assert!(mock.get_packets_for_pin("out").await.is_empty());
    }

    #[tokio::test]
    async fn dual_track_classified_av1_audio_end_to_end() {
        // num_inputs == 2 with zero video dimensions keeps classification on
        // (per-pin type resolution), exercising the dual-track receive loop.
        let (vtx, vrx) = mpsc::channel(64);
        let (atx, arx) = mpsc::channel(64);
        let inputs = HashMap::from([("in".to_string(), vrx), ("in_1".to_string(), arx)]);
        let config = Mp4MuxerConfig { mode: Mp4StreamingMode::Stream, num_inputs: 2, ..default() };
        let (mock, handle) = spawn_muxer(
            config,
            inputs,
            &[("in", vtype(VideoCodec::Av1)), ("in_1", atype(AudioCodec::Opus))],
        );

        for i in 0..3u8 {
            vtx.send(av_binary(vec![0x10 + i; 256], None, true, 33_333)).await.unwrap();
            atx.send(av_binary(vec![0x80 + i; 64], None, true, 20_000)).await.unwrap();
        }
        drop(vtx);
        drop(atx);
        handle.await.unwrap().unwrap();

        let out = mock.get_packets_for_pin("out").await;
        let first = first_binary_bytes(&out);
        assert_eq!(&first[4..8], b"ftyp");
    }

    #[tokio::test]
    async fn file_mode_dual_track_round_trip_end_to_end() {
        let (vtx, vrx) = mpsc::channel(64);
        let (atx, arx) = mpsc::channel(64);
        let inputs = HashMap::from([("in".to_string(), vrx), ("in_1".to_string(), arx)]);
        let config = Mp4MuxerConfig {
            mode: Mp4StreamingMode::File,
            num_inputs: 2,
            video_width: 320,
            video_height: 240,
            video_codec: Some(VideoCodec::H264),
            audio_codec: Some(AudioCodec::Aac),
            ..default()
        };
        let (mock, handle) = spawn_muxer(
            config,
            inputs,
            &[("in", vtype(VideoCodec::H264)), ("in_1", atype(AudioCodec::Aac))],
        );

        vtx.send(av_binary(h264_keyframe_annexb(), None, true, 33_333)).await.unwrap();
        for i in 0..4u8 {
            vtx.send(av_binary(h264_pframe_annexb(i), None, false, 33_333)).await.unwrap();
            atx.send(av_binary(vec![0xBB; 64], None, true, 21_333)).await.unwrap();
        }
        drop(vtx);
        drop(atx);
        handle.await.unwrap().unwrap();

        let out = mock.get_packets_for_pin("out").await;
        let bytes = concat_binary(&out);
        assert!(bytes.len() > 8, "file-mode output should be a complete MP4");
        assert_eq!(&bytes[4..8], b"ftyp", "regular MP4 starts with the ftyp box");
    }

    #[tokio::test]
    async fn run_errors_without_any_input() {
        let (_mock, handle) = spawn_muxer(Mp4MuxerConfig::default(), HashMap::new(), &[]);
        assert!(handle.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn run_errors_on_multiple_same_kind_inputs() {
        // Two inputs both resolved to video (dims zero ⇒ classification stays on)
        // is a misconfiguration the node rejects.
        let (_vtx0, vrx0) = mpsc::channel::<Packet>(4);
        let (_vtx1, vrx1) = mpsc::channel::<Packet>(4);
        let inputs = HashMap::from([("in".to_string(), vrx0), ("in_1".to_string(), vrx1)]);
        let config = Mp4MuxerConfig { num_inputs: 2, ..default() };
        let (_mock, handle) = spawn_muxer(
            config,
            inputs,
            &[("in", vtype(VideoCodec::H264)), ("in_1", vtype(VideoCodec::H264))],
        );
        assert!(handle.await.unwrap().is_err());
    }
}
