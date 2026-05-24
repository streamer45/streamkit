// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! VP9 video codec nodes (CPU).

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::Deserialize;
use std::ffi::CStr;
use std::sync::Arc;
use std::time::Instant;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    EncodedVideoFormat, Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat,
    VideoCodec, VideoFrame, VideoLayout,
};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, PooledVideoData, ProcessorNode,
    StreamKitError, VideoFramePool,
};
use tokio::sync::mpsc;
use vpx::vp8e_enc_control_id::{VP8E_SET_CPUUSED, VP8E_SET_ENABLEAUTOALTREF};
use vpx::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT;
use vpx::vpx_img_fmt::{VPX_IMG_FMT_I420, VPX_IMG_FMT_NV12};
use vpx::vpx_kf_mode::VPX_KF_AUTO;
use vpx_sys as vpx;

const VP9_TIMEBASE_DEN: i32 = 1_000_000;
use super::VP9_CONTENT_TYPE;

// libvpx ABI values are macros in vpx headers; libvpx-sys doesn't expose them.
// Values are derived from /usr/include/vpx headers (VPX_IMAGE/VPX_CODEC/VPX_ENCODER ABI).
const VPX_IMAGE_ABI_VERSION: i32 = 5;
const VPX_CODEC_ABI_VERSION: i32 = 4 + VPX_IMAGE_ABI_VERSION;
const VPX_DECODER_ABI_VERSION: i32 = 3 + VPX_CODEC_ABI_VERSION;
const VPX_EXT_RATECTRL_ABI_VERSION: i32 = 1;
const VPX_ENCODER_ABI_VERSION: i32 = 15 + VPX_CODEC_ABI_VERSION + VPX_EXT_RATECTRL_ABI_VERSION;

/// Asserts at startup that the linked libvpx exposes the VP9 encoder and decoder
/// interfaces.  This catches library version mismatches or missing codec support
/// early (at node registration) rather than at the first encode/decode attempt.
///
/// The check verifies that `vpx_codec_vp9_cx()` and `vpx_codec_vp9_dx()` return
/// non-null pointers and that their `iface_name` strings contain "VP9".
fn assert_vpx_abi_versions() {
    // SAFETY: `vpx_codec_vp9_cx()` returns a pointer to a static
    // `vpx_codec_iface_t`.  It is safe to pass to `vpx_codec_iface_name`.
    let cx_iface = unsafe { vpx::vpx_codec_vp9_cx() };
    assert!(!cx_iface.is_null(), "vpx_codec_vp9_cx() returned null — is libvpx built with VP9?");

    // SAFETY: `vpx_codec_iface_name` accepts a non-null iface pointer and
    // returns a static C string.
    let cx_name = unsafe { CStr::from_ptr(vpx::vpx_codec_iface_name(cx_iface)) };
    let cx_name_str = cx_name.to_str().unwrap_or("<invalid UTF-8>");
    assert!(
        cx_name_str.contains("VP9"),
        "vpx_codec_vp9_cx() iface name does not contain 'VP9': {cx_name_str}"
    );

    // SAFETY: same reasoning for `vpx_codec_vp9_dx()`.
    let dx_iface = unsafe { vpx::vpx_codec_vp9_dx() };
    assert!(!dx_iface.is_null(), "vpx_codec_vp9_dx() returned null — is libvpx built with VP9?");

    let dx_name = unsafe { CStr::from_ptr(vpx::vpx_codec_iface_name(dx_iface)) };
    let dx_name_str = dx_name.to_str().unwrap_or("<invalid UTF-8>");
    assert!(
        dx_name_str.contains("VP9"),
        "vpx_codec_vp9_dx() iface name does not contain 'VP9': {dx_name_str}"
    );

    tracing::debug!("libvpx ABI check passed: encoder={cx_name_str}, decoder={dx_name_str}");
}

const VPX_EFLAG_FORCE_KF: vpx::vpx_enc_frame_flags_t = 1;
const VPX_FRAME_IS_KEY: u32 = 0x1;
const VPX_DL_BEST_QUALITY: u64 = 0;
const VPX_DL_GOOD_QUALITY: u64 = 1_000_000;
const VPX_DL_REALTIME: u64 = 1;
const VPX_CODEC_CAP_ENCODER: u32 = 0x2;

const VP9_DEFAULT_BITRATE_KBPS: u32 = 2500;
const VP9_DEFAULT_KF_INTERVAL: u32 = 120;
const VP9_DEFAULT_THREADS: u32 = 2;
const VP9_DEFAULT_CPU_USED: i32 = 6;

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Vp9DecoderConfig {
    pub threads: u32,
}

impl Default for Vp9DecoderConfig {
    fn default() -> Self {
        Self { threads: VP9_DEFAULT_THREADS }
    }
}

/// Controls the CPU time the VP9 encoder is allowed to spend per frame.
///
/// Maps to the libvpx `deadline` parameter in `vpx_codec_encode`.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Vp9EncoderDeadline {
    /// Real-time encoding – lowest latency, may sacrifice quality (VPX_DL_REALTIME).
    #[default]
    Realtime,
    /// Good quality – allows up to ~1 second per frame (VPX_DL_GOOD_QUALITY).
    GoodQuality,
    /// Best quality – unlimited time per frame (VPX_DL_BEST_QUALITY).
    BestQuality,
}

impl Vp9EncoderDeadline {
    const fn as_vpx_deadline(self) -> u64 {
        match self {
            Self::Realtime => VPX_DL_REALTIME,
            Self::GoodQuality => VPX_DL_GOOD_QUALITY,
            Self::BestQuality => VPX_DL_BEST_QUALITY,
        }
    }
}

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Vp9EncoderConfig {
    pub bitrate_kbps: u32,
    pub keyframe_interval: u32,
    pub threads: u32,
    pub deadline: Vp9EncoderDeadline,
    /// libvpx `VP8E_SET_CPUUSED` control value.  Higher values trade quality
    /// for speed.  Valid range depends on [`deadline`](Vp9EncoderDeadline):
    ///   - `realtime`: 0–9 (default 6)
    ///   - `good_quality` / `best_quality`: 0–5
    pub cpu_used: i32,
}

impl Default for Vp9EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_kbps: VP9_DEFAULT_BITRATE_KBPS,
            keyframe_interval: VP9_DEFAULT_KF_INTERVAL,
            threads: VP9_DEFAULT_THREADS,
            deadline: Vp9EncoderDeadline::default(),
            cpu_used: VP9_DEFAULT_CPU_USED,
        }
    }
}

pub struct Vp9DecoderNode {
    config: Vp9DecoderConfig,
}

impl Vp9DecoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: Vp9DecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for Vp9DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::Vp9,
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

        tracing::info!("Vp9DecoderNode starting");
        let mut input_rx = context.take_input("in")?;
        let video_pool = context.video_pool.clone();

        let meter = global::meter("skit_nodes");
        let packets_processed_counter = meter.u64_counter("vp9_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("vp9_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, mut decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let decoder_threads = self.config.threads;
        let decode_task = tokio::task::spawn_blocking(move || {
            let mut decoder = match Vp9Decoder::new(decoder_threads) {
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
                                "Vp9DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("Vp9DecoderNode input stream closed");
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
            "Vp9DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("Vp9DecoderNode finished");
        Ok(())
    }
}

pub struct Vp9EncoderNode {
    config: Vp9EncoderConfig,
}

impl Vp9EncoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: Vp9EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for Vp9EncoderNode {
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
                codec: VideoCodec::Vp9,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some(VP9_CONTENT_TYPE.to_string())
    }

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for Vp9EncoderNode {
    const CONTENT_TYPE: &'static str = VP9_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "Vp9EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "vp9_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "vp9_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<Vp9Encoder>(
            self.config,
            encode_rx,
            result_tx,
            duration_histogram,
        )
    }
}

impl StandardVideoEncoder for Vp9Encoder {
    type Config = Vp9EncoderConfig;
    const CODEC_NAME: &'static str = "VP9";

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
}

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};

struct Vp9Decoder {
    ctx: vpx::vpx_codec_ctx_t,
}

impl Vp9Decoder {
    fn new(threads: u32) -> Result<Self, String> {
        let iface = unsafe {
            // SAFETY: libvpx returns a static codec interface pointer.
            vpx::vpx_codec_vp9_dx()
        };
        if iface.is_null() {
            return Err("VP9 decoder interface not available".to_string());
        }

        let mut ctx = unsafe {
            // SAFETY: vpx_codec_ctx_t is a plain C struct that can be zero-initialized.
            std::mem::zeroed()
        };
        let cfg = vpx::vpx_codec_dec_cfg_t { threads, w: 0, h: 0 };

        let res = unsafe {
            // SAFETY: ctx and cfg are valid and iface is non-null.
            vpx::vpx_codec_dec_init_ver(
                &raw mut ctx,
                iface,
                &raw const cfg,
                0,
                VPX_DECODER_ABI_VERSION,
            )
        };
        check_vpx(res, &raw mut ctx, "VP9 decoder init")?;

        Ok(Self { ctx })
    }

    fn decode_packet(
        &mut self,
        data: &[u8],
        metadata: Option<PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        let data_len =
            u32::try_from(data.len()).map_err(|_| "VP9 packet too large for libvpx".to_string())?;
        let res = unsafe {
            // SAFETY: libvpx expects a valid buffer for the duration of the call.
            vpx::vpx_codec_decode(
                &raw mut self.ctx,
                data.as_ptr(),
                data_len,
                std::ptr::null_mut(),
                0,
            )
        };
        check_vpx(res, &raw mut self.ctx, "VP9 decode")?;

        // Most VP9 packets produce exactly one frame; pre-allocate for that
        // common case to avoid a heap allocation + realloc in the hot path.
        let mut frames = Vec::with_capacity(1);
        let mut iter: vpx::vpx_codec_iter_t = std::ptr::null_mut();
        let mut remaining_metadata = metadata;

        loop {
            let image_ptr = unsafe {
                // SAFETY: iter is managed by libvpx and image_ptr is valid until next call.
                vpx::vpx_codec_get_frame(&raw mut self.ctx, &raw mut iter)
            };
            if image_ptr.is_null() {
                break;
            }

            let image = unsafe {
                // SAFETY: image_ptr is non-null and points to a valid vpx_image_t.
                &*image_ptr
            };

            // Peek ahead: if another frame follows, clone metadata; otherwise move it.
            let next_ptr = unsafe {
                let mut peek_iter = iter;
                vpx::vpx_codec_get_frame(&raw mut self.ctx, &raw mut peek_iter)
            };
            let meta = if next_ptr.is_null() {
                remaining_metadata.take()
            } else {
                remaining_metadata.clone()
            };

            let frame = copy_vpx_image(image, meta, video_pool)?;
            frames.push(frame);
        }

        Ok(frames)
    }
}

impl Drop for Vp9Decoder {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: ctx is initialized by libvpx and must be destroyed exactly once.
            vpx::vpx_codec_destroy(&raw mut self.ctx);
        }
    }
}

struct Vp9Encoder {
    ctx: vpx::vpx_codec_ctx_t,
    next_pts: i64,
    deadline: u64,
}

impl Vp9Encoder {
    fn new(width: u32, height: u32, config: &Vp9EncoderConfig) -> Result<Self, String> {
        let iface = unsafe {
            // SAFETY: libvpx returns a static codec interface pointer.
            vpx::vpx_codec_vp9_cx()
        };
        if iface.is_null() {
            return Err("VP9 encoder interface not available".to_string());
        }
        let caps = unsafe {
            // SAFETY: iface is non-null.
            vpx::vpx_codec_get_caps(iface)
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        if (caps as u32 & VPX_CODEC_CAP_ENCODER) == 0 {
            return Err("libvpx does not expose VP9 encoder capabilities".to_string());
        }

        let mut ctx = unsafe {
            // SAFETY: vpx_codec_ctx_t is a plain C struct that can be zero-initialized.
            std::mem::zeroed()
        };
        let mut cfg = std::mem::MaybeUninit::<vpx::vpx_codec_enc_cfg_t>::uninit();
        let res = unsafe {
            // SAFETY: cfg is valid for initialization and iface is non-null.
            vpx::vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0)
        };
        check_vpx(res, std::ptr::null_mut(), "VP9 encoder config")?;
        let mut cfg = unsafe {
            // SAFETY: vpx_codec_enc_config_default initializes cfg on success.
            cfg.assume_init()
        };

        cfg.g_w = width;
        cfg.g_h = height;
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = VP9_TIMEBASE_DEN;
        cfg.rc_target_bitrate = config.bitrate_kbps.max(1);
        cfg.g_threads = config.threads.max(1);
        cfg.g_lag_in_frames = 0;
        cfg.kf_mode = VPX_KF_AUTO;
        cfg.kf_min_dist = 0;
        cfg.kf_max_dist = config.keyframe_interval.max(1);

        let res = unsafe {
            // SAFETY: ctx and cfg are valid and iface is non-null.
            vpx::vpx_codec_enc_init_ver(
                &raw mut ctx,
                iface,
                &raw const cfg,
                0,
                VPX_ENCODER_ABI_VERSION,
            )
        };
        if let Err(err) = check_vpx(res, &raw mut ctx, "VP9 encoder init") {
            let cfg_summary = format!(
                "w={width} h={height} timebase=1/{den} bitrate_kbps={} threads={} lag={} kf_max={}",
                cfg.rc_target_bitrate,
                cfg.g_threads,
                cfg.g_lag_in_frames,
                cfg.kf_max_dist,
                den = cfg.g_timebase.den
            );
            return Err(format!("{err} (cfg: {cfg_summary})"));
        }

        let max_cpu_used = match config.deadline {
            Vp9EncoderDeadline::Realtime => 9,
            Vp9EncoderDeadline::GoodQuality | Vp9EncoderDeadline::BestQuality => 5,
        };
        let cpu_used = config.cpu_used.clamp(0, max_cpu_used);
        if cpu_used != config.cpu_used {
            tracing::warn!(
                "cpu_used {} clamped to {} for {:?} deadline",
                config.cpu_used,
                cpu_used,
                config.deadline
            );
        }

        unsafe {
            // SAFETY: Control calls are valid after encoder initialization.
            set_codec_control(&raw mut ctx, VP8E_SET_ENABLEAUTOALTREF as i32, 0)?;
            set_codec_control(&raw mut ctx, VP8E_SET_CPUUSED as i32, cpu_used)?;
        }

        Ok(Self { ctx, next_pts: 0, deadline: config.deadline.as_vpx_deadline() })
    }

    fn encode_frame(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        let vpx_fmt = match frame.pixel_format {
            PixelFormat::I420 => VPX_IMG_FMT_I420,
            PixelFormat::Nv12 => VPX_IMG_FMT_NV12,
            other => {
                return Err(format!("VP9 encoder expects I420 or NV12 input, got {other:?}"));
            },
        };

        let layout = frame.layout();
        if frame.data_len() < layout.total_bytes() {
            return Err(format!(
                "VP9 encoder expected {} bytes, got {}",
                layout.total_bytes(),
                frame.data_len()
            ));
        }
        let expected_layout = VideoLayout::aligned(
            frame.width,
            frame.height,
            frame.pixel_format,
            layout.stride_align(),
        );
        if layout != expected_layout {
            return Err(format!(
                "VP9 encoder requires the canonical aligned {:?} layout",
                frame.pixel_format
            ));
        }

        let mut image = std::mem::MaybeUninit::<vpx::vpx_image_t>::uninit();
        let image_ptr = unsafe {
            // SAFETY: frame data is valid for the duration of this call.
            // `cast_mut()` is required by the C API signature, but libvpx only
            // reads from the image buffer during `vpx_codec_encode` — it never
            // writes back through this pointer.
            vpx::vpx_img_wrap(
                image.as_mut_ptr(),
                vpx_fmt,
                frame.width,
                frame.height,
                layout.stride_align(),
                frame.data.as_slice().as_ptr().cast_mut(),
            )
        };
        if image_ptr.is_null() {
            return Err(format!("Failed to wrap {:?} frame for VP9 encoder", frame.pixel_format));
        }
        let image = unsafe {
            // SAFETY: vpx_img_wrap initialized image on success.
            image.assume_init()
        };

        let (pts, duration) = self.next_pts(metadata.as_ref());
        let mut flags: vpx::vpx_enc_frame_flags_t = 0;
        if metadata.as_ref().and_then(|meta| meta.keyframe).unwrap_or(false) {
            flags |= VPX_EFLAG_FORCE_KF;
        }

        let res = unsafe {
            // SAFETY: image is initialized and ctx is ready for encode.
            vpx::vpx_codec_encode(
                &raw mut self.ctx,
                &raw const image,
                pts,
                duration,
                flags,
                self.deadline,
            )
        };
        check_vpx(res, &raw mut self.ctx, "VP9 encode")?;

        let packets = self.drain_packets(metadata);

        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, String> {
        let mut output = Vec::new();
        // With g_lag_in_frames = 0 and auto-alt-ref disabled, libvpx has no
        // buffered frames, so a single flush call suffices.  We still loop
        // as a defensive measure, but cap at 2 iterations (the first drains
        // any residual packet, the second confirms the pipeline is empty).
        for _ in 0..2 {
            let res = unsafe {
                // SAFETY: Passing a null image flushes delayed frames.
                vpx::vpx_codec_encode(&raw mut self.ctx, std::ptr::null(), 0, 0, 0, self.deadline)
            };
            check_vpx(res, &raw mut self.ctx, "VP9 encode flush")?;

            let mut packets = self.drain_packets(None);
            if packets.is_empty() {
                break;
            }
            output.append(&mut packets);
        }

        Ok(output)
    }

    fn drain_packets(&mut self, metadata: Option<PacketMetadata>) -> Vec<EncodedPacket> {
        let mut packets = Vec::new();
        let mut iter: vpx::vpx_codec_iter_t = std::ptr::null_mut();
        let mut remaining_metadata = metadata;
        loop {
            let packet_ptr = unsafe {
                // SAFETY: iter is managed by libvpx and packet_ptr is valid until next call.
                vpx::vpx_codec_get_cx_data(&raw mut self.ctx, &raw mut iter)
            };
            if packet_ptr.is_null() {
                break;
            }

            let packet = unsafe {
                // SAFETY: packet_ptr is non-null and points to a valid vpx_codec_cx_pkt_t.
                &*packet_ptr
            };

            if packet.kind != VPX_CODEC_CX_FRAME_PKT {
                continue;
            }

            let frame_pkt = unsafe {
                // SAFETY: Union access for frame packet data.
                packet.data.frame
            };

            let data: Bytes = unsafe {
                // SAFETY: frame_pkt.buf is valid for frame_pkt.sz bytes.
                // Copy into Bytes directly so the downstream Packet::Binary
                // doesn't need a second Vec → Bytes conversion.
                #[allow(clippy::cast_possible_truncation)]
                Bytes::copy_from_slice(std::slice::from_raw_parts(
                    frame_pkt.buf as *const u8,
                    frame_pkt.sz as usize,
                ))
            };

            let is_keyframe = (frame_pkt.flags as u32 & VPX_FRAME_IS_KEY) != 0;

            // Peek ahead: if another frame packet follows, clone metadata; otherwise move it.
            let next_ptr = unsafe {
                let mut peek_iter = iter;
                vpx::vpx_codec_get_cx_data(&raw mut self.ctx, &raw mut peek_iter)
            };
            let meta = if next_ptr.is_null() {
                remaining_metadata.take()
            } else {
                remaining_metadata.clone()
            };

            let output_metadata = merge_keyframe_metadata(
                meta,
                is_keyframe,
                frame_pkt.pts,
                frame_pkt.duration as u64,
            );

            packets.push(EncodedPacket { data, metadata: Some(output_metadata) });
        }

        packets
    }

    fn next_pts(&mut self, metadata: Option<&PacketMetadata>) -> (i64, u64) {
        // Default to 1µs rather than 0 so libvpx rate-control heuristics
        // always see a non-zero duration.  The PTS advance fallback already
        // uses `pts + 1`, so this keeps the two paths consistent.
        let duration = metadata.and_then(|meta| meta.duration_us).unwrap_or(1);

        let pts =
            metadata.and_then(|meta| meta.timestamp_us).map_or(self.next_pts, u64::cast_signed);

        self.next_pts = if duration > 0 { pts + duration.cast_signed() } else { pts + 1 };
        (pts, duration)
    }
}

impl Drop for Vp9Encoder {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: ctx is initialized by libvpx and must be destroyed exactly once.
            vpx::vpx_codec_destroy(&raw mut self.ctx);
        }
    }
}

fn check_vpx(
    res: vpx::vpx_codec_err_t,
    ctx: *mut vpx::vpx_codec_ctx_t,
    context: &str,
) -> Result<(), String> {
    if res == vpx::VPX_CODEC_OK {
        return Ok(());
    }

    let err = vpx_error(ctx, res);
    let detail = if ctx.is_null() {
        None
    } else {
        let detail_ptr = unsafe {
            // SAFETY: libvpx returns a NUL-terminated error detail string.
            vpx::vpx_codec_error_detail(ctx)
        };
        if detail_ptr.is_null() {
            None
        } else {
            Some(unsafe {
                // SAFETY: detail_ptr is a valid C string.
                CStr::from_ptr(detail_ptr).to_string_lossy().into_owned()
            })
        }
    };

    detail.map_or_else(
        || Err(format!("{context}: {err}")),
        |detail| Err(format!("{context}: {err} ({detail})")),
    )
}

unsafe fn set_codec_control(
    ctx: *mut vpx::vpx_codec_ctx_t,
    ctrl_id: i32,
    value: i32,
) -> Result<(), String> {
    let res = vpx::vpx_codec_control_(ctx, ctrl_id, value);
    check_vpx(res, ctx, "VP9 codec control")
}

fn vpx_error(ctx: *mut vpx::vpx_codec_ctx_t, err: vpx::vpx_codec_err_t) -> String {
    unsafe {
        // SAFETY: libvpx returns a NUL-terminated error string.
        let msg_ptr = if ctx.is_null() {
            vpx::vpx_codec_err_to_string(err)
        } else {
            vpx::vpx_codec_error(ctx)
        };
        if msg_ptr.is_null() {
            "libvpx error".to_string()
        } else {
            CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
        }
    }
}

/// Copy a decoded I420 vpx_image into an NV12 `VideoFrame`.
///
/// libvpx always decodes VP9 to I420 (three separate Y, U, V planes).
/// We convert to NV12 on the fly by copying the Y plane as-is and
/// interleaving the U and V planes into a single UV plane.
/// This is a cheap operation — just zipping two half-size planes.
fn copy_vpx_image(
    image: &vpx::vpx_image_t,
    metadata: Option<PacketMetadata>,
    video_pool: Option<&Arc<VideoFramePool>>,
) -> Result<VideoFrame, String> {
    if image.fmt != VPX_IMG_FMT_I420 {
        return Err("VP9 decoder produced non-I420 frame".to_string());
    }

    let width = image.d_w;
    let height = image.d_h;
    if width == 0 || height == 0 {
        return Err("VP9 decoder produced empty frame".to_string());
    }

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

    let y_src_ptr = image.planes[0];
    if y_src_ptr.is_null() {
        return Err("VP9 decoder returned null Y plane".to_string());
    }
    copy_plane(
        &mut data_slice[y_plane.offset..y_plane.offset + y_plane.stride * y_plane.height as usize],
        y_plane.stride,
        y_src_ptr,
        image.stride[0],
        width as usize,
        height as usize,
    )?;

    let u_src_ptr = image.planes[1];
    let v_src_ptr = image.planes[2];
    if u_src_ptr.is_null() || v_src_ptr.is_null() {
        return Err("VP9 decoder returned null chroma plane".to_string());
    }

    let chroma_w = (width as usize).div_ceil(2);
    let chroma_h = uv_plane.height as usize;

    if image.stride[1] <= 0 || image.stride[2] <= 0 {
        return Err("Invalid source stride for VP9 chroma plane".to_string());
    }

    #[allow(clippy::cast_sign_loss)]
    let u_src_stride = image.stride[1] as usize;
    #[allow(clippy::cast_sign_loss)]
    let v_src_stride = image.stride[2] as usize;

    for row in 0..chroma_h {
        let u_row = unsafe {
            // SAFETY: u_src_ptr is valid with u_src_stride bytes per row.
            std::slice::from_raw_parts(u_src_ptr.add(row * u_src_stride), chroma_w)
        };
        let v_row = unsafe {
            // SAFETY: v_src_ptr is valid with v_src_stride bytes per row.
            std::slice::from_raw_parts(v_src_ptr.add(row * v_src_stride), chroma_w)
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

fn copy_plane(
    dst: &mut [u8],
    dst_stride: usize,
    src_ptr: *const u8,
    src_stride: i32,
    width: usize,
    height: usize,
) -> Result<(), String> {
    if src_stride <= 0 {
        return Err("Invalid source stride for VP9 plane".to_string());
    }
    #[allow(clippy::cast_sign_loss)]
    let src_stride = src_stride as usize;

    for row in 0..height {
        let src_row = unsafe {
            // SAFETY: src_ptr points to a valid plane with src_stride bytes per row.
            std::slice::from_raw_parts(src_ptr.add(row * src_stride), width)
        };
        let dst_start = row * dst_stride;
        let dst_end = dst_start + width;
        if dst_end > dst.len() {
            return Err("VP9 plane copy overflow".to_string());
        }
        dst[dst_start..dst_end].copy_from_slice(src_row);
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

#[cfg(test)]
fn vp9_encoder_available() -> bool {
    let iface = unsafe { vpx::vpx_codec_vp9_cx() };
    if iface.is_null() {
        return false;
    }
    let caps = unsafe { vpx::vpx_codec_get_caps(iface) };
    u32::try_from(caps).is_ok_and(|caps_u32| (caps_u32 & VPX_CODEC_CAP_ENCODER) != 0)
}

use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)] // Default config and schema serialization should never fail
pub fn register_vp9_nodes(registry: &mut NodeRegistry) {
    assert_vpx_abi_versions();

    let default_decoder = Vp9DecoderNode::new(Vp9DecoderConfig::default())
        .expect("default VP9 decoder config should be valid");
    register_static_node!(
        registry,
        "video::vp9::decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Vp9DecoderNode::new(config)?))
        },
        Vp9DecoderConfig,
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        ["video", "codecs", "vp9"],
        "Decodes VP9-compressed packets into raw NV12 video frames. \
         Use this before CPU compositing or analysis pipelines.",
    );

    let default_encoder = Vp9EncoderNode::new(Vp9EncoderConfig::default())
        .expect("default VP9 encoder config should be valid");
    register_static_node!(
        registry, "video::vp9::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Vp9EncoderNode::new(config)?))
        },
        Vp9EncoderConfig,
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        ["video", "codecs", "vp9"],
        "Encodes raw video frames (NV12 or I420) into VP9 packets for transport or container muxing. \
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
    use std::collections::{HashMap, HashSet};
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use tokio::sync::mpsc;

    fn vpx_string(ptr: *const c_char) -> String {
        if ptr.is_null() {
            return "null".to_string();
        }
        unsafe {
            // SAFETY: libvpx returns NUL-terminated C strings.
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }

    fn dump_vpx_info() {
        let version = unsafe {
            // SAFETY: libvpx returns static string pointers.
            vpx_string(vpx::vpx_codec_version_str())
        };
        let extra = unsafe { vpx_string(vpx::vpx_codec_version_extra_str()) };
        let build = unsafe { vpx_string(vpx::vpx_codec_build_config()) };
        eprintln!("libvpx version: {version} {extra}");
        eprintln!("libvpx build config: {build}");
    }

    #[tokio::test]
    async fn test_vp9_encode_decode_roundtrip() {
        dump_vpx_info();
        if !vp9_encoder_available() {
            eprintln!("Skipping VP9 encode/decode roundtrip: encoder not available in libvpx");
            return;
        }

        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = Vp9EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 800,
            threads: 1,
            ..Default::default()
        };
        let encoder = Vp9EncoderNode::new(encoder_config.clone()).unwrap();

        // Debug probe: run a direct encode to surface libvpx details if packets are missing.
        let mut probe_encoder = Vp9Encoder::new(64, 64, &encoder_config).unwrap();
        let mut probe_frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
        probe_frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(1_000),
            duration_us: Some(33_333),
            sequence: Some(0),
            keyframe: Some(true),
        });
        match probe_encoder.encode_frame(&probe_frame, probe_frame.metadata.clone()) {
            Ok(packets) => {
                eprintln!("VP9 probe encode packets: {}", packets.len());
                if packets.is_empty() {
                    if let Ok(flushed) = probe_encoder.flush() {
                        eprintln!("VP9 probe flush packets: {}", flushed.len());
                    }
                    let detail = unsafe {
                        // SAFETY: ctx is valid for the duration of the encoder.
                        vpx_string(vpx::vpx_codec_error_detail(&raw mut probe_encoder.ctx))
                    };
                    eprintln!("VP9 probe error detail: {detail}");
                }
            },
            Err(err) => {
                eprintln!("VP9 probe encode error: {err}");
            },
        }

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        let mut expected_metadata = HashMap::new();
        for index in 0_u64..5 {
            let timestamp = 1_000 + 33_333_u64 * index;
            let duration: u64 = 33_333;
            expected_metadata.insert(index, (timestamp, duration));

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
        assert!(!encoded_packets.is_empty(), "VP9 encoder produced no packets");
        let mut encoded_sequences = HashSet::new();

        for packet in &encoded_packets {
            let Packet::Binary { metadata, .. } = packet else {
                continue;
            };
            let meta = metadata.as_ref().expect("Encoded VP9 packet missing metadata");
            let seq = meta.sequence.expect("Encoded VP9 packet missing sequence");
            let (expected_ts, expected_dur) = expected_metadata
                .get(&seq)
                .copied()
                .expect("Encoded VP9 packet has unexpected sequence");

            assert_eq!(
                meta.timestamp_us,
                Some(expected_ts),
                "Encoded VP9 packet timestamp mismatch"
            );
            assert_eq!(
                meta.duration_us,
                Some(expected_dur),
                "Encoded VP9 packet duration mismatch"
            );
            encoded_sequences.insert(seq);
        }

        assert_eq!(
            encoded_sequences.len(),
            expected_metadata.len(),
            "Encoded VP9 packets did not cover all input frames"
        );

        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = Vp9DecoderNode::new(Vp9DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            if let Packet::Binary { data, metadata, .. } = packet {
                dec_input_tx
                    .send(Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed(VP9_CONTENT_TYPE)),
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
        assert!(!decoded_packets.is_empty(), "VP9 decoder produced no frames");
        let mut decoded_sequences = HashSet::new();

        for packet in decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");

                    let meta = frame.metadata.as_ref().expect("Decoded VP9 frame missing metadata");
                    let seq = meta.sequence.expect("Decoded VP9 frame missing sequence");
                    let (expected_ts, expected_dur) = expected_metadata
                        .get(&seq)
                        .copied()
                        .expect("Decoded VP9 frame has unexpected sequence");

                    assert_eq!(
                        meta.timestamp_us,
                        Some(expected_ts),
                        "Decoded VP9 frame timestamp mismatch"
                    );
                    assert_eq!(
                        meta.duration_us,
                        Some(expected_dur),
                        "Decoded VP9 frame duration mismatch"
                    );
                    decoded_sequences.insert(seq);
                },
                _ => panic!("Expected Video packet from VP9 decoder"),
            }
        }

        assert_eq!(
            decoded_sequences.len(),
            expected_metadata.len(),
            "Decoded VP9 frames did not cover all input frames"
        );
    }
}
