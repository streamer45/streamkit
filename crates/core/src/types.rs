// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Core data types that flow through StreamKit pipelines.

use crate::error::StreamKitError;
use crate::frame_pool::{PooledSamples, PooledVideoData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::sync::Arc;
use ts_rs::TS;

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum SampleFormat {
    F32,
    S16Le,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[non_exhaustive]
pub enum PixelFormat {
    Rgba8,
    I420,
    Nv12,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct RawVideoFormat {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_format: PixelFormat,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
#[non_exhaustive]
pub enum AudioCodec {
    #[serde(alias = "Opus")]
    Opus,
    #[serde(alias = "Aac", alias = "AAC")]
    Aac,
}

impl AudioCodec {
    /// Default frame duration in microseconds for this codec.
    ///
    /// Used as a fallback when audio packets lack `duration_us` metadata.
    /// - Opus: 20 ms (960 samples at 48 kHz)
    /// - AAC-LC: ~21.333 ms (1024 samples at 48 kHz)
    pub const fn default_frame_duration_us(self) -> u64 {
        match self {
            Self::Opus => 20_000,
            Self::Aac => 21_333,
        }
    }

    /// Canonical lowercase name used in the C ABI (`custom_type_id`).
    ///
    /// Adding a new `AudioCodec` variant?  Add its name here and in
    /// [`Self::from_c_name`] — that's the **only** place codec-name
    /// strings need to live.
    pub const fn as_c_name(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Aac => "aac",
        }
    }

    /// Parse a C ABI codec name back to an `AudioCodec`.
    ///
    /// Accepts the canonical lowercase names produced by [`Self::as_c_name`].
    pub fn from_c_name(name: &str) -> Result<Self, String> {
        match name {
            "opus" => Ok(Self::Opus),
            "aac" => Ok(Self::Aac),
            other => Err(format!("unknown audio codec name: {other:?}")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
#[non_exhaustive]
pub enum VideoCodec {
    #[serde(alias = "Vp9", alias = "VP9")]
    Vp9,
    /// OpenH264 Constrained Baseline encoder/decoder.
    #[serde(alias = "avc", alias = "avc1", alias = "H264")]
    H264,
    /// CPU AV1 codec support via rav1e (encoder) and rav1d (decoder).
    #[serde(alias = "Av1", alias = "AV1")]
    Av1,
}

impl VideoCodec {
    /// Canonical lowercase name used in the C ABI (`custom_type_id`).
    ///
    /// Adding a new `VideoCodec` variant?  Add its name here and in
    /// [`Self::from_c_name`] — that's the **only** place codec-name
    /// strings need to live.
    pub const fn as_c_name(self) -> &'static str {
        match self {
            Self::Vp9 => "vp9",
            Self::H264 => "h264",
            Self::Av1 => "av1",
        }
    }

    /// Parse a C ABI codec name back to a `VideoCodec`.
    ///
    /// Accepts the canonical lowercase names produced by [`Self::as_c_name`].
    pub fn from_c_name(name: &str) -> Result<Self, String> {
        match name {
            "vp9" => Ok(Self::Vp9),
            "h264" => Ok(Self::H264),
            "av1" => Ok(Self::Av1),
            other => Err(format!("unknown video codec name: {other:?}")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum VideoBitstreamFormat {
    AnnexB,
    Avcc,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct EncodedAudioFormat {
    pub codec: AudioCodec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_private: Option<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct EncodedVideoFormat {
    pub codec: VideoCodec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitstream_format: Option<VideoBitstreamFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_private: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

/// See `timing` module for canonical semantics.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct PacketMetadata {
    pub timestamp_us: Option<u64>,
    pub duration_us: Option<u64>,
    pub sequence: Option<u64>,
    pub keyframe: Option<bool>,
}

/// Used for pre-flight pipeline validation.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum PacketType {
    RawAudio(AudioFormat),
    RawVideo(RawVideoFormat),
    EncodedAudio(EncodedAudioFormat),
    EncodedVideo(EncodedVideoFormat),
    Text,
    Transcription,
    /// `type_id` should be namespaced and versioned (e.g., `plugin::native::vad/vad-event@1`).
    Custom {
        type_id: String,
    },
    Binary,
    Any,
    /// A type that passes through the input type unchanged (for type inference).
    ///
    /// Used by passthrough nodes like pacer, script, and passthrough, where output type = input type.
    ///
    /// **Validation Behavior:**
    /// - **OneShot (static) pipelines:** Passthrough types are resolved at compile-time during
    ///   pipeline compilation. The graph builder traces connections and resolves each Passthrough
    ///   output to the concrete type of its input. This allows full pre-flight type checking.
    /// - **Dynamic pipelines:** Passthrough types are validated at runtime during connection.
    ///   When a connection involves Passthrough, the connection is allowed and the type will be
    ///   resolved when actual packets flow through the node.
    ///
    /// **Example:** A pacer node with `Passthrough` output connected to a raw audio input will:
    /// - In oneshot mode: Be resolved to `RawAudio` during compilation
    /// - In dynamic mode: Accept the connection and adapt at runtime to whatever audio format it receives
    Passthrough,
}

#[derive(Debug, Clone, Serialize)]
pub enum Packet {
    Audio(AudioFrame),
    Video(VideoFrame),
    Text(Arc<str>),
    Transcription(Arc<TranscriptionData>),
    Custom(Arc<CustomPacketData>),
    Binary {
        #[serde(serialize_with = "serialize_bytes")]
        data: bytes::Bytes,
        content_type: Option<Cow<'static, str>>,
        metadata: Option<PacketMetadata>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CustomEncoding {
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct CustomPacketData {
    pub type_id: String,
    pub encoding: CustomEncoding,
    pub data: JsonValue,
    pub metadata: Option<PacketMetadata>,
}

fn serialize_bytes<S>(bytes: &bytes::Bytes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes.as_ref())
        .serialize(serializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct TranscriptionData {
    pub text: String,
    pub segments: Vec<TranscriptionSegment>,
    pub language: Option<String>,
    pub metadata: Option<PacketMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoPlane {
    pub offset: usize,
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoLayout {
    plane_count: usize,
    planes: [VideoPlane; 3],
    total_bytes: usize,
    stride_align: u32,
}

impl VideoLayout {
    pub fn packed(width: u32, height: u32, pixel_format: PixelFormat) -> Self {
        Self::aligned(width, height, pixel_format, 1)
    }

    /// Compute the layout for the given dimensions, pixel format, and stride alignment.
    ///
    /// Zero-width or zero-height dimensions are allowed and produce a zero-byte layout.
    /// This can be useful as a sentinel but callers should generally validate dimensions
    /// before constructing frames.
    pub fn aligned(width: u32, height: u32, pixel_format: PixelFormat, stride_align: u32) -> Self {
        const EMPTY_PLANE: VideoPlane = VideoPlane { offset: 0, stride: 0, width: 0, height: 0 };
        let mut planes = [EMPTY_PLANE; 3];
        let stride_align = stride_align.max(1);
        let stride_align_usize = stride_align as usize;

        let (plane_count, total_bytes) = match pixel_format {
            PixelFormat::Rgba8 => {
                let stride = align_up(width as usize * 4, stride_align_usize);
                let size = stride * height as usize;
                planes[0] = VideoPlane { offset: 0, stride, width, height };
                (1, size)
            },
            PixelFormat::I420 => {
                let luma_stride = align_up(width as usize, stride_align_usize);
                let luma_size = luma_stride * height as usize;
                let chroma_width = (width + 1) as usize / 2;
                let chroma_height = (height + 1) as usize / 2;
                let chroma_stride = align_up(chroma_width, stride_align_usize);
                let chroma_size = chroma_stride * chroma_height;

                planes[0] = VideoPlane { offset: 0, stride: luma_stride, width, height };
                planes[1] = VideoPlane {
                    offset: luma_size,
                    stride: chroma_stride,
                    width: chroma_width as u32,
                    height: chroma_height as u32,
                };
                planes[2] = VideoPlane {
                    offset: luma_size + chroma_size,
                    stride: chroma_stride,
                    width: chroma_width as u32,
                    height: chroma_height as u32,
                };

                (3, luma_size + chroma_size * 2)
            },
            PixelFormat::Nv12 => {
                let luma_stride = align_up(width as usize, stride_align_usize);
                let luma_size = luma_stride * height as usize;
                let chroma_width = (width + 1) as usize / 2 * 2; // interleaved UV pairs
                let chroma_height = (height + 1) as usize / 2;
                let chroma_stride = align_up(chroma_width, stride_align_usize);
                let chroma_size = chroma_stride * chroma_height;

                planes[0] = VideoPlane { offset: 0, stride: luma_stride, width, height };
                planes[1] = VideoPlane {
                    offset: luma_size,
                    stride: chroma_stride,
                    width: chroma_width as u32,
                    height: chroma_height as u32,
                };

                (2, luma_size + chroma_size)
            },
        };

        Self { plane_count, planes, total_bytes, stride_align }
    }

    pub const fn plane_count(&self) -> usize {
        self.plane_count
    }

    pub fn planes(&self) -> &[VideoPlane] {
        &self.planes[..self.plane_count]
    }

    pub fn plane(&self, index: usize) -> Option<VideoPlane> {
        if index < self.plane_count {
            Some(self.planes[index])
        } else {
            None
        }
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub const fn stride_align(&self) -> u32 {
        self.stride_align
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

pub struct VideoPlaneRef<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

pub struct VideoPlaneMut<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

/// Arc-backed audio frame with copy-on-write semantics via `make_samples_mut()`.
#[derive(Debug, Clone, Serialize)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    #[serde(serialize_with = "serialize_arc_pooled_samples")]
    pub samples: Arc<PooledSamples>,
    pub metadata: Option<PacketMetadata>,
}

fn serialize_arc_pooled_video_bytes<S>(
    arc: &Arc<PooledVideoData>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, arc.as_slice())
        .serialize(serializer)
}

fn serialize_arc_pooled_samples<S>(
    arc: &Arc<PooledSamples>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    arc.as_slice().serialize(serializer)
}

impl AudioFrame {
    pub fn from_pooled(
        sample_rate: u32,
        channels: u16,
        samples: PooledSamples,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self { sample_rate, channels, samples: Arc::new(samples), metadata }
    }

    pub fn new(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        Self::from_pooled(sample_rate, channels, PooledSamples::from_vec(samples), None)
    }

    pub fn with_metadata(
        sample_rate: u32,
        channels: u16,
        samples: Vec<f32>,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self::from_pooled(sample_rate, channels, PooledSamples::from_vec(samples), metadata)
    }

    pub const fn from_arc(
        sample_rate: u32,
        channels: u16,
        samples: Arc<PooledSamples>,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self { sample_rate, channels, samples, metadata }
    }

    pub fn samples(&self) -> &[f32] {
        self.samples.as_slice()
    }

    /// Copy-on-write: clones only if the Arc is shared.
    pub fn make_samples_mut(&mut self) -> &mut [f32] {
        Arc::make_mut(&mut self.samples).as_mut_slice()
    }

    pub fn has_unique_samples(&self) -> bool {
        Arc::strong_count(&self.samples) == 1
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[allow(clippy::len_without_is_empty)] // is_empty provided explicitly
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn num_frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    pub fn duration_us(&self) -> Option<u64> {
        if self.sample_rate == 0 {
            return None;
        }
        let frames = self.num_frames() as u64;
        Some((frames * 1_000_000) / u64::from(self.sample_rate))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub layout: VideoLayout,
    #[serde(serialize_with = "serialize_arc_pooled_video_bytes")]
    pub data: Arc<PooledVideoData>,
    pub metadata: Option<PacketMetadata>,
}

impl VideoFrame {
    fn validate_layout(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        layout: &VideoLayout,
        data_len: usize,
    ) -> Result<(), StreamKitError> {
        let expected_layout =
            VideoLayout::aligned(width, height, pixel_format, layout.stride_align());
        if *layout != expected_layout {
            return Err(StreamKitError::Runtime(format!(
                "VideoFrame layout mismatch: expected {expected_layout:?}, got {layout:?}"
            )));
        }
        if data_len < layout.total_bytes() {
            return Err(StreamKitError::Runtime(format!(
                "VideoFrame data buffer too small: need {} bytes, have {data_len}",
                layout.total_bytes(),
            )));
        }
        Ok(())
    }

    pub fn from_pooled(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        data: PooledVideoData,
        metadata: Option<PacketMetadata>,
    ) -> Result<Self, StreamKitError> {
        let layout = VideoLayout::packed(width, height, pixel_format);
        Self::from_pooled_with_layout(width, height, pixel_format, layout, data, metadata)
    }

    pub fn from_pooled_with_layout(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        layout: VideoLayout,
        mut data: PooledVideoData,
        metadata: Option<PacketMetadata>,
    ) -> Result<Self, StreamKitError> {
        Self::validate_layout(width, height, pixel_format, &layout, data.len())?;
        data.truncate(layout.total_bytes());
        Ok(Self { width, height, pixel_format, layout, data: Arc::new(data), metadata })
    }

    pub fn new(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        data: Vec<u8>,
    ) -> Result<Self, StreamKitError> {
        Self::from_pooled(width, height, pixel_format, PooledVideoData::from_vec(data), None)
    }

    pub fn with_metadata(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        data: Vec<u8>,
        metadata: Option<PacketMetadata>,
    ) -> Result<Self, StreamKitError> {
        Self::from_pooled(width, height, pixel_format, PooledVideoData::from_vec(data), metadata)
    }

    pub fn from_arc(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        data: Arc<PooledVideoData>,
        metadata: Option<PacketMetadata>,
    ) -> Result<Self, StreamKitError> {
        let layout = VideoLayout::packed(width, height, pixel_format);
        Self::from_arc_with_layout(width, height, pixel_format, layout, data, metadata)
    }

    pub fn from_arc_with_layout(
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        layout: VideoLayout,
        data: Arc<PooledVideoData>,
        metadata: Option<PacketMetadata>,
    ) -> Result<Self, StreamKitError> {
        Self::validate_layout(width, height, pixel_format, &layout, data.len())?;
        // Truncate to match the layout, consistent with `from_pooled_with_layout`.
        let data = if data.len() > layout.total_bytes() {
            let mut owned = Arc::unwrap_or_clone(data);
            owned.truncate(layout.total_bytes());
            Arc::new(owned)
        } else {
            data
        };
        Ok(Self { width, height, pixel_format, layout, data, metadata })
    }

    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn make_data_mut(&mut self) -> &mut [u8] {
        Arc::make_mut(&mut self.data).as_mut_slice()
    }

    /// Returns `true` when this frame holds the only strong reference to the
    /// underlying data buffer, meaning `make_data_mut()` will not trigger a
    /// copy.
    ///
    /// **Note:** This check is inherently racy — another thread could clone
    /// the `Arc` between the call to `has_unique_data()` and a subsequent
    /// mutation.  Use it as an advisory hint (e.g., for logging or metrics),
    /// not as a synchronisation primitive.
    pub fn has_unique_data(&self) -> bool {
        Arc::strong_count(&self.data) == 1
    }

    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn layout(&self) -> VideoLayout {
        self.layout
    }

    pub fn plane(&self, index: usize) -> Option<VideoPlaneRef<'_>> {
        let layout = self.layout();
        let plane = layout.plane(index)?;
        let start = plane.offset;
        let end = start + plane.stride * plane.height as usize;
        if end <= self.data.len() {
            Some(VideoPlaneRef {
                data: &self.data.as_slice()[start..end],
                stride: plane.stride,
                width: plane.width,
                height: plane.height,
            })
        } else {
            None
        }
    }

    pub fn plane_mut(&mut self, index: usize) -> Option<VideoPlaneMut<'_>> {
        let layout = self.layout();
        let plane = layout.plane(index)?;
        let start = plane.offset;
        let end = start + plane.stride * plane.height as usize;
        // Check bounds before triggering CoW to avoid a wasted copy.
        if end > self.data.len() {
            return None;
        }
        let data = Arc::make_mut(&mut self.data);
        Some(VideoPlaneMut {
            data: &mut data.as_mut_slice()[start..end],
            stride: plane.stride,
            width: plane.width,
            height: plane.height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_pool::FramePool;

    #[test]
    fn video_frame_copy_on_write() {
        let frame_a = VideoFrame::new(2, 1, PixelFormat::Rgba8, vec![0u8; 8]).unwrap();
        let mut frame_b = frame_a.clone();

        assert!(!frame_a.has_unique_data());
        assert!(!frame_b.has_unique_data());

        frame_b.make_data_mut()[0] = 7;

        assert_eq!(frame_a.data()[0], 0);
        assert_eq!(frame_b.data()[0], 7);
        assert!(frame_a.has_unique_data());
        assert!(frame_b.has_unique_data());
    }

    #[test]
    fn from_arc_and_pooled_with_layout_consistent_data_len() {
        let width = 2;
        let height = 1;
        let pf = PixelFormat::Rgba8;
        let layout = VideoLayout::packed(width, height, pf);
        let expected = layout.total_bytes(); // 8 bytes

        // Supply extra trailing bytes beyond what the layout requires.
        let oversized: Vec<u8> = vec![0xAA; expected + 16];

        let pooled_frame = VideoFrame::from_pooled_with_layout(
            width,
            height,
            pf,
            layout,
            PooledVideoData::from_vec(oversized.clone()),
            None,
        )
        .unwrap();

        let arc_frame = VideoFrame::from_arc_with_layout(
            width,
            height,
            pf,
            layout,
            Arc::new(PooledVideoData::from_vec(oversized)),
            None,
        )
        .unwrap();

        assert_eq!(
            pooled_frame.data_len(),
            expected,
            "from_pooled_with_layout should truncate to layout size"
        );
        assert_eq!(
            arc_frame.data_len(),
            expected,
            "from_arc_with_layout should truncate to layout size"
        );
        assert_eq!(
            pooled_frame.data_len(),
            arc_frame.data_len(),
            "both constructors must produce frames with the same data length"
        );
    }

    #[test]
    fn video_frame_pool_returns_on_drop() {
        let pool = FramePool::<u8>::preallocated(&[8], 1);
        assert_eq!(pool.stats().buckets[0].available, 1);

        {
            let data = pool.get(8);
            let frame = VideoFrame::from_pooled(2, 1, PixelFormat::Rgba8, data, None).unwrap();
            assert_eq!(frame.data_len(), 8);
            assert_eq!(pool.stats().buckets[0].available, 0);
            drop(frame);
        }

        assert_eq!(pool.stats().buckets[0].available, 1);
    }

    #[test]
    fn audio_codec_c_name_roundtrip() {
        for codec in [AudioCodec::Opus, AudioCodec::Aac] {
            let name = codec.as_c_name();
            let parsed = AudioCodec::from_c_name(name)
                .unwrap_or_else(|e| panic!("roundtrip failed for {codec:?}: {e}"));
            assert_eq!(codec, parsed, "roundtrip mismatch for {name:?}");
        }
    }

    #[test]
    fn video_codec_c_name_roundtrip() {
        for codec in [VideoCodec::Vp9, VideoCodec::H264, VideoCodec::Av1] {
            let name = codec.as_c_name();
            let parsed = VideoCodec::from_c_name(name)
                .unwrap_or_else(|e| panic!("roundtrip failed for {codec:?}: {e}"));
            assert_eq!(codec, parsed, "roundtrip mismatch for {name:?}");
        }
    }

    #[test]
    fn codec_from_c_name_is_strict_canonical_only() {
        // from_c_name only accepts canonical lowercase names from as_c_name().
        // Serde aliases ("avc", "avc1", "H264", etc.) are for config
        // deserialization only — the C ABI is a controlled interface.
        assert!(VideoCodec::from_c_name("avc").is_err());
        assert!(VideoCodec::from_c_name("avc1").is_err());
        assert!(VideoCodec::from_c_name("H264").is_err());
        assert!(AudioCodec::from_c_name("Opus").is_err());
        assert!(AudioCodec::from_c_name("AAC").is_err());
    }

    #[test]
    fn codec_from_c_name_unknown_errors() {
        assert!(AudioCodec::from_c_name("mp3").is_err());
        assert!(VideoCodec::from_c_name("hevc").is_err());
    }

    #[test]
    fn audio_codec_default_frame_durations() {
        assert_eq!(AudioCodec::Opus.default_frame_duration_us(), 20_000);
        assert_eq!(AudioCodec::Aac.default_frame_duration_us(), 21_333);
    }
}
