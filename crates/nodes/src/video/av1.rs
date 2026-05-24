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
use std::sync::Arc;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    EncodedVideoFormat, Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat,
    VideoCodec, VideoFrame, VideoPlane,
};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
    VideoFramePool,
};
use tokio::sync::mpsc;

use super::{AV1_CONTENT_TYPE, EAGAIN_YIELD_THRESHOLD, MAX_EAGAIN_EMPTY_RETRIES};

/// Default to constant-quality mode (quantizer-based).  In bitrate mode
/// rav1e buffers 10+ frames for rate-control look-ahead before producing
/// any output, even with `low_latency: true`.  CQ mode emits packets
/// immediately, which is far better for real-time pipelines.
const AV1_DEFAULT_BITRATE_KBPS: u32 = 0;
const AV1_DEFAULT_KF_INTERVAL: u32 = 120;
/// Default to auto-detect (`0`).  rav1e delegates to rayon (uses all
/// logical cores) and rav1d auto-detects like C dav1d.  Note: at
/// speed=10 tile parallelism is ineffective, so this has minimal
/// real-world impact on encode throughput.
const AV1_DEFAULT_THREADS: u32 = 0;
const AV1_DEFAULT_SPEED: u32 = 10;

/// rav1d / dav1d error code for EAGAIN ("input not consumed, drain pictures
/// first").  Matches libc `EAGAIN` on Linux.
const DAV1D_EAGAIN: i32 = -11;

// AV1 OBU validation
/// Read a LEB128-encoded unsigned integer from `data`.
/// Returns `(value, bytes_consumed)` or an error if the data is truncated
/// or the encoding exceeds the 8-byte AV1-spec limit.
fn read_leb128(data: &[u8]) -> Result<(u64, usize), &'static str> {
    let mut value: u64 = 0;
    for i in 0..8 {
        if i >= data.len() {
            return Err("truncated LEB128 size field");
        }
        let byte = u64::from(data[i]);
        value |= (byte & 0x7F) << (i * 7);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err("LEB128 size exceeds 8-byte AV1 limit")
}

/// Walk the OBUs in `data` and verify structural integrity.
///
/// Returns `Ok(())` when every OBU header is well-formed and its declared
/// size fits inside the remaining buffer.  Returns `Err` on the first
/// issue found.
///
/// This is a defence against rav1d 1.1.0's internal panic
/// (`decode.rs:4997`) when its error handler accesses an uninitialised
/// `frame_hdr` while recovering from a corrupted bitstream.  Because
/// rav1d's `dav1d_send_data` is `extern "C"`, a panic inside it aborts
/// the process — so we must reject obviously malformed data *before* it
/// reaches the decoder.
fn validate_av1_obus(data: &[u8]) -> Result<(), &'static str> {
    let mut offset = 0;
    while offset < data.len() {
        let header_byte = data[offset];

        // Bit 7 — forbidden bit (must be 0).
        if header_byte & 0x80 != 0 {
            return Err("forbidden bit set in OBU header");
        }

        let obu_type = (header_byte >> 3) & 0x0F;
        let has_extension = header_byte & 0x04 != 0;
        let has_size = header_byte & 0x02 != 0;

        // Valid OBU types: 1–8, 15.
        match obu_type {
            1..=8 | 15 => {},
            _ => return Err("invalid OBU type"),
        }

        offset += 1;

        // Extension byte (temporal_id, spatial_id, reserved).
        if has_extension {
            if offset >= data.len() {
                return Err("truncated OBU extension byte");
            }
            offset += 1;
        }

        if has_size {
            let remaining = &data[offset..];
            let (size, leb_bytes) = read_leb128(remaining)?;
            offset += leb_bytes;
            let left = data.len() - offset;
            if size > left as u64 {
                return Err("OBU size exceeds remaining data");
            }
            offset += usize::try_from(size).map_err(|_| "OBU size too large for platform")?;
        } else {
            // No size field — the OBU extends to end of data.
            break;
        }
    }
    Ok(())
}

// Configuration structs
#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Av1DecoderConfig {
    /// Number of decoder threads.  `0` = auto-detect (rav1d picks a
    /// thread count based on the number of logical cores, matching
    /// C dav1d behaviour).
    pub threads: u32,
}

impl Default for Av1DecoderConfig {
    fn default() -> Self {
        Self { threads: AV1_DEFAULT_THREADS }
    }
}

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Av1EncoderConfig {
    pub bitrate_kbps: u32,
    pub keyframe_interval: u32,
    /// Number of encoder threads.  `0` = auto-detect (rav1e delegates
    /// to rayon, using all available logical cores).
    pub threads: u32,
    /// rav1e speed preset (0 = slowest/best quality, 10 = fastest/real-time).
    pub speed: u32,
    /// Enable rav1e low-latency mode (disables frame reordering).
    pub low_latency: bool,
    /// Constant-quality quantizer (0–255 scale, lower = better quality).
    ///
    /// Only used when `bitrate_kbps` is 0 (constant-quality mode).
    /// Default: 80.
    pub quantizer: u32,
}

impl Default for Av1EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_kbps: AV1_DEFAULT_BITRATE_KBPS,
            keyframe_interval: AV1_DEFAULT_KF_INTERVAL,
            threads: AV1_DEFAULT_THREADS,
            speed: AV1_DEFAULT_SPEED,
            low_latency: true,
            quantizer: 80,
        }
    }
}

// Decoder node
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
        let packets_processed_counter = meter.u64_counter("av1_decoder_packets_processed").build();
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
                // Exit early if the async side has been cancelled (e.g.
                // tokio runtime shutting down).
                if result_tx.is_closed() {
                    return;
                }

                let decode_start_time = Instant::now();
                let result = decoder.decode_packet(&data, metadata.as_ref(), video_pool.as_ref());
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

            // Flush remaining frames from the decoder — skip if the
            // async side is already gone (runtime shutting down).
            if result_tx.is_closed() {
                return;
            }
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

// Encoder node
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

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for Av1EncoderNode {
    const CONTENT_TYPE: &'static str = AV1_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "Av1EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "av1_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "av1_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<Av1Encoder>(
            self.config,
            encode_rx,
            result_tx,
            duration_histogram,
        )
    }
}

impl StandardVideoEncoder for Av1Encoder {
    type Config = Av1EncoderConfig;
    const CODEC_NAME: &'static str = "AV1";

    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String> {
        Self::new(width, height, config)
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        self.encode_frame(frame, metadata)
    }

    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String> {
        self.flush()
    }

    fn flush_on_dimension_change() -> bool {
        true
    }
}

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};

// rav1d-based AV1 decoder
struct Av1Decoder {
    /// Opaque rav1d context handle.  Owned; closed in [`Drop`].
    ctx: rav1d::include::dav1d::dav1d::Dav1dContext,
}

// `Av1Decoder` is intentionally `!Send` (inherits from `Dav1dContext`).
// It is created and used entirely inside a single `spawn_blocking` closure
// in `Av1DecoderNode::run`, so no `Send` bound is required — it is a local
// variable, not a captured value.

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
        settings.n_threads = c_int::try_from(threads).unwrap_or(0);
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
        metadata: Option<&PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        use rav1d::include::dav1d::data::Dav1dData;
        use std::ptr::NonNull;

        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Validate OBU structure before feeding data to rav1d.  rav1d 1.1.0
        // panics inside its error handler when processing truncated or
        // corrupted bitstreams, and because the entry point is `extern "C"`
        // the panic aborts the process.
        //
        // Note: the C dav1d decoder (`video::dav1d::decoder`) handles corrupt
        // data natively via negative error codes and does not need this
        // pre-validation step.
        if let Err(reason) = validate_av1_obus(data) {
            tracing::warn!(size = data.len(), reason, "Skipping malformed AV1 packet");
            return Ok(Vec::new());
        }

        let mut dav1d_data: Dav1dData = Dav1dData::default();
        let buf_ptr = unsafe {
            rav1d::src::lib::dav1d_data_create(NonNull::new(&raw mut dav1d_data), data.len())
        };
        if buf_ptr.is_null() {
            return Err("rav1d: failed to allocate Dav1dData buffer".to_string());
        }
        // SAFETY: `dav1d_data_create` returned a valid buffer of `data.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, data.len());
        }

        let mut data_guard = Dav1dDataGuard::new(&raw mut dav1d_data);

        let mut all_frames = Vec::new();

        // dav1d API: EAGAIN (-11) means the input was not consumed; drain
        // pending pictures then retry.  Retries are bounded to prevent a
        // busy-loop that would hang the tokio runtime on shutdown.
        let mut eagain_empty_retries: u32 = 0;

        loop {
            let res = unsafe {
                rav1d::src::lib::dav1d_send_data(Some(self.ctx), NonNull::new(&raw mut dav1d_data))
            };

            if res.0 == 0 {
                data_guard.defuse();
                break;
            }

            if res.0 == DAV1D_EAGAIN {
                let mut drained = self.drain_pictures(metadata, video_pool)?;
                if drained.is_empty() {
                    eagain_empty_retries += 1;
                    if eagain_empty_retries > MAX_EAGAIN_EMPTY_RETRIES {
                        return Err("rav1d: dav1d_send_data stuck in EAGAIN loop \
                             (no pictures produced after 1000 retries)"
                            .to_string());
                    }
                    // Progressive backoff to avoid a tight spin-loop.
                    if eagain_empty_retries <= EAGAIN_YIELD_THRESHOLD {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                } else {
                    eagain_empty_retries = 0;
                }
                all_frames.append(&mut drained);
                continue;
            }

            return Err(format!("rav1d: dav1d_send_data failed with code {}", res.0));
        }

        let mut drained = self.drain_pictures(metadata, video_pool)?;
        all_frames.append(&mut drained);

        Ok(all_frames)
    }

    /// Pull all currently available decoded pictures from the decoder.
    fn drain_pictures(
        &mut self,
        metadata: Option<&PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        use rav1d::include::dav1d::picture::Dav1dPicture;
        use std::ptr::NonNull;

        let mut frames = Vec::with_capacity(1);
        loop {
            let mut pic: Dav1dPicture = Dav1dPicture::default();
            let res = unsafe {
                rav1d::src::lib::dav1d_get_picture(Some(self.ctx), NonNull::new(&raw mut pic))
            };
            if res.0 == DAV1D_EAGAIN {
                break;
            }
            if res.0 < 0 {
                if !frames.is_empty() {
                    tracing::warn!(
                        "rav1d: dav1d_get_picture error {} after draining {} frame(s) — \
                         returning buffered frames",
                        res.0,
                        frames.len(),
                    );
                    break;
                }
                return Err(format!("rav1d: dav1d_get_picture failed with code {}", res.0));
            }

            let meta = metadata.cloned();

            match copy_dav1d_picture(&pic, meta, video_pool) {
                Ok(frame) => frames.push(frame),
                Err(err) => {
                    unsafe {
                        rav1d::src::lib::dav1d_picture_unref(NonNull::new(&raw mut pic));
                    }
                    if frames.is_empty() {
                        return Err(err);
                    }
                    tracing::warn!(
                        "rav1d: copy_dav1d_picture error after draining {} frame(s) — \
                         returning buffered frames: {err}",
                        frames.len(),
                    );
                    break;
                },
            }

            unsafe {
                rav1d::src::lib::dav1d_picture_unref(NonNull::new(&raw mut pic));
            }
        }

        Ok(frames)
    }

    /// Drain remaining buffered pictures and reset the decoder.
    ///
    /// The dav1d API's `dav1d_flush` resets all inter-frame state (designed for
    /// seeking), so we must drain any buffered pictures **before** calling it.
    /// With `max_frame_delay = 1` there are typically no buffered frames at
    /// end-of-stream, but this ordering is correct for any delay setting.
    fn flush(
        &mut self,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        let frames = self.drain_pictures(None, video_pool)?;
        unsafe {
            rav1d::src::lib::dav1d_flush(self.ctx);
        }
        Ok(frames)
    }
}

/// RAII guard for a `Dav1dData` buffer.  Calls `dav1d_data_unref` on drop
/// unless explicitly defused (e.g. after `dav1d_send_data` consumes the data).
///
/// # Safety invariant
///
/// The raw pointer must remain valid for the guard's entire lifetime.
/// In practice this means the guard **must** be dropped before the
/// stack-local `Dav1dData` it points to goes out of scope.  Never store
/// this guard in a struct or return it from the function that creates it.
struct Dav1dDataGuard {
    ptr: *mut rav1d::include::dav1d::data::Dav1dData,
    active: bool,
}

impl Dav1dDataGuard {
    const fn new(ptr: *mut rav1d::include::dav1d::data::Dav1dData) -> Self {
        Self { ptr, active: true }
    }

    /// Prevent the guard from calling `dav1d_data_unref` on drop.
    const fn defuse(&mut self) {
        self.active = false;
    }
}

impl Drop for Dav1dDataGuard {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                rav1d::src::lib::dav1d_data_unref(std::ptr::NonNull::new(self.ptr));
            }
        }
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
    use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;

    // The chroma copy below assumes 4:2:0 subsampling (UV = width/2 × height/2).
    // Reject any other layout to avoid silently producing corrupted frames.
    if pic.p.layout != DAV1D_PIXEL_LAYOUT_I420 {
        return Err(format!(
            "AV1 decoder produced unsupported pixel layout {} (expected I420 = {})",
            pic.p.layout, DAV1D_PIXEL_LAYOUT_I420,
        ));
    }

    // Reject non-8-bit content.  rav1d is compiled with `bitdepth_16` so it
    // *can* decode 10-bit AV1, but the I420→NV12 copy below treats every
    // sample as a single byte.  Feeding higher bit-depth data would produce
    // silently corrupted output.
    if pic.p.bpc != 8 {
        return Err(format!(
            "AV1 decoder produced {}-bit content, but only 8-bit is supported",
            pic.p.bpc,
        ));
    }

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

    super::i420_to_nv12(
        &super::I420Planes {
            y_ptr: y_ptr.as_ptr().cast::<u8>(),
            u_ptr: u_ptr.as_ptr().cast::<u8>(),
            v_ptr: v_ptr.as_ptr().cast::<u8>(),
            y_stride,
            uv_stride: u_stride,
            width,
            height,
        },
        metadata,
        video_pool,
    )
}

// rav1e-based AV1 encoder
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
            (0, config.quantizer.min(255) as usize) // CQ mode with configurable quantizer (0–255 scale)
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

        let rav1e_cfg =
            Config::default().with_encoder_config(enc_cfg).with_threads(config.threads as usize);

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
                copy_plane_to_rav1e(&mut rav1e_frame.planes[0], data, &planes[0], width, height)?;
                // U plane
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                copy_plane_to_rav1e(
                    &mut rav1e_frame.planes[1],
                    data,
                    &planes[1],
                    chroma_w,
                    chroma_h,
                )?;
                // V plane
                copy_plane_to_rav1e(
                    &mut rav1e_frame.planes[2],
                    data,
                    &planes[2],
                    chroma_w,
                    chroma_h,
                )?;
            },
            PixelFormat::Nv12 => {
                let planes = layout.planes();
                // Y plane — direct copy.
                copy_plane_to_rav1e(&mut rav1e_frame.planes[0], data, &planes[0], width, height)?;
                // NV12 has interleaved UV — de-interleave into separate U and V planes.
                let chroma_w = width.div_ceil(2);
                let chroma_h = height.div_ceil(2);
                let uv_plane = &planes[1];
                // Hoist mutable borrows outside the loop.  We cannot borrow
                // two planes through `rav1e_frame.planes` simultaneously, so
                // use `split_at_mut` to get disjoint references.
                let (first_planes, rest) = rav1e_frame.planes.split_at_mut(2);
                let u_stride = first_planes[1].cfg.stride;
                let v_stride = rest[0].cfg.stride;
                let u_data = first_planes[1].data_origin_mut();
                let v_data = rest[0].data_origin_mut();
                for row in 0..chroma_h {
                    let src_start = uv_plane.offset + row * uv_plane.stride;
                    for col in 0..chroma_w {
                        u_data[row * u_stride + col] = data[src_start + col * 2];
                    }
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
        //
        // `EnoughData` means the encoder has been flushed or hit its frame limit
        // — it will not accept any more frames.  This should not happen during
        // normal encoding (we only flush at end-of-stream), but if it does the
        // frame is already consumed by `into()` and cannot be retried.  Log a
        // warning and continue to drain any remaining buffered packets.
        let send_result = match frame_params {
            Some(params) => self.ctx.send_frame((rav1e_frame, params)),
            None => self.ctx.send_frame(rav1e_frame),
        };
        if let Err(e) = send_result {
            if e == EncoderStatus::EnoughData {
                tracing::warn!(
                    "rav1e: send_frame returned EnoughData — encoder is no longer \
                     accepting frames (frame dropped)"
                );
            } else {
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
) -> Result<(), String> {
    let dst_stride = dst.cfg.stride;
    let dst_data = dst.data_origin_mut();
    for row in 0..height {
        let src_start = src_plane.offset + row * src_plane.stride;
        let dst_start = row * dst_stride;
        if src_start + width > src_data.len() {
            return Err(format!(
                "copy_plane_to_rav1e: source data too short (need {}, have {}) at row {row}",
                src_start + width,
                src_data.len(),
            ));
        }
        dst_data[dst_start..dst_start + width]
            .copy_from_slice(&src_data[src_start..src_start + width]);
    }
    Ok(())
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

// Registration
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)] // Default config and schema serialization should never fail
pub fn register_av1_nodes(registry: &mut NodeRegistry) {
    let default_decoder = Av1DecoderNode::new(Av1DecoderConfig::default())
        .expect("default AV1 decoder config should be valid");
    register_static_node!(
        registry,
        "video::av1::decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Av1DecoderNode::new(config)?))
        },
        Av1DecoderConfig,
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        ["video", "codecs", "av1"],
        "Decodes AV1-compressed packets into raw NV12 video frames using rav1d (pure-Rust dav1d). \
         Use this before CPU compositing or analysis pipelines.",
    );

    let default_encoder = Av1EncoderNode::new(Av1EncoderConfig::default())
        .expect("default AV1 encoder config should be valid");
    register_static_node!(
        registry,
        "video::av1::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Av1EncoderNode::new(config)?))
        },
        Av1EncoderConfig,
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        ["video", "codecs", "av1"],
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
    use std::borrow::Cow;
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
            quantizer: 80,
        };
        let encoder = Av1EncoderNode::new(encoder_config).unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..5 {
            let timestamp = 1_000 + 33_333_u64 * index;
            let duration: u64 = 33_333;

            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
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
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");
                },
                _ => panic!("Expected Video packet from AV1 decoder"),
            }
        }
    }

    /// Encode many frames rapidly, then decode them all — exercises the
    /// `dav1d_send_data` EAGAIN retry loop when the decoder's internal buffer
    /// is full and data cannot be consumed in a single call.
    #[tokio::test]
    async fn test_av1_decode_many_frames_no_data_loss() {
        const FRAME_COUNT: u64 = 10;

        // --- Encode ---
        let (enc_input_tx, enc_input_rx) = mpsc::channel(32);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 32);
        let encoder_config = Av1EncoderConfig {
            keyframe_interval: 10,
            bitrate_kbps: 0,
            threads: 1,
            speed: 10,
            low_latency: true,
            quantizer: 80,
        };
        let encoder = Av1EncoderNode::new(encoder_config).unwrap();
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0..FRAME_COUNT {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * index),
                duration_us: Some(33_333),
                sequence: Some(index),
                keyframe: Some(index % 10 == 0),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(
            !encoded_packets.is_empty(),
            "AV1 encoder produced no packets for {FRAME_COUNT} input frames"
        );

        // --- Decode ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(32);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 32);
        let decoder = Av1DecoderNode::new(Av1DecoderConfig { threads: 1 }).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        let encoded_count = encoded_packets.len();
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
        // With the EAGAIN retry fix, the decoder must not silently drop any
        // frames.  We should get at least as many decoded frames as encoded
        // packets (each packet should produce at least one frame).
        assert!(
            decoded_packets.len() >= encoded_count,
            "Decoded frame count ({}) should be >= encoded packet count ({}) — \
             data may have been dropped on EAGAIN",
            decoded_packets.len(),
            encoded_count,
        );

        for packet in &decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                },
                _ => panic!("Expected Video packet from AV1 decoder"),
            }
        }
    }

    /// Verify that decoded frames preserve the metadata (timestamp, duration)
    /// from the input packet — including when multiple frames are produced.
    #[tokio::test]
    async fn test_av1_metadata_propagation() {
        // --- Encode a few frames with known metadata ---
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
            quantizer: 80,
        };
        let encoder = Av1EncoderNode::new(encoder_config).unwrap();
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        let timestamps: Vec<u64> = vec![1_000, 34_333, 67_666];
        for (i, &ts) in timestamps.iter().enumerate() {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(ts),
                duration_us: Some(33_333),
                sequence: Some(i as u64),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty());

        // --- Decode and verify metadata is preserved ---
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
        assert!(!decoded_packets.is_empty(), "Decoder should produce at least one frame");

        // Every decoded frame should have metadata (the .clone() fix ensures
        // that even multi-frame outputs from a single packet all get metadata).
        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Video(frame) => {
                    assert!(
                        frame.metadata.is_some(),
                        "Decoded frame {i} should have metadata (was None — \
                         indicates the .clone() metadata fix regressed)"
                    );
                },
                _ => panic!("Expected Video packet from AV1 decoder"),
            }
        }
    }

    // OBU validation unit tests
    #[test]
    fn test_validate_av1_obus_valid_sequence_header() {
        // Minimal sequence header OBU: type=1, has_size=1, size=1, one payload byte.
        let data = [0x0a, 0x01, 0x00];
        assert!(validate_av1_obus(&data).is_ok());
    }

    #[test]
    fn test_validate_av1_obus_forbidden_bit() {
        let data = [0x8a, 0x01, 0x00]; // forbidden bit set
        assert!(validate_av1_obus(&data).is_err());
    }

    #[test]
    fn test_validate_av1_obus_invalid_type() {
        // OBU type 0 is reserved/invalid.
        let data = [0x02, 0x01, 0x00]; // type=0, has_size=1
        assert!(validate_av1_obus(&data).is_err());
    }

    #[test]
    fn test_validate_av1_obus_truncated_size() {
        // Sequence header OBU claiming size=10 but only 2 payload bytes.
        let data = [0x0a, 0x0a, 0x00, 0x00];
        assert!(validate_av1_obus(&data).is_err());
    }

    #[test]
    fn test_validate_av1_obus_truncated_extension() {
        // OBU with extension flag set but no extension byte.
        let data = [0x0e]; // type=1, extension=1, has_size=1
        assert!(validate_av1_obus(&data).is_err());
    }

    #[test]
    fn test_validate_av1_obus_empty() {
        assert!(validate_av1_obus(&[]).is_ok());
    }

    #[test]
    fn test_validate_av1_obus_multiple_obus() {
        // Two OBUs: temporal delimiter (type=2, size=0) + sequence header (type=1, size=1).
        let data = [
            0x12, 0x00, // TD: type=2, has_size=1, size=0
            0x0a, 0x01, 0x00, // SEQ_HDR: type=1, has_size=1, size=1
        ];
        assert!(validate_av1_obus(&data).is_ok());
    }

    #[test]
    fn test_validate_av1_obus_no_size_field() {
        // Single OBU without size field — extends to end of data.
        let data = [0x08, 0xAA, 0xBB]; // type=1, has_size=0
        assert!(validate_av1_obus(&data).is_ok());
    }

    #[test]
    fn test_read_leb128_basic() {
        assert_eq!(read_leb128(&[0x00]).unwrap(), (0, 1));
        assert_eq!(read_leb128(&[0x7F]).unwrap(), (127, 1));
        assert_eq!(read_leb128(&[0x80, 0x01]).unwrap(), (128, 2));
    }

    #[test]
    fn test_read_leb128_truncated() {
        assert!(read_leb128(&[0x80]).is_err()); // continuation bit set, no next byte
    }
}
