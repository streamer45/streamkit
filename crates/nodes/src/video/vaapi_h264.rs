// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! VA-API HW-accelerated H.264 encoder and decoder nodes.
//!
//! Uses the [`cros-codecs`](https://crates.io/crates/cros-codecs) crate which
//! provides high-level VA-API H.264 codec abstractions on Linux.  The cros-codecs
//! `StatelessDecoder` and `StatelessEncoder` handle all H.264 bitstream parsing
//! and VA-API parameter buffer construction internally — this module manages
//! frame I/O and integrates with StreamKit's pipeline architecture.
//!
//! # Nodes
//!
//! - [`VaapiH264DecoderNode`] — decodes H.264 NAL packets to NV12 [`VideoFrame`]s
//! - [`VaapiH264EncoderNode`] — encodes NV12/I420 [`VideoFrame`]s to H.264 packets
//!
//! Both perform runtime capability detection: if no VA-API device is found (or
//! H.264 is not supported), the codec task returns an error so the pipeline can
//! fall back to a CPU codec (OpenH264).
//!
//! # Feature gate
//!
//! Requires `vaapi` Cargo feature and `libva-dev` + `libgbm-dev` system packages.
//!
//! # Platform support
//!
//! - **Intel**: H.264 encode + decode on all modern Intel GPUs (Sandy Bridge+).
//! - **AMD**: H.264 encode + decode via Mesa RadeonSI VA-API.
//! - **NVIDIA**: Decode only via community `nvidia-vaapi-driver` (no VA-API encoding).

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    EncodedVideoFormat, Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat,
    VideoCodec, VideoFrame,
};
use streamkit_core::{
    config_helpers, get_codec_channel_capacity, packet_helpers, state_helpers, InputPin,
    NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

// cros-codecs high-level APIs.
use cros_codecs::backend::vaapi::decoder::VaapiBackend as VaapiDecBackend;
use cros_codecs::codec::h264::parser::Level as H264Level;
use cros_codecs::codec::h264::parser::Profile as H264Profile;
use cros_codecs::decoder::stateless::h264::H264;
use cros_codecs::decoder::stateless::{DecodeError, StatelessDecoder, StatelessVideoDecoder};
use cros_codecs::decoder::{BlockingMode, DecodedHandle, DecoderEvent};
use cros_codecs::encoder::h264::EncoderConfig as CrosH264EncoderConfig;
use cros_codecs::encoder::stateless::StatelessEncoder;
use cros_codecs::encoder::{
    FrameMetadata as CrosFrameMetadata, PredictionStructure, RateControl, Tunings, VideoEncoder,
};
use cros_codecs::libva;
use cros_codecs::video_frame::gbm_video_frame::{GbmDevice, GbmUsage, GbmVideoFrame};
use cros_codecs::video_frame::{ReadMapping, VideoFrame as CrosVideoFrame};
use cros_codecs::{FrameLayout, PlaneLayout, Resolution as CrosResolution};

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};
use super::HwAccelMode;
use super::H264_CONTENT_TYPE;

// Re-use helpers from the VA-API AV1 module — they are codec-agnostic NV12
// I/O routines (VA surface upload, GBM mapping, render-device detection, etc.).
use super::vaapi_av1::{
    align_up_u32, nv12_fourcc, open_va_and_gbm, open_va_display, read_nv12_from_mapping,
    write_nv12_to_va_surface,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// H.264 macroblock size — coded resolution must be aligned to this.
const H264_MB_SIZE: u32 = 16;

/// Maximum number of consecutive retries when the decoder returns
/// `CheckEvents` or `NotEnoughOutputBuffers` without making progress.
const MAX_EAGAIN_EMPTY_RETRIES: u32 = 1000;

/// After this many retries, switch from `thread::yield_now()` to
/// `thread::sleep(1ms)` to avoid a tight spin-loop.
const EAGAIN_YIELD_THRESHOLD: u32 = 10;

/// Default constant-quality parameter for H.264 (0–51 QP scale).
const DEFAULT_QUALITY: u32 = 26;

/// Default framerate for rate-control hints.
const DEFAULT_FRAMERATE: u32 = 30;

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the VA-API H.264 hardware decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct VaapiH264DecoderConfig {
    /// Path to the DRM render device (e.g. `/dev/dri/renderD128`).
    /// When `None`, auto-detects the first VA-API capable device.
    pub render_device: Option<String>,

    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
}

impl Default for VaapiH264DecoderConfig {
    fn default() -> Self {
        Self { render_device: None, hw_accel: HwAccelMode::Auto }
    }
}

pub struct VaapiH264DecoderNode {
    config: VaapiH264DecoderConfig,
}

impl VaapiH264DecoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub fn new(config: VaapiH264DecoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VaapiH264DecoderNode only supports hardware decoding; \
                 use video::h264::decoder for CPU decode"
                    .into(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VaapiH264DecoderNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                codec: VideoCodec::H264,
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

        tracing::info!("VaapiH264DecoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("vaapi_h264_decoder_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("vaapi_h264_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let render_device = self.config.render_device.clone();
        let decode_task = tokio::task::spawn_blocking(move || {
            vaapi_h264_decode_loop(
                render_device.as_ref(),
                decode_rx,
                &result_tx,
                &decode_duration_histogram,
            );
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
                                "VaapiH264DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("VaapiH264DecoderNode input stream closed");
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
            "VaapiH264DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("VaapiH264DecoderNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Decoder — blocking decode loop
// ---------------------------------------------------------------------------

/// Blocking decode loop running inside `spawn_blocking`.
///
/// Creates the VA-API display, GBM device, and cros-codecs `StatelessDecoder`,
/// then processes input packets until the channel is closed.
fn vaapi_h264_decode_loop(
    render_device: Option<&String>,
    mut decode_rx: mpsc::Receiver<(Bytes, Option<PacketMetadata>)>,
    result_tx: &mpsc::Sender<Result<VideoFrame, String>>,
    duration_histogram: &opentelemetry::metrics::Histogram<f64>,
) {
    // ── Open GBM device + VA display ──────────────────────────────────
    let (display, gbm, path) = match open_va_and_gbm(render_device) {
        Ok(v) => v,
        Err(e) => {
            let _ = result_tx.blocking_send(Err(e));
            return;
        },
    };
    tracing::info!(device = %path, "VA-API H.264 decoder opened display");

    // ── Create stateless decoder ─────────────────────────────────────
    let mut decoder = match StatelessDecoder::<H264, VaapiDecBackend<GbmVideoFrame>>::new_vaapi(
        display,
        BlockingMode::Blocking,
    ) {
        Ok(d) => d,
        Err(e) => {
            let _ =
                result_tx.blocking_send(Err(format!("failed to create VA-API H.264 decoder: {e}")));
            return;
        },
    };

    // Stream resolution — updated on FormatChanged events.
    let mut coded_width: u32 = 0;
    let mut coded_height: u32 = 0;

    while let Some((data, metadata)) = decode_rx.blocking_recv() {
        if result_tx.is_closed() {
            return;
        }

        let decode_start = Instant::now();
        let timestamp = metadata.as_ref().and_then(|m| m.timestamp_us).unwrap_or(0);

        // Feed bitstream to the decoder.
        let mut offset = 0usize;
        let bitstream = data.as_ref();
        let mut eagain_empty_retries: u32 = 0;

        while offset < bitstream.len() {
            let gbm_ref = Arc::clone(&gbm);
            let cw = coded_width;
            let ch = coded_height;
            let mut alloc_cb = move || {
                let res = CrosResolution { width: cw, height: ch };
                gbm_ref.clone().new_frame(nv12_fourcc(), res.clone(), res, GbmUsage::Decode).ok()
            };

            let mut made_progress = false;

            match decoder.decode(timestamp, &bitstream[offset..], &mut alloc_cb) {
                Ok(bytes_consumed) => {
                    offset += bytes_consumed;
                    made_progress = true;
                },
                Err(DecodeError::CheckEvents | DecodeError::NotEnoughOutputBuffers(_)) => {
                    // Process pending events / drain ready frames, then retry.
                },
                Err(e) => {
                    tracing::error!(error = %e, "VA-API H.264 decode error");
                    let _ = result_tx.blocking_send(Err(format!("VA-API H.264 decode error: {e}")));
                    break;
                },
            }

            // Process all pending events (format changes + ready frames).
            let (should_exit, had_events) = drain_decoder_events(
                &mut decoder,
                result_tx,
                metadata.as_ref(),
                &mut coded_width,
                &mut coded_height,
            );
            if should_exit {
                return;
            }

            if made_progress || had_events {
                eagain_empty_retries = 0;
            } else {
                eagain_empty_retries += 1;
                if eagain_empty_retries > MAX_EAGAIN_EMPTY_RETRIES {
                    tracing::error!(
                        "VA-API H.264 decoder stuck: no progress after {MAX_EAGAIN_EMPTY_RETRIES} retries"
                    );
                    let _ = result_tx.blocking_send(Err(
                        "VA-API H.264 decoder stuck in CheckEvents/NotEnoughOutputBuffers loop"
                            .to_string(),
                    ));
                    break;
                }
                // Progressive backoff to avoid a tight spin-loop.
                if eagain_empty_retries <= EAGAIN_YIELD_THRESHOLD {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }

        duration_histogram.record(decode_start.elapsed().as_secs_f64(), &[]);
    }

    // Flush remaining frames from the decoder.
    if result_tx.is_closed() {
        return;
    }
    if let Err(e) = decoder.flush() {
        tracing::warn!(error = %e, "VA-API H.264 decoder flush failed");
    }
    drain_decoder_events(&mut decoder, result_tx, None, &mut coded_width, &mut coded_height);
}

/// Drain all pending events from the decoder.
///
/// Returns `(should_exit, had_events)`:
/// - `should_exit`: the result channel is closed and the caller should return.
/// - `had_events`: at least one event (format change or frame) was processed.
fn drain_decoder_events(
    decoder: &mut StatelessDecoder<H264, VaapiDecBackend<GbmVideoFrame>>,
    result_tx: &mpsc::Sender<Result<VideoFrame, String>>,
    metadata: Option<&PacketMetadata>,
    coded_width: &mut u32,
    coded_height: &mut u32,
) -> (bool, bool) {
    let mut had_events = false;
    while let Some(event) = decoder.next_event() {
        had_events = true;
        match event {
            DecoderEvent::FormatChanged => {
                if let Some(info) = decoder.stream_info() {
                    let dw = info.display_resolution.width;
                    let dh = info.display_resolution.height;
                    *coded_width = info.coded_resolution.width;
                    *coded_height = info.coded_resolution.height;
                    tracing::info!(
                        display_width = dw,
                        display_height = dh,
                        coded_width = *coded_width,
                        coded_height = *coded_height,
                        "VA-API H.264 decoder stream format changed"
                    );
                }
            },
            DecoderEvent::FrameReady(handle) => {
                if let Err(e) = handle.sync() {
                    tracing::error!(error = %e, "VA-API H.264 frame sync failed");
                    continue;
                }

                let display_res = handle.display_resolution();
                let frame_w = display_res.width;
                let frame_h = display_res.height;

                let gbm_frame = handle.video_frame();
                let pitches = gbm_frame.get_plane_pitch();

                // Extract NV12 data while the mapping is alive.
                let nv12_data = {
                    let mapping = match gbm_frame.map() {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::error!(error = %e, "failed to map decoded GBM frame");
                            continue;
                        },
                    };
                    read_nv12_from_mapping(mapping.as_ref(), frame_w, frame_h, &pitches)
                };

                match VideoFrame::with_metadata(
                    frame_w,
                    frame_h,
                    PixelFormat::Nv12,
                    nv12_data,
                    metadata.cloned(),
                ) {
                    Ok(frame) => {
                        if result_tx.blocking_send(Ok(frame)).is_err() {
                            return (true, had_events);
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "failed to construct VideoFrame from decoded data"
                        );
                    },
                }
            },
        }
    }
    (false, had_events)
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Configuration for the VA-API H.264 hardware encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct VaapiH264EncoderConfig {
    /// Path to the DRM render device (e.g. `/dev/dri/renderD128`).
    /// When `None`, auto-detects the first VA-API capable device.
    pub render_device: Option<String>,

    /// Constant quality parameter (QP).  Lower values produce higher quality
    /// at the cost of larger bitstream.  H.264 QP range is 0–51, default 26.
    pub quality: u32,

    /// Target framerate in frames per second (used for rate control hints).
    pub framerate: u32,

    /// Use low-power encoding mode if the driver supports it.
    /// Low-power mode uses the GPU's fixed-function encoder (if available)
    /// rather than shader-based encoding, typically offering lower latency
    /// at reduced quality flexibility.
    pub low_power: bool,

    /// Hardware acceleration mode.
    pub hw_accel: HwAccelMode,
}

impl Default for VaapiH264EncoderConfig {
    fn default() -> Self {
        Self {
            render_device: None,
            quality: DEFAULT_QUALITY,
            framerate: DEFAULT_FRAMERATE,
            low_power: false,
            hw_accel: HwAccelMode::Auto,
        }
    }
}

pub struct VaapiH264EncoderNode {
    config: VaapiH264EncoderConfig,
}

impl VaapiH264EncoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub fn new(config: VaapiH264EncoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VaapiH264EncoderNode only supports hardware encoding; \
                 use video::h264::encoder for CPU encode"
                    .into(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VaapiH264EncoderNode {
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
                codec: VideoCodec::H264,
                bitstream_format: None,
                codec_private: None,
                profile: None,
                level: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    fn content_type(&self) -> Option<String> {
        Some(H264_CONTENT_TYPE.to_string())
    }

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for VaapiH264EncoderNode {
    const CONTENT_TYPE: &'static str = H264_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "VaapiH264EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "vaapi_h264_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "vaapi_h264_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<VaapiH264Encoder>(
            self.config,
            encode_rx,
            result_tx,
            duration_histogram,
        )
    }
}

// ---------------------------------------------------------------------------
// Encoder — internal codec wrapper
// ---------------------------------------------------------------------------

/// Type alias for the VA-API H.264 encoder using direct VA surfaces.
///
/// Bypasses GBM buffer allocation entirely — input frames are uploaded to
/// VA surfaces via the VA-API Image API and passed straight through to the
/// encoder backend.  This avoids the `GBM_BO_USE_HW_VIDEO_ENCODER` flag
/// which Mesa's iris driver does not support for NV12 on some hardware
/// (e.g. Intel Tiger Lake with Mesa 23.x).
type CrosVaapiH264Encoder = StatelessEncoder<
    cros_codecs::encoder::h264::H264,
    libva::Surface<()>,
    cros_codecs::backend::vaapi::encoder::VaapiBackend<(), libva::Surface<()>>,
>;

/// Internal encoder state wrapping the cros-codecs `StatelessEncoder`.
///
/// `!Send` due to internal `Rc<libva::Display>` — lives entirely inside
/// a `spawn_blocking` thread.
struct VaapiH264Encoder {
    encoder: CrosVaapiH264Encoder,
    display: Rc<libva::Display>,
    width: u32,
    height: u32,
    coded_width: u32,
    coded_height: u32,
    frame_count: u64,
}

impl StandardVideoEncoder for VaapiH264Encoder {
    type Config = VaapiH264EncoderConfig;
    const CODEC_NAME: &'static str = "VA-API H.264";

    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String> {
        let (display, path) = open_va_display(config.render_device.as_ref())?;
        tracing::info!(device = %path, width, height, "VA-API H.264 encoder opening");

        let coded_width = align_up_u32(width, H264_MB_SIZE);
        let coded_height = align_up_u32(height, H264_MB_SIZE);

        // Auto-detect the correct entrypoint.  Modern Intel GPUs (Gen 9+ /
        // Skylake onwards) only expose the low-power fixed-function encoder
        // (`VAEntrypointEncSliceLP`), while older hardware and some AMD
        // drivers use `VAEntrypointEncSlice`.  Query the driver and pick
        // whichever is available, preferring the config value when set.
        let low_power = {
            use libva::VAEntrypoint::{VAEntrypointEncSlice, VAEntrypointEncSliceLP};
            use libva::VAProfile::VAProfileH264Main;

            let entrypoints = display
                .query_config_entrypoints(VAProfileH264Main)
                .map_err(|e| format!("failed to query H.264 entrypoints: {e}"))?;

            let has_lp = entrypoints.contains(&VAEntrypointEncSliceLP);
            let has_full = entrypoints.contains(&VAEntrypointEncSlice);

            if !has_lp && !has_full {
                return Err(
                    "VA-API driver does not support H.264 encoding (no EncSlice entrypoint)".into(),
                );
            }

            // Prefer the user's explicit config; otherwise auto-detect.
            if config.low_power {
                if !has_lp {
                    return Err(
                        "low_power=true requested but VAEntrypointEncSliceLP is not supported"
                            .into(),
                    );
                }
                true
            } else if has_lp && !has_full {
                // Driver only supports low-power (common on modern Intel).
                tracing::info!("auto-selecting low-power H.264 encoder (VAEntrypointEncSliceLP)");
                true
            } else {
                false
            }
        };

        // Pass the display resolution (not the macroblock-aligned coded
        // resolution) so SpsBuilder::resolution() computes frame_crop offsets
        // automatically, preventing visible padding bars (fixes #292).
        let cros_config = CrosH264EncoderConfig {
            resolution: CrosResolution { width, height },
            profile: H264Profile::Main,
            level: H264Level::L4,
            pred_structure: PredictionStructure::LowDelay { limit: 1024 },
            initial_tunings: Tunings {
                rate_control: RateControl::ConstantQuality(config.quality),
                framerate: config.framerate,
                min_quality: 0,
                max_quality: 51,
            },
        };

        let encoder = CrosVaapiH264Encoder::new_vaapi(
            Rc::clone(&display),
            cros_config,
            nv12_fourcc(),
            CrosResolution { width: coded_width, height: coded_height },
            low_power,
            BlockingMode::Blocking,
        )
        .map_err(|e| format!("failed to create VA-API H.264 encoder: {e}"))?;

        tracing::info!(
            device = %path,
            width,
            height,
            coded_width,
            coded_height,
            quality = config.quality,
            "VA-API H.264 encoder created"
        );

        Ok(Self { encoder, display, width, height, coded_width, coded_height, frame_count: 0 })
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        if frame.pixel_format == PixelFormat::Rgba8 {
            return Err("VA-API H.264 encoder requires NV12 or I420 input; \
                 insert a video::pixel_convert node upstream"
                .into());
        }

        // Create a VA surface and upload NV12 data via the Image API.
        // This bypasses GBM buffer allocation (GBM_BO_USE_HW_VIDEO_ENCODER),
        // which Mesa's iris driver does not support for NV12 on all hardware.
        let nv12_fourcc_val: u32 = nv12_fourcc().into();
        let mut surfaces = self
            .display
            .create_surfaces(
                libva::VA_RT_FORMAT_YUV420,
                Some(nv12_fourcc_val),
                self.coded_width,
                self.coded_height,
                Some(libva::UsageHint::USAGE_HINT_ENCODER),
                vec![()],
            )
            .map_err(|e| format!("failed to create VA surface for encoding: {e}"))?;
        let surface =
            surfaces.pop().ok_or_else(|| "create_surfaces returned empty vec".to_string())?;

        // Write frame data into the VA surface.
        let (pitches, offsets) = write_nv12_to_va_surface(&self.display, &surface, frame)?;

        let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);
        let timestamp = metadata.as_ref().and_then(|m| m.timestamp_us).unwrap_or(self.frame_count);

        let frame_layout = FrameLayout {
            format: (nv12_fourcc(), 0), // DRM_FORMAT_MOD_LINEAR
            size: CrosResolution { width: self.coded_width, height: self.coded_height },
            planes: vec![
                PlaneLayout { buffer_index: 0, offset: offsets[0], stride: pitches[0] },
                PlaneLayout { buffer_index: 0, offset: offsets[1], stride: pitches[1] },
            ],
        };

        let cros_meta =
            CrosFrameMetadata { timestamp, layout: frame_layout, force_keyframe: is_keyframe };

        self.encoder
            .encode(cros_meta, surface)
            .map_err(|e| format!("VA-API H.264 encode error: {e}"))?;

        self.frame_count += 1;

        // Poll for all available encoded output.
        let mut packets = Vec::new();
        loop {
            match self.encoder.poll() {
                Ok(Some(coded)) => {
                    let out_meta = merge_h264_keyframe_metadata(metadata.clone(), &coded.bitstream);
                    packets.push(EncodedPacket {
                        data: Bytes::from(coded.bitstream),
                        metadata: out_meta,
                    });
                },
                Ok(None) => break,
                Err(e) => return Err(format!("VA-API H.264 encoder poll error: {e}")),
            }
        }

        Ok(packets)
    }

    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String> {
        self.encoder.drain().map_err(|e| format!("VA-API H.264 encoder drain error: {e}"))?;

        let mut packets = Vec::new();
        loop {
            match self.encoder.poll() {
                Ok(Some(coded)) => {
                    let out_meta = merge_h264_keyframe_metadata(None, &coded.bitstream);
                    packets.push(EncodedPacket {
                        data: Bytes::from(coded.bitstream),
                        metadata: out_meta,
                    });
                },
                Ok(None) => break,
                Err(e) => return Err(format!("VA-API H.264 encoder poll error: {e}")),
            }
        }

        Ok(packets)
    }

    fn flush_on_dimension_change() -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Keyframe detection
// ---------------------------------------------------------------------------

/// Detect whether an H.264 Annex B bitstream contains an IDR (keyframe)
/// NAL unit.
///
/// Scans for Annex B start codes (`00 00 01` or `00 00 00 01`) and checks
/// whether any NAL unit has `nal_unit_type == 5` (IDR slice).  This is the
/// standard way to identify keyframes in H.264 elementary streams.
fn h264_bitstream_is_idr(bitstream: &[u8]) -> bool {
    let len = bitstream.len();
    let mut i = 0;
    while i + 2 < len {
        // Look for 3-byte start code 00 00 01.
        if bitstream[i] == 0 && bitstream[i + 1] == 0 && bitstream[i + 2] == 1 {
            let nal_pos = i + 3;
            if nal_pos < len {
                let nal_type = bitstream[nal_pos] & 0x1F;
                if nal_type == 5 {
                    return true; // IDR slice
                }
            }
            i = nal_pos;
        // Also handle 4-byte start code 00 00 00 01.
        } else if i + 3 < len
            && bitstream[i] == 0
            && bitstream[i + 1] == 0
            && bitstream[i + 2] == 0
            && bitstream[i + 3] == 1
        {
            let nal_pos = i + 4;
            if nal_pos < len {
                let nal_type = bitstream[nal_pos] & 0x1F;
                if nal_type == 5 {
                    return true; // IDR slice
                }
            }
            i = nal_pos;
        } else {
            i += 1;
        }
    }
    false
}

/// Build output metadata with the keyframe flag set from encoder output.
///
/// Uses bitstream-level detection because cros-codecs'
/// `CodedBitstreamBuffer.metadata.force_keyframe` only reflects the
/// *caller's* request — not encoder-initiated periodic keyframes from
/// the `LowDelay` prediction structure.
fn merge_h264_keyframe_metadata(
    metadata: Option<PacketMetadata>,
    bitstream: &[u8],
) -> Option<PacketMetadata> {
    let is_keyframe = h264_bitstream_is_idr(bitstream);
    Some(match metadata {
        Some(mut m) => {
            m.keyframe = Some(is_keyframe);
            m
        },
        None => PacketMetadata {
            timestamp_us: None,
            duration_us: None,
            sequence: None,
            keyframe: Some(is_keyframe),
        },
    })
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_vaapi_h264_nodes(registry: &mut NodeRegistry) {
    let default_decoder = VaapiH264DecoderNode::new(VaapiH264DecoderConfig::default())
        .expect("default VA-API H.264 decoder config should be valid");
    registry.register_static_with_description(
        "video::vaapi::h264_decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VaapiH264DecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VaapiH264DecoderConfig))
            .expect("VaapiH264DecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "h264".to_string(),
            "hw".to_string(),
            "vaapi".to_string(),
        ],
        false,
        "Decodes H.264-compressed packets into raw NV12 video frames using VA-API \
         hardware acceleration. Requires a VA-API capable GPU (Intel Sandy Bridge+, \
         AMD, or NVIDIA with nvidia-vaapi-driver).",
    );

    let default_encoder = VaapiH264EncoderNode::new(VaapiH264EncoderConfig::default())
        .expect("default VA-API H.264 encoder config should be valid");
    registry.register_static_with_description(
        "video::vaapi::h264_encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VaapiH264EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VaapiH264EncoderConfig))
            .expect("VaapiH264EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "h264".to_string(),
            "hw".to_string(),
            "vaapi".to_string(),
        ],
        false,
        "Encodes raw NV12/I420 video frames into H.264-compressed packets using VA-API \
         hardware acceleration. Uses constant-quality (CQP) rate control. Requires a \
         VA-API capable GPU with H.264 encode support (Intel, AMD).",
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]
mod tests {
    use super::*;

    // ── Unit tests (no GPU required) ─────────────────────────────────

    #[test]
    fn test_force_cpu_rejected_decoder() {
        let config =
            VaapiH264DecoderConfig { hw_accel: HwAccelMode::ForceCpu, ..Default::default() };
        let result = VaapiH264DecoderNode::new(config);
        assert!(result.is_err(), "ForceCpu should be rejected for VA-API H.264 decoder");
    }

    #[test]
    fn test_force_cpu_rejected_encoder() {
        let config =
            VaapiH264EncoderConfig { hw_accel: HwAccelMode::ForceCpu, ..Default::default() };
        let result = VaapiH264EncoderNode::new(config);
        assert!(result.is_err(), "ForceCpu should be rejected for VA-API H.264 encoder");
    }

    #[test]
    fn test_default_configs() {
        let dec = VaapiH264DecoderConfig::default();
        assert!(dec.render_device.is_none());
        assert!(matches!(dec.hw_accel, HwAccelMode::Auto));

        let enc = VaapiH264EncoderConfig::default();
        assert!(enc.render_device.is_none());
        assert_eq!(enc.quality, DEFAULT_QUALITY);
        assert_eq!(enc.framerate, DEFAULT_FRAMERATE);
        assert!(!enc.low_power);
        assert!(matches!(enc.hw_accel, HwAccelMode::Auto));
    }

    #[test]
    fn test_decoder_pins() {
        let node = VaapiH264DecoderNode::new(VaapiH264DecoderConfig::default()).unwrap();
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.output_pins().len(), 1);
        assert_eq!(node.input_pins()[0].name, "in");
        assert_eq!(node.output_pins()[0].name, "out");
    }

    #[test]
    fn test_encoder_pins() {
        let node = VaapiH264EncoderNode::new(VaapiH264EncoderConfig::default()).unwrap();
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.output_pins().len(), 1);
        assert_eq!(node.input_pins()[0].name, "in");
        assert_eq!(node.output_pins()[0].name, "out");
        // Encoder should accept both I420 and NV12 inputs.
        assert_eq!(node.input_pins()[0].accepts_types.len(), 2);
    }

    #[test]
    fn test_encoder_content_type() {
        let node = VaapiH264EncoderNode::new(VaapiH264EncoderConfig::default()).unwrap();
        assert_eq!(node.content_type(), Some(H264_CONTENT_TYPE.to_string()));
    }

    // ── Registration test ────────────────────────────────────────────

    #[test]
    fn test_registration() {
        let mut registry = NodeRegistry::new();
        register_vaapi_h264_nodes(&mut registry);
        assert!(
            registry.create_node("video::vaapi::h264_decoder", None).is_ok(),
            "VA-API H.264 decoder should be registered"
        );
        assert!(
            registry.create_node("video::vaapi::h264_encoder", None).is_ok(),
            "VA-API H.264 encoder should be registered"
        );
    }

    // ── GPU integration tests ────────────────────────────────────────
    //
    // These require a VA-API capable GPU with H.264 support.  They are
    // compiled with the `vaapi` feature but skip at runtime if no VA-API
    // device is available.

    /// Check whether a usable VA-API display can be opened.
    fn vaapi_available() -> bool {
        use super::super::vaapi_av1::resolve_render_device;
        let path = resolve_render_device(None);
        libva::Display::open_drm_display(std::path::Path::new(&path)).is_ok()
    }

    /// Check whether the VA-API driver supports H.264 *encoding*.
    ///
    /// NVIDIA's community `nvidia-vaapi-driver` only supports decode, so
    /// encode tests must be skipped on NVIDIA GPUs to avoid false failures.
    fn vaapi_h264_encode_available() -> bool {
        if !vaapi_available() {
            return false;
        }
        VaapiH264Encoder::new_encoder(64, 64, &VaapiH264EncoderConfig::default()).is_ok()
    }

    /// Encoder + Decoder roundtrip: encode 5 NV12 frames, decode them back,
    /// verify dimensions and pixel format.
    #[tokio::test]
    async fn test_vaapi_h264_encode_decode_roundtrip() {
        if !vaapi_h264_encode_available() {
            eprintln!("SKIP: no VA-API H.264 encode support available");
            return;
        }

        use crate::test_utils::{
            assert_state_initializing, assert_state_running, assert_state_stopped,
            create_test_context, create_test_video_frame,
        };
        use std::borrow::Cow;
        use std::collections::HashMap;

        // --- Encode ---
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = VaapiH264EncoderConfig {
            render_device: None,
            hw_accel: HwAccelMode::Auto,
            quality: 40, // fast, lower quality for test speed
            framerate: 30,
            low_power: false,
        };
        let encoder = VaapiH264EncoderNode::new(encoder_config).unwrap();
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..5 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(1_000 + 33_333 * index),
                duration_us: Some(33_333),
                sequence: Some(index),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(!encoded_packets.is_empty(), "VA-API H.264 encoder produced no packets");

        // --- Decode ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = VaapiH264DecoderNode::new(VaapiH264DecoderConfig::default()).unwrap();
        let dec_handle = tokio::spawn(async move { Box::new(decoder).run(dec_context).await });

        assert_state_initializing(&mut dec_state_rx).await;
        assert_state_running(&mut dec_state_rx).await;

        for packet in encoded_packets {
            if let Packet::Binary { data, metadata, .. } = packet {
                dec_input_tx
                    .send(Packet::Binary {
                        data,
                        content_type: Some(Cow::Borrowed(H264_CONTENT_TYPE)),
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
        assert!(!decoded_packets.is_empty(), "VA-API H.264 decoder produced no frames");

        for packet in decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");
                },
                _ => panic!("Expected Video packet from VA-API H.264 decoder"),
            }
        }
    }

    // ── Keyframe detection unit tests ────────────────────────────────

    #[test]
    fn test_h264_idr_detection_with_4byte_start_code() {
        // Annex B bitstream with 4-byte start code + IDR NAL (type 5).
        // NAL header byte: forbidden_zero_bit(1)=0, nal_ref_idc(2)=3, nal_type(5)=5
        // → 0b_0_11_00101 = 0x65
        let bitstream: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x01, 0x65, // IDR slice NAL
            0xAA, 0xBB, // dummy slice data
        ];
        assert!(h264_bitstream_is_idr(&bitstream));
    }

    #[test]
    fn test_h264_idr_detection_with_3byte_start_code() {
        // Annex B bitstream with 3-byte start code + IDR NAL (type 5).
        let bitstream: Vec<u8> = vec![
            0x00, 0x00, 0x01, 0x65, // IDR slice NAL
            0xAA, // dummy data
        ];
        assert!(h264_bitstream_is_idr(&bitstream));
    }

    #[test]
    fn test_h264_non_idr_detection() {
        // Non-IDR slice NAL (type 1).
        // NAL header: nal_ref_idc=2, nal_type=1 → 0b_0_10_00001 = 0x41
        let bitstream: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x01, 0x41, // Non-IDR slice NAL
            0xCC,
        ];
        assert!(!h264_bitstream_is_idr(&bitstream));
    }

    #[test]
    fn test_h264_idr_with_sps_pps_prefix() {
        // Typical encoder output: SPS (type 7) + PPS (type 8) + IDR (type 5).
        let bitstream: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x01, 0x67, // SPS NAL (type 7)
            0x42, 0x00, 0x1E, // dummy SPS data
            0x00, 0x00, 0x00, 0x01, 0x68, // PPS NAL (type 8)
            0xCE, 0x38, 0x80, // dummy PPS data
            0x00, 0x00, 0x00, 0x01, 0x65, // IDR slice NAL (type 5)
            0x88, 0x80, // dummy slice data
        ];
        assert!(h264_bitstream_is_idr(&bitstream));
    }

    #[test]
    fn test_h264_idr_detection_empty() {
        assert!(!h264_bitstream_is_idr(&[]));
    }

    #[test]
    fn test_merge_h264_keyframe_metadata_with_existing() {
        let meta = PacketMetadata {
            timestamp_us: Some(100_000),
            duration_us: Some(33_333),
            sequence: Some(3),
            keyframe: None,
        };
        // IDR bitstream
        let bitstream: Vec<u8> = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0xAA];
        let result = merge_h264_keyframe_metadata(Some(meta), &bitstream).unwrap();
        assert_eq!(result.keyframe, Some(true));
        assert_eq!(result.timestamp_us, Some(100_000));
        assert_eq!(result.sequence, Some(3));
    }

    #[test]
    fn test_merge_h264_keyframe_metadata_without_existing() {
        // Non-IDR bitstream
        let bitstream: Vec<u8> = vec![0x00, 0x00, 0x00, 0x01, 0x41, 0xBB];
        let result = merge_h264_keyframe_metadata(None, &bitstream).unwrap();
        assert_eq!(result.keyframe, Some(false));
        assert!(result.timestamp_us.is_none());
    }
}
