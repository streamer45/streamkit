// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Core data types that flow through StreamKit pipelines.
//!
//! This module defines the fundamental data structures used throughout the system:
//! - [`Packet`]: Generic container for any type of data (audio, text, transcription, etc.)
//! - [`AudioFrame`]: Raw audio data with zero-copy Arc-based semantics
//! - [`PacketType`]: Type system for pre-flight pipeline validation
//! - [`AudioFormat`]: Audio stream format descriptors
//! - Transcription types for speech processing
//! - Extensible custom packet types for plugins

use crate::error::StreamKitError;
use crate::frame_pool::{PooledSamples, PooledVideoData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use std::sync::Arc;
use ts_rs::TS;

/// Describes the specific format of raw audio data.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum SampleFormat {
    F32,   // 32-bit floating point
    S16Le, // 16-bit signed integer, little-endian
}

/// Contains the detailed metadata for a raw audio stream.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

/// Describes the pixel format of raw video frames.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum PixelFormat {
    Rgba8,
    I420,
    Nv12,
}

/// Contains the detailed metadata for a raw video stream.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct RawVideoFormat {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_format: PixelFormat,
}

/// Supported encoded audio codecs.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum AudioCodec {
    Opus,
}

/// Supported encoded video codecs.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum VideoCodec {
    Vp9,
    /// Forward-looking placeholder — not yet wired into any encoder/decoder.
    H264,
    /// Forward-looking placeholder — not yet wired into any encoder/decoder.
    Av1,
}

/// Bitstream format hints for video codecs (primarily H264).
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum VideoBitstreamFormat {
    AnnexB,
    Avcc,
}

/// Encoded audio format details (extensible for codec-specific config).
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct EncodedAudioFormat {
    pub codec: AudioCodec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_private: Option<Vec<u8>>,
}

/// Encoded video format details (extensible for codec-specific config).
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

/// Optional timing and sequencing metadata that can be attached to packets.
/// Used for pacing, synchronization, and A/V alignment. See `timing` module for
/// canonical semantics (media-time epoch, monotonicity, and preservation rules).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct PacketMetadata {
    /// Absolute timestamp in microseconds (presentation time)
    pub timestamp_us: Option<u64>,
    /// Duration of this packet/frame in microseconds
    pub duration_us: Option<u64>,
    /// Sequence number for ordering and detecting loss
    pub sequence: Option<u64>,
    /// Keyframe flag for encoded video packets (and raw frames if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyframe: Option<bool>,
}

/// Describes the *type* of data, used for pre-flight pipeline validation.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub enum PacketType {
    /// Raw, uncompressed audio with a specific format.
    RawAudio(AudioFormat),
    /// Raw, uncompressed video with a specific format.
    RawVideo(RawVideoFormat),
    /// Encoded audio with codec metadata.
    EncodedAudio(EncodedAudioFormat),
    /// Encoded video with codec metadata.
    EncodedVideo(EncodedVideoFormat),
    /// Plain text.
    Text,
    /// Structured transcription data with timestamps and metadata.
    Transcription,
    /// Extensible structured packet type (typically produced/consumed by plugins).
    ///
    /// `type_id` should be namespaced and versioned (e.g., `plugin::native::vad/vad-event@1`).
    Custom { type_id: String },
    /// Generic binary data.
    Binary,
    /// A special type for nodes that can accept any format.
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

/// A generic container for any type of data that can flow through a pipeline.
#[derive(Debug, Clone, Serialize)]
pub enum Packet {
    Audio(AudioFrame),
    Video(VideoFrame),
    /// Text payload (Arc-backed to make fan-out cloning cheap).
    Text(Arc<str>),
    /// Transcription payload (Arc-backed to make fan-out cloning cheap).
    Transcription(Arc<TranscriptionData>),
    /// Extensible structured payload (Arc-backed to make fan-out cloning cheap).
    Custom(Arc<CustomPacketData>),
    /// Binary data with optional content-type and timing metadata for proper handling
    /// of different binary formats (e.g., "audio/ogg", "application/octet-stream").
    ///
    /// The `content_type` uses `Cow<'static, str>` to avoid heap allocations when using
    /// static string literals (e.g., `Cow::Borrowed("audio/ogg")`), while still supporting
    /// dynamic content types when needed.
    Binary {
        #[serde(serialize_with = "serialize_bytes")]
        data: bytes::Bytes,
        content_type: Option<Cow<'static, str>>,
        metadata: Option<PacketMetadata>,
    },
}

/// Encoding for [`Packet::Custom`] payloads.
///
/// This is intentionally extensible. For now we keep things user-friendly and debuggable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CustomEncoding {
    /// UTF-8 JSON value (object/array/string/number/bool/null).
    Json,
}

/// Extensible structured packet data.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct CustomPacketData {
    /// Namespaced, versioned type id (e.g., `plugin::native::vad/vad-event@1`).
    pub type_id: String,
    pub encoding: CustomEncoding,
    pub data: JsonValue,
    /// Optional timing/ordering metadata.
    pub metadata: Option<PacketMetadata>,
}

/// Custom serializer for bytes::Bytes to base64 string
fn serialize_bytes<S>(bytes: &bytes::Bytes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    // Serialize as base64 for JSON compatibility
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes.as_ref())
        .serialize(serializer)
}

/// A segment of transcribed text with timing information.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct TranscriptionSegment {
    /// The transcribed text for this segment
    pub text: String,
    /// Start time in milliseconds
    pub start_time_ms: u64,
    /// End time in milliseconds
    pub end_time_ms: u64,
    /// Confidence score (0.0 - 1.0), if available
    pub confidence: Option<f32>,
}

/// Structured transcription data with timing and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct TranscriptionData {
    /// The full transcribed text (concatenation of all segments)
    pub text: String,
    /// Individual segments with timing information
    pub segments: Vec<TranscriptionSegment>,
    /// Detected or specified language code (e.g., "en", "es", "fr")
    pub language: Option<String>,
    /// Optional timing metadata for the entire transcription
    pub metadata: Option<PacketMetadata>,
}

/// Layout information for a single video plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoPlane {
    pub offset: usize,
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

/// Packed layout for a video frame.
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

fn align_up(value: usize, align: usize) -> usize {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

/// A view into a single video plane.
pub struct VideoPlaneRef<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

/// A mutable view into a single video plane.
pub struct VideoPlaneMut<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

/// A single frame or packet of raw audio data, using f32 as the internal standard.
///
/// Audio samples are stored in an `Arc<PooledSamples>` for efficient zero-copy cloning when packets
/// are distributed to multiple outputs (fan-out). This makes packet cloning extremely cheap
/// (just an atomic refcount increment) while still allowing mutation when needed via
/// copy-on-write semantics.
///
/// # Immutability by Default
/// AudioFrame follows an immutable-by-default design:
/// - Cloning is cheap (O(1) - just increments Arc refcount)
/// - Nodes can pass packets through without copying
/// - Nodes that need to modify samples use `make_samples_mut()` for copy-on-write
///
/// # Example: Pass-through (zero-copy)
/// ```ignore
/// // Packet flows through pacer without any memory allocation
/// output_sender.send("out", packet).await?;
/// ```
///
/// # Example: Mutation (copy-on-write)
/// ```ignore
/// // Gain filter - copies only if Arc is shared, mutates in place if unique
/// if let Packet::Audio(mut frame) = packet {
///     for sample in frame.make_samples_mut() {
///         *sample *= gain;
///     }
///     output_sender.send("out", Packet::Audio(frame)).await?;
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    /// The raw audio data, with samples interleaved for multi-channel audio
    /// (e.g., [L, R, L, R, ...]). Stored in an Arc for efficient cloning.
    #[serde(serialize_with = "serialize_arc_pooled_samples")]
    pub samples: Arc<PooledSamples>,
    /// Optional timing metadata for pacing and synchronization
    pub metadata: Option<PacketMetadata>,
}

/// Custom serializer for Arc<PooledVideoData> - serializes as base64
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

/// Custom serializer for Arc<PooledSamples> - serializes as a slice
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
    /// Create a new AudioFrame from pooled storage (preferred for hot paths).
    pub fn from_pooled(
        sample_rate: u32,
        channels: u16,
        samples: PooledSamples,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self { sample_rate, channels, samples: Arc::new(samples), metadata }
    }

    /// Create a new AudioFrame from a Vec of samples.
    ///
    /// This is the preferred way to construct AudioFrame as it clearly wraps
    /// the samples in an Arc.
    ///
    /// # Example
    /// ```rust
    /// use streamkit_core::types::AudioFrame;
    /// let samples = vec![0.5, -0.5, 0.3, -0.3]; // Stereo: L, R, L, R
    /// let frame = AudioFrame::new(48000, 2, samples);
    /// assert_eq!(frame.sample_rate, 48000);
    /// assert_eq!(frame.channels, 2);
    /// ```
    pub fn new(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        Self::from_pooled(sample_rate, channels, PooledSamples::from_vec(samples), None)
    }

    /// Create a new AudioFrame with metadata.
    ///
    /// # Example
    /// ```rust
    /// use streamkit_core::types::{AudioFrame, PacketMetadata};
    /// let metadata = PacketMetadata {
    ///     timestamp_us: Some(1000),
    ///     duration_us: Some(20_000),
    ///     sequence: Some(42),
    ///     keyframe: None,
    /// };
    /// let frame = AudioFrame::with_metadata(48000, 2, vec![0.5, -0.5], Some(metadata));
    /// assert_eq!(frame.metadata.unwrap().sequence, Some(42));
    /// ```
    pub fn with_metadata(
        sample_rate: u32,
        channels: u16,
        samples: Vec<f32>,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self::from_pooled(sample_rate, channels, PooledSamples::from_vec(samples), metadata)
    }

    /// Create an AudioFrame from an already-Arc'd pooled buffer.
    ///
    /// This is useful when you already have samples in an Arc and want to avoid
    /// any allocation or copying.
    pub const fn from_arc(
        sample_rate: u32,
        channels: u16,
        samples: Arc<PooledSamples>,
        metadata: Option<PacketMetadata>,
    ) -> Self {
        Self { sample_rate, channels, samples, metadata }
    }

    /// Get immutable access to samples as a slice (zero cost).
    ///
    /// # Example
    /// ```rust
    /// use streamkit_core::types::AudioFrame;
    /// let frame = AudioFrame::new(48000, 2, vec![0.5, -0.3, 0.7, -0.1]);
    /// let peak = frame.samples().iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    /// assert!((peak - 0.7).abs() < 0.001);
    /// ```
    pub fn samples(&self) -> &[f32] {
        self.samples.as_slice()
    }

    /// Get mutable access to samples, cloning only if Arc is shared.
    ///
    /// This implements copy-on-write semantics:
    /// - If this is the only reference: mutates in place (zero cost)
    /// - If shared with other clones: clones the data first (one copy)
    ///
    /// # Example
    /// ```rust
    /// use streamkit_core::types::AudioFrame;
    /// let mut frame = AudioFrame::new(48000, 2, vec![0.5, -0.5, 0.25, -0.25]);
    /// let gain = 2.0;
    /// for sample in frame.make_samples_mut() {
    ///     *sample *= gain;
    /// }
    /// assert_eq!(frame.samples(), &[1.0, -1.0, 0.5, -0.5]);
    /// ```
    pub fn make_samples_mut(&mut self) -> &mut [f32] {
        Arc::make_mut(&mut self.samples).as_mut_slice()
    }

    /// Check if we have exclusive ownership of the samples.
    ///
    /// Returns `true` if this is the only Arc reference to the samples,
    /// meaning `make_samples_mut()` will mutate in place without copying.
    ///
    /// This is primarily useful for optimization hints and debugging.
    ///
    /// # Example
    /// ```rust
    /// use streamkit_core::types::AudioFrame;
    /// let frame1 = AudioFrame::new(48000, 1, vec![0.5, 0.3]);
    /// assert!(frame1.has_unique_samples()); // Only one owner
    ///
    /// let frame2 = frame1.clone();
    /// assert!(!frame1.has_unique_samples()); // Now shared
    /// assert!(!frame2.has_unique_samples()); // Both share the Arc
    /// ```
    pub fn has_unique_samples(&self) -> bool {
        Arc::strong_count(&self.samples) == 1
    }

    /// Get the number of samples (total across all channels).
    ///
    /// For stereo audio, this is twice the number of sample frames.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the frame is empty (no samples).
    #[allow(clippy::len_without_is_empty)] // is_empty provided explicitly
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Get the number of sample frames (samples / channels).
    ///
    /// For 960 stereo samples, this returns 480 frames.
    pub fn num_frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// Calculate the duration of this audio frame in microseconds.
    ///
    /// Returns `None` if sample_rate is 0 (invalid).
    pub fn duration_us(&self) -> Option<u64> {
        if self.sample_rate == 0 {
            return None;
        }
        let frames = self.num_frames() as u64;
        Some((frames * 1_000_000) / u64::from(self.sample_rate))
    }
}

/// A single frame of raw video data, stored in an Arc for zero-copy fan-out.
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
    /// Validate that `layout` is consistent with the given dimensions/format
    /// and that `data_len` is large enough.
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
        Ok(Self { width, height, pixel_format, layout, data, metadata })
    }

    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn make_data_mut(&mut self) -> &mut [u8] {
        Arc::make_mut(&mut self.data).as_mut_slice()
    }

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
}
