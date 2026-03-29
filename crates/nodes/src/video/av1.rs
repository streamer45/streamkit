// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! AV1 video codec nodes (CPU).
//!
//! Uses [rav1e](https://crates.io/crates/rav1e) for encoding and
//! [rav1d](https://crates.io/crates/rav1d) (a pure-Rust port of dav1d) for
//! decoding.  Both nodes follow the same `spawn_blocking` + `mpsc` channel
//! architecture as the VP9 nodes in [`super::vp9`].

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use rav1e::data::FrameParameters;
use rav1e::prelude::FrameTypeOverride;
use rav1e::prelude::{
    ChromaSamplePosition, ChromaSampling, Config, Context, EncoderConfig, EncoderStatus, FrameType,
    Rational, SpeedSettings,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    EncodedVideoFormat, Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat,
    VideoCodec, VideoFrame, VideoLayout, VideoPlane,
};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, PooledVideoData, ProcessorNode,
    StreamKitError, VideoFramePool,
};
use tokio::sync::mpsc;

const AV1_CONTENT_TYPE: &str = "video/av1";

const AV1_DEFAULT_BITRATE_KBPS: u32 = 2500;
const AV1_DEFAULT_KF_INTERVAL: u32 = 120;
const AV1_DEFAULT_THREADS: u32 = 2;
const AV1_DEFAULT_SPEED: u32 = 10;

// ---------------------------------------------------------------------------
// Configuration structs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct Av1DecoderConfig {
    pub threads: u32,
}

impl Default for Av1DecoderConfig {
    fn default() -> Self {
        Self { threads: AV1_DEFAULT_THREADS }
    }
}

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct Av1EncoderConfig {
    pub bitrate_kbps: u32,
    pub keyframe_interval: u32,
    pub threads: u32,
    /// rav1e speed preset (0 = slowest/best quality, 10 = fastest/real-time).
    pub speed: u32,
    /// Enable rav1e low-latency mode (disables frame reordering).
    pub low_latency: bool,
}

impl Default for Av1EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_kbps: AV1_DEFAULT_BITRATE_KBPS,
            keyframe_interval: AV1_DEFAULT_KF_INTERVAL,
            threads: AV1_DEFAULT_THREADS,
            speed: AV1_DEFAULT_SPEED,
            low_latency: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder node
// ---------------------------------------------------------------------------

pub struct Av1DecoderNode {
    config: Av1DecoderConfig,
}

impl Av1DecoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: Av1DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for Av1DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            })],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Nv12,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("Av1DecoderNode starting");
        let mut input_rx = context.take_input("in")?;
        let video_pool = context.video_pool.clone();

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("av1_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("av1_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, mut decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let decoder_threads = self.config.threads;
        let decode_task = tokio::task::spawn_blocking(move || {
            let mut decoder = match Av1Decoder::new(decoder_threads) {
                Ok(decoder) => decoder,
                Err(err) => {
                    let _ = result_tx.blocking_send(Err(err));
                    return;
                },
            };

            while let Some((data, metadata)) = decode_rx.blocking_recv() {
                let decode_start_time = Instant::now();
                let result = decoder.decode_packet(&data, metadata, video_pool.as_ref());
                decode_duration_histogram.record(decode_start_time.elapsed().as_secs_f64(), &[]);

                match result {
                    Ok(frames) => {
                        for frame in frames {
                            if result_tx.blocking_send(Ok(frame)).is_err() {
                                return;
                            }
                        }
                    },
                    Err(err) => {
                        let _ = result_tx.blocking_send(Err(err));
                    },
                }
            }

            // Flush remaining frames from the decoder.
            match decoder.flush(video_pool.as_ref()) {
                Ok(frames) => {
                    for frame in frames {
                        if result_tx.blocking_send(Ok(frame)).is_err() {
                            return;
                        }
                    }
                },
                Err(err) => {
                    let _ = result_tx.blocking_send(Err(err));
                },
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

        let decode_tx_clone = decode_tx.clone();
        let mut input_task = tokio::spawn(async move {
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Binary { data, metadata, .. } = packet {
                        if decode_tx_clone.send((data, metadata)).await.is_err() {
                            tracing::error!(
                                "Av1DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("Av1DecoderNode input stream closed");
        });

        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            decode_task,
            decode_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            Packet::Video,
            "Av1DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("Av1DecoderNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoder node
// ---------------------------------------------------------------------------

pub struct Av1EncoderNode {
    config: Av1EncoderConfig,
}

impl Av1EncoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: Av1EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for Av1EncoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::I420,
                }),
                PacketType::RawVideo(RawVideoFormat {
                    width: None,
                    height: None,
                    pixel_format: PixelFormat::Nv12,
                }),
            ],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Av1,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some(AV1_CONTENT_TYPE.to_string())
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("Av1EncoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("av1_packets_processed").build();
        let encode_duration_histogram = meter
            .f64_histogram("av1_encode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (encode_tx, mut encode_rx) =
            mpsc::channel::<(VideoFrame, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<EncodedPacket, String>>(get_codec_channel_capacity());

        let encoder_config = self.config;
        let encode_task = tokio::task::spawn_blocking(move || {
            let mut encoder: Option<Av1Encoder> = None;
            let mut current_dimensions: Option<(u32, u32)> = None;

            while let Some((frame, metadata)) = encode_rx.blocking_recv() {
                if frame.pixel_format == PixelFormat::Rgba8 {
                    let _ =
                        result_tx.blocking_send(Err("AV1 encoder requires NV12 or I420 input; \
                         insert a video::pixel_convert node upstream"
                            .to_string()));
                    continue;
                }

                let frame_dimensions = (frame.width, frame.height);
                if current_dimensions != Some(frame_dimensions) {
                    match Av1Encoder::new(frame.width, frame.height, &encoder_config) {
                        Ok(new_encoder) => {
                            encoder = Some(new_encoder);
                            current_dimensions = Some(frame_dimensions);
                        },
                        Err(err) => {
                            let _ = result_tx.blocking_send(Err(err));
                            continue;
                        },
                    }
                }

                let Some(encoder) = encoder.as_mut() else {
                    let _ = result_tx.blocking_send(Err("AV1 encoder not initialized".to_string()));
                    continue;
                };

                let encode_start_time = Instant::now();
                let result = encoder.encode_frame(&frame, metadata);
                encode_duration_histogram.record(encode_start_time.elapsed().as_secs_f64(), &[]);

                match result {
                    Ok(packets) => {
                        for packet in packets {
                            if result_tx.blocking_send(Ok(packet)).is_err() {
                                return;
                            }
                        }
                    },
                    Err(err) => {
                        let _ = result_tx.blocking_send(Err(err));
                    },
                }
            }

            if let Some(encoder) = encoder.as_mut() {
                match encoder.flush() {
                    Ok(packets) => {
                        for packet in packets {
                            if result_tx.blocking_send(Ok(packet)).is_err() {
                                return;
                            }
                        }
                    },
                    Err(err) => {
                        let _ = result_tx.blocking_send(Err(err));
                    },
                }
            }
        });

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let batch_size = context.batch_size;

        let encode_tx_clone = encode_tx.clone();
        let mut input_task = tokio::spawn(async move {
            loop {
                let Some(first_packet) = input_rx.recv().await else {
                    break;
                };

                let packet_batch =
                    packet_helpers::batch_packets_greedy(first_packet, &mut input_rx, batch_size);

                for packet in packet_batch {
                    if let Packet::Video(mut frame) = packet {
                        let metadata = frame.metadata.take();
                        if encode_tx_clone.send((frame, metadata)).await.is_err() {
                            tracing::error!(
                                "Av1EncoderNode encode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("Av1EncoderNode input stream closed");
        });

        crate::codec_utils::codec_forward_loop(
            &mut context,
            &mut result_rx,
            &mut input_task,
            encode_task,
            encode_tx,
            &packets_processed_counter,
            &mut stats_tracker,
            |encoded| Packet::Binary {
                data: encoded.data,
                content_type: Some(Cow::Borrowed(AV1_CONTENT_TYPE)),
                metadata: encoded.metadata,
            },
            "Av1EncoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("Av1EncoderNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal codec types
// ---------------------------------------------------------------------------

struct EncodedPacket {
    data: Bytes,
    metadata: Option<PacketMetadata>,
}

// ---------------------------------------------------------------------------
// rav1d-based AV1 decoder
// ---------------------------------------------------------------------------

struct Av1Decoder {
    /// Opaque rav1d context handle.  Owned; closed in [`Drop`].
    ctx: rav1d::include::dav1d::dav1d::Dav1dContext,
}

// SAFETY: The rav1d context is internally thread-safe (like dav1d).
unsafe impl Send for Av1Decoder {}

impl Av1Decoder {
    fn new(threads: u32) -> Result<Self, String> {
        use rav1d::include::dav1d::dav1d::Dav1dSettings;
        use std::ffi::c_int;
        use std::ptr::NonNull;

        let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` writes a valid `Dav1dSettings` into `s`.
        unsafe {
            rav1d::src::lib::dav1d_default_settings(
                // `MaybeUninit::as_mut_ptr()` always returns a valid, non-null
                // pointer so this `unwrap_or_else` is purely defensive.
                NonNull::new(settings.as_mut_ptr())
                    .unwrap_or_else(|| unreachable!("MaybeUninit pointer is never null")),
            );
        }
        let mut settings = unsafe { settings.assume_init() };
        #[allow(clippy::cast_possible_wrap)]
        {
            settings.n_threads = threads.max(1) as c_int;
        }
        // Optimise for low latency: emit frames as soon as possible.
        settings.max_frame_delay = 1;

        let mut ctx_slot: Option<rav1d::include::dav1d::dav1d::Dav1dContext> = None;
        // SAFETY: `ctx_slot` is valid to write into; `settings` is valid to read.
        let res = unsafe {
            rav1d::src::lib::dav1d_open(
                // Stack pointers are guaranteed non-null.
                NonNull::new(&raw mut ctx_slot),
                NonNull::new(&raw mut settings),
            )
        };
        if res.0 < 0 {
            return Err(format!("rav1d: dav1d_open failed with code {}", res.0));
        }
        let ctx = ctx_slot.ok_or_else(|| "rav1d: dav1d_open returned null context".to_string())?;
        Ok(Self { ctx })
    }

    fn decode_packet(
        &mut self,
        data: &[u8],
        metadata: Option<PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        use rav1d::include::dav1d::data::Dav1dData;
        use std::ptr::NonNull;

        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Wrap the input data in a `Dav1dData`.
        let mut dav1d_data: Dav1dData = Dav1dData::default();
        let buf_ptr = unsafe {
            rav1d::src::lib::dav1d_data_create(NonNull::new(&raw mut dav1d_data), data.len())
        };
        if buf_ptr.is_null() {
            return Err("rav1d: failed to allocate Dav1dData buffer".to_string());
        }
        // Copy our data into the rav1d-managed buffer.
        // SAFETY: `dav1d_data_create` returned a valid buffer of `data.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, data.len());
        }

        // Feed data to the decoder.
        let res = unsafe {
            rav1d::src::lib::dav1d_send_data(Some(self.ctx), NonNull::new(&raw mut dav1d_data))
        };
        // EAGAIN (-11) means the decoder has buffered frames; we still try to
        // pull pictures below.  Any other negative value is a real error.
        if res.0 < 0 && res.0 != -11 {
            return Err(format!("rav1d: dav1d_send_data failed with code {}", res.0));
        }

        // Drain available pictures.
        self.drain_pictures(metadata, video_pool)
    }

    /// Pull all currently available decoded pictures from the decoder.
    fn drain_pictures(
        &mut self,
        metadata: Option<PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        use rav1d::include::dav1d::picture::Dav1dPicture;
        use std::ptr::NonNull;

        let mut frames = Vec::with_capacity(1);
        let remaining_metadata = metadata;

        loop {
            let mut pic: Dav1dPicture = Dav1dPicture::default();
            let res = unsafe {
                rav1d::src::lib::dav1d_get_picture(Some(self.ctx), NonNull::new(&raw mut pic))
            };
            if res.0 == -11 {
                // EAGAIN — no more pictures available right now.
                break;
            }
            if res.0 < 0 {
                // A real error — but if we already collected frames, return them
                // and report the error on the next call.
                if !frames.is_empty() {
                    break;
                }
                return Err(format!("rav1d: dav1d_get_picture failed with code {}", res.0));
            }

            let meta = remaining_metadata.clone();

            match copy_dav1d_picture(&pic, meta, video_pool) {
                Ok(frame) => frames.push(frame),
                Err(err) => {
                    unsafe {
                        rav1d::src::lib::dav1d_picture_unref(NonNull::new(&raw mut pic));
                    }
                    if frames.is_empty() {
                        return Err(err);
                    }
                    break;
                },
            }

            unsafe {
                rav1d::src::lib::dav1d_picture_unref(NonNull::new(&raw mut pic));
            }
        }

        Ok(frames)
    }

    /// Flush the decoder (signal end-of-stream) and drain remaining pictures.
    fn flush(
        &mut self,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        // Signal end-of-stream by flushing the decoder.
        unsafe {
            rav1d::src::lib::dav1d_flush(self.ctx);
        }
        // Drain any remaining pictures.
        self.drain_pictures(None, video_pool)
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        use std::ptr::NonNull;
        let mut ctx_slot = Some(self.ctx);
        unsafe {
            rav1d::src::lib::dav1d_close(NonNull::new(&raw mut ctx_slot));
        }
    }
}

/// Copy a decoded `Dav1dPicture` (I420) into an NV12 [`VideoFrame`].
///
/// rav1d always decodes AV1 to I420 (three separate Y, U, V planes).
/// We convert to NV12 on the fly by copying the Y plane as-is and
/// interleaving the U and V planes into a single UV plane.
fn copy_dav1d_picture(
    pic: &rav1d::include::dav1d::picture::Dav1dPicture,
    metadata: Option<PacketMetadata>,
    video_pool: Option<&Arc<VideoFramePool>>,
) -> Result<VideoFrame, String> {
    use std::ffi::c_int;

    let width = pic.p.w;
    let height = pic.p.h;
    if width <= 0 || height <= 0 {
        return Err("AV1 decoder produced empty frame".to_string());
    }

    #[allow(clippy::cast_sign_loss)]
    let width = width as u32;
    #[allow(clippy::cast_sign_loss)]
    let height = height as u32;

    // Y plane
    let y_ptr = pic.data[0].ok_or("AV1 decoder returned null Y plane")?;
    let y_stride = pic.stride[0];
    if y_stride <= 0 {
        return Err("AV1 decoder returned invalid Y stride".to_string());
    }

    // U plane
    let u_ptr = pic.data[1].ok_or("AV1 decoder returned null U plane")?;
    let u_stride = pic.stride[1];
    if u_stride <= 0 {
        return Err("AV1 decoder returned invalid U stride".to_string());
    }

    // V plane
    let v_ptr = pic.data[2].ok_or("AV1 decoder returned null V plane")?;
    // V shares the chroma stride with U in dav1d's layout.
    let v_stride = u_stride;

    // Output layout is NV12 (Y + interleaved UV).
    let nv12_layout = VideoLayout::packed(width, height, PixelFormat::Nv12);
    let mut data = video_pool.map_or_else(
        || PooledVideoData::from_vec(vec![0u8; nv12_layout.total_bytes()]),
        |pool| pool.get(nv12_layout.total_bytes()),
    );
    let data_slice = data.as_mut_slice();

    let nv12_planes = nv12_layout.planes();
    let y_plane = nv12_planes[0];
    let uv_plane = nv12_planes[1];

    // Copy Y plane.
    copy_dav1d_plane(
        &mut data_slice[y_plane.offset..y_plane.offset + y_plane.stride * y_plane.height as usize],
        y_plane.stride,
        y_ptr.as_ptr().cast::<u8>(),
        #[allow(clippy::cast_possible_truncation)]
        {
            y_stride as c_int
        },
        width as usize,
        height as usize,
    )?;

    // Interleave U + V into NV12's single UV plane.
    let chroma_w = (width as usize).div_ceil(2);
    let chroma_h = uv_plane.height as usize;

    #[allow(clippy::cast_sign_loss)]
    let u_stride_usize = u_stride as usize;
    #[allow(clippy::cast_sign_loss)]
    let v_stride_usize = v_stride as usize;

    for row in 0..chroma_h {
        let u_row = unsafe {
            std::slice::from_raw_parts(
                u_ptr.as_ptr().cast::<u8>().add(row * u_stride_usize),
                chroma_w,
            )
        };
        let v_row = unsafe {
            std::slice::from_raw_parts(
                v_ptr.as_ptr().cast::<u8>().add(row * v_stride_usize),
                chroma_w,
            )
        };
        let dst_start = uv_plane.offset + row * uv_plane.stride;
        for col in 0..chroma_w {
            data_slice[dst_start + col * 2] = u_row[col];
            data_slice[dst_start + col * 2 + 1] = v_row[col];
        }
    }

    VideoFrame::from_pooled(width, height, PixelFormat::Nv12, data, metadata)
        .map_err(|e| e.to_string())
}

fn copy_dav1d_plane(
    dst: &mut [u8],
    dst_stride: usize,
    src_ptr: *const u8,
    src_stride: std::ffi::c_int,
    width: usize,
    height: usize,
) -> Result<(), String> {
    if src_stride <= 0 {
        return Err("Invalid source stride for AV1 plane".to_string());
    }
    #[allow(clippy::cast_sign_loss)]
    let src_stride = src_stride as usize;

    for row in 0..height {
        let src_row = unsafe { std::slice::from_raw_parts(src_ptr.add(row * src_stride), width) };
        let dst_start = row * dst_stride;
        let dst_end = dst_start + width;
        if dst_end > dst.len() {
            return Err("AV1 plane copy overflow".to_string());
        }
        dst[dst_start..dst_end].copy_from_slice(src_row);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// rav1e-based AV1 encoder
// ---------------------------------------------------------------------------

struct Av1Encoder {
    ctx: Context<u8>,
    next_pts: i64,
}

impl Av1Encoder {
    fn new(width: u32, height: u32, config: &Av1EncoderConfig) -> Result<Self, String> {
        #[allow(clippy::cast_possible_truncation)]
        let speed = config.speed.min(10) as u8;
        let speed_settings = SpeedSettings::from_preset(speed);

        // When bitrate_kbps is 0, use constant-quality mode (quantizer-based).
        // Otherwise use bitrate-based rate control.
        let (bitrate, quantizer) = if config.bitrate_kbps == 0 {
            (0, 100) // CQ mode with a reasonable default quantizer
        } else {
            (i32::try_from(config.bitrate_kbps).unwrap_or(i32::MAX), 0)
        };

        let enc_cfg = EncoderConfig {
            width: width as usize,
            height: height as usize,
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            chroma_sample_position: ChromaSamplePosition::Unknown,
            // time_base is the reciprocal of the frame rate.  rav1e uses it
            // internally for rate-control and tiling constraints; using a very
            // small value (e.g. 1/1_000_000) would be interpreted as ~1M fps
            // and trigger AV1-spec tile-rate limits that panic on small frames.
            // 1/30 is a safe default for 30 fps content.
            time_base: Rational { num: 1, den: 30 },
            low_latency: config.low_latency,
            min_key_frame_interval: 0,
            max_key_frame_interval: u64::from(config.keyframe_interval.max(1)),
            bitrate,
            quantizer,
            speed_settings,
            ..Default::default()
        };

        let rav1e_cfg = Config::default()
            .with_encoder_config(enc_cfg)
            .with_threads(config.threads.max(1) as usize);

        let ctx: Context<u8> = rav1e_cfg
            .new_context()
            .map_err(|e| format!("rav1e: failed to create encoder context: {e}"))?;

        Ok(Self { ctx, next_pts: 0 })
    }

    fn encode_frame(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        if !matches!(frame.pixel_format, PixelFormat::I420 | PixelFormat::Nv12) {
            return Err(format!(
                "AV1 encoder expects I420 or NV12 input, got {:?}",
                frame.pixel_format
            ));
        }

        let layout = frame.layout();
        if frame.data_len() < layout.total_bytes() {
            return Err(format!(
                "AV1 encoder expected {} bytes, got {}",
                layout.total_bytes(),
                frame.data_len()
            ));
        }

        let width = frame.width as usize;
        let height = frame.height as usize;

        // Build a rav1e Frame with I420 data.
        let mut rav1e_frame = self.ctx.new_frame();
        let data = frame.data.as_slice();

        match frame.pixel_format {
            PixelFormat::I420 => {
                let planes = layout.planes();
                // Y plane
                copy_plane_to_rav1e(&mut rav1e_frame.planes[0], data, &planes[0], width, height);
                // U plane
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                copy_plane_to_rav1e(
                    &mut rav1e_frame.planes[1],
                    data,
                    &planes[1],
                    chroma_w,
                    chroma_h,
                );
                // V plane
                copy_plane_to_rav1e(
                    &mut rav1e_frame.planes[2],
                    data,
                    &planes[2],
                    chroma_w,
                    chroma_h,
                );
            },
            PixelFormat::Nv12 => {
                let planes = layout.planes();
                // Y plane — direct copy.
                copy_plane_to_rav1e(&mut rav1e_frame.planes[0], data, &planes[0], width, height);
                // NV12 has interleaved UV — de-interleave into separate U and V planes.
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let uv_plane = &planes[1];
                let u_stride = rav1e_frame.planes[1].cfg.stride;
                let v_stride = rav1e_frame.planes[2].cfg.stride;
                for row in 0..chroma_h {
                    let src_start = uv_plane.offset + row * uv_plane.stride;
                    let u_data = rav1e_frame.planes[1].data_origin_mut();
                    for col in 0..chroma_w {
                        u_data[row * u_stride + col] = data[src_start + col * 2];
                    }
                    let v_data = rav1e_frame.planes[2].data_origin_mut();
                    for col in 0..chroma_w {
                        v_data[row * v_stride + col] = data[src_start + col * 2 + 1];
                    }
                }
            },
            _ => unreachable!("already checked above"),
        }

        let (pts, duration) = self.next_pts(metadata.as_ref());

        let is_keyframe_request = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);
        let frame_params = if is_keyframe_request {
            Some(FrameParameters {
                frame_type_override: FrameTypeOverride::Key,
                opaque: None,
                ..Default::default()
            })
        } else {
            None
        };

        // Send frame to rav1e.
        let send_result = match frame_params {
            Some(params) => self.ctx.send_frame((rav1e_frame, params)),
            None => self.ctx.send_frame(rav1e_frame),
        };
        if let Err(e) = send_result {
            if e != EncoderStatus::EnoughData {
                return Err(format!("rav1e: send_frame failed: {e}"));
            }
        }

        // Drain available packets.
        self.drain_packets(metadata, pts, duration)
    }

    fn drain_packets(
        &mut self,
        metadata: Option<PacketMetadata>,
        pts: i64,
        duration: u64,
    ) -> Result<Vec<EncodedPacket>, String> {
        let mut packets = Vec::new();
        let mut remaining_metadata = metadata;

        loop {
            match self.ctx.receive_packet() {
                Ok(pkt) => {
                    let is_keyframe = pkt.frame_type == FrameType::KEY;
                    let data = Bytes::copy_from_slice(&pkt.data);

                    let meta = remaining_metadata.take();
                    let output_metadata = merge_keyframe_metadata(meta, is_keyframe, pts, duration);

                    packets.push(EncodedPacket { data, metadata: Some(output_metadata) });
                },
                Err(
                    EncoderStatus::NeedMoreData
                    | EncoderStatus::Encoded
                    | EncoderStatus::LimitReached,
                ) => break,
                Err(e) => return Err(format!("rav1e: receive_packet failed: {e}")),
            }
        }

        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, String> {
        self.ctx.flush();

        let mut output = Vec::new();
        loop {
            match self.ctx.receive_packet() {
                Ok(pkt) => {
                    let is_keyframe = pkt.frame_type == FrameType::KEY;
                    let data = Bytes::copy_from_slice(&pkt.data);
                    let meta = merge_keyframe_metadata(None, is_keyframe, 0, 0);
                    output.push(EncodedPacket { data, metadata: Some(meta) });
                },
                Err(EncoderStatus::LimitReached) => break,
                Err(EncoderStatus::NeedMoreData | EncoderStatus::Encoded) => {
                    // In low-latency mode with no buffered frames, these are
                    // expected after flush completes.
                    break;
                },
                Err(e) => return Err(format!("rav1e: flush receive_packet failed: {e}")),
            }
        }

        Ok(output)
    }

    fn next_pts(&mut self, metadata: Option<&PacketMetadata>) -> (i64, u64) {
        let duration = metadata.and_then(|meta| meta.duration_us).unwrap_or(1);

        let pts =
            metadata.and_then(|meta| meta.timestamp_us).map_or(self.next_pts, u64::cast_signed);

        self.next_pts = if duration > 0 { pts + duration.cast_signed() } else { pts + 1 };
        (pts, duration)
    }
}

/// Copy a plane from the source layout into a rav1e `Plane<u8>`.
fn copy_plane_to_rav1e(
    dst: &mut rav1e::prelude::Plane<u8>,
    src_data: &[u8],
    src_plane: &VideoPlane,
    width: usize,
    height: usize,
) {
    let dst_stride = dst.cfg.stride;
    let dst_data = dst.data_origin_mut();
    for row in 0..height {
        let src_start = src_plane.offset + row * src_plane.stride;
        let dst_start = row * dst_stride;
        let src_end = (src_start + width).min(src_data.len());
        let copy_width = src_end - src_start;
        dst_data[dst_start..dst_start + copy_width].copy_from_slice(&src_data[src_start..src_end]);
    }
}

const fn merge_keyframe_metadata(
    metadata: Option<PacketMetadata>,
    keyframe: bool,
    pts: i64,
    duration: u64,
) -> PacketMetadata {
    match metadata {
        Some(mut meta) => {
            meta.keyframe = Some(keyframe);
            meta
        },
        None => PacketMetadata {
            timestamp_us: if pts >= 0 { Some(pts.cast_unsigned()) } else { None },
            duration_us: if duration > 0 { Some(duration) } else { None },
            sequence: None,
            keyframe: Some(keyframe),
        },
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_av1_nodes(registry: &mut NodeRegistry) {
    let default_decoder = Av1DecoderNode::new(Av1DecoderConfig::default())
        .expect("default AV1 decoder config should be valid");
    registry.register_static_with_description(
        "video::av1::decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Av1DecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(Av1DecoderConfig))
            .expect("Av1DecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "av1".to_string()],
        false,
        "Decodes AV1-compressed packets into raw NV12 video frames using rav1d (pure-Rust dav1d). \
         Use this before CPU compositing or analysis pipelines.",
    );

    let default_encoder = Av1EncoderNode::new(Av1EncoderConfig::default())
        .expect("default AV1 encoder config should be valid");
    registry.register_static_with_description(
        "video::av1::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Av1EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(Av1EncoderConfig))
            .expect("Av1EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "av1".to_string()],
        false,
        "Encodes raw video frames (NV12 or I420) into AV1 packets using rav1e (pure-Rust). \
         Insert a video::pixel_convert node upstream if the source outputs RGBA8.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
        create_test_video_frame,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_av1_encode_decode_roundtrip() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = Av1EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 0,
            threads: 1,
            speed: 10,
            low_latency: true,
        };
        let encoder = Av1EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..5 {
            let timestamp = 1_000 + 33_333_u64 * index;
            let duration: u64 = 33_333;

            let mut frame = create_test_video_frame(256, 256, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp),
                duration_us: Some(duration),
                sequence: Some(index),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "AV1 encoder produced no packets");

        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = Av1DecoderNode::new(Av1DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            if let Packet::Binary { data, metadata, .. } = packet {
                dec_input_tx
                    .send(Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed(AV1_CONTENT_TYPE)),
                        metadata,
                    })
                    .await
                    .unwrap();
            }
        }
        drop(dec_input_tx);

        assert_state_stopped(&mut dec_state_rx).await;
        dec_handle.await.unwrap().unwrap();

        let decoded_packets = dec_sender.get_packets_for_pin("out").await;
        assert!(!decoded_packets.is_empty(), "AV1 decoder produced no frames");

        for packet in decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 256);
                    assert_eq!(frame.height, 256);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");
                },
                _ => panic!("Expected Video packet from AV1 decoder"),
            }
        }
    }
}
