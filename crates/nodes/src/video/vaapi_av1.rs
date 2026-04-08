// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! VA-API HW-accelerated AV1 encoder and decoder nodes.
//!
//! Uses the [`cros-codecs`](https://crates.io/crates/cros-codecs) crate which
//! provides high-level VA-API AV1 codec abstractions on Linux.  The cros-codecs
//! `StatelessDecoder` and `StatelessEncoder` handle all AV1 bitstream parsing
//! and VA-API parameter buffer construction internally — this module manages
//! frame I/O and integrates with StreamKit's pipeline architecture.
//!
//! # Nodes
//!
//! - [`VaapiAv1DecoderNode`] — decodes AV1 OBU packets to NV12 [`VideoFrame`]s
//! - [`VaapiAv1EncoderNode`] — encodes NV12/I420 [`VideoFrame`]s to AV1 packets
//!
//! Both perform runtime capability detection: if no VA-API device is found (or
//! AV1 is not supported), node creation returns an error so the pipeline can
//! fall back to a CPU codec (rav1e/dav1d/SVT-AV1).
//!
//! # Feature gate
//!
//! Requires `vaapi` Cargo feature and `libva-dev` + `libgbm-dev` system packages.
//!
//! # Platform support
//!
//! - **Intel**: Full AV1 encode (Arc+) and decode via `intel-media-driver`.
//! - **AMD**: AV1 encode + decode via Mesa RadeonSI VA-API.
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
use cros_codecs::codec::av1::parser::Profile as Av1Profile;
use cros_codecs::decoder::stateless::av1::Av1;
use cros_codecs::decoder::stateless::{DecodeError, StatelessDecoder, StatelessVideoDecoder};
use cros_codecs::decoder::{BlockingMode, DecodedHandle, DecoderEvent};
use cros_codecs::encoder::av1::EncoderConfig as CrosEncoderConfig;
use cros_codecs::encoder::stateless::StatelessEncoder;
use cros_codecs::encoder::{
    FrameMetadata as CrosFrameMetadata, PredictionStructure, RateControl, Tunings, VideoEncoder,
};
use cros_codecs::libva;
use cros_codecs::video_frame::gbm_video_frame::{
    GbmDevice, GbmExternalBufferDescriptor, GbmUsage, GbmVideoFrame,
};
use cros_codecs::video_frame::{ReadMapping, VideoFrame as CrosVideoFrame, WriteMapping};
use cros_codecs::{Fourcc as CrosFourcc, FrameLayout, PlaneLayout, Resolution as CrosResolution};

use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner, StandardVideoEncoder};
use super::HwAccelMode;
use super::AV1_CONTENT_TYPE;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default VA-API render device path.
const DEFAULT_RENDER_DEVICE: &str = "/dev/dri/renderD128";

/// AV1 superblock size — coded resolution must be aligned to this.
const AV1_SB_SIZE: u32 = 64;

/// Maximum number of consecutive retries when the decoder returns
/// `CheckEvents` or `NotEnoughOutputBuffers` without making progress.
/// Matches the established pattern in `av1.rs` and `dav1d.rs`.
const MAX_EAGAIN_EMPTY_RETRIES: u32 = 1000;

/// After this many retries, switch from `thread::yield_now()` to
/// `thread::sleep(1ms)` to avoid a tight spin-loop.
const EAGAIN_YIELD_THRESHOLD: u32 = 10;

/// Default constant-quality parameter (0–255, lower = better quality).
const DEFAULT_QUALITY: u32 = 128;

/// Default framerate for rate-control hints.
const DEFAULT_FRAMERATE: u32 = 30;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// NV12 fourcc code for GBM/VA-API surfaces.
fn nv12_fourcc() -> CrosFourcc {
    CrosFourcc::from(b"NV12")
}

/// Align `value` up to the next multiple of `alignment`.
fn align_up_u32(value: u32, alignment: u32) -> u32 {
    debug_assert!(alignment > 0);
    value.div_ceil(alignment) * alignment
}

/// Auto-detect a VA-API render device by scanning `/dev/dri/renderD*`.
///
/// Returns the first device path that can be opened as a VA display, or `None`
/// if no VA-API capable device is found.
fn detect_render_device() -> Option<String> {
    let mut entries: Vec<_> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("renderD")))
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if libva::Display::open_drm_display(&path).is_ok() {
            return path.to_str().map(String::from);
        }
    }

    None
}

/// Resolve the render device path from config, auto-detection, or default.
fn resolve_render_device(configured: Option<&String>) -> String {
    if let Some(path) = configured {
        return path.clone();
    }

    if let Some(path) = detect_render_device() {
        tracing::info!(device = %path, "auto-detected VA-API render device");
        return path;
    }

    tracing::info!(
        device = DEFAULT_RENDER_DEVICE,
        "no VA-API device detected, falling back to default"
    );
    DEFAULT_RENDER_DEVICE.to_string()
}

/// Open a VA display and a GBM device on the same render node.
fn open_va_and_gbm(
    render_device: Option<&String>,
) -> Result<(Rc<libva::Display>, Arc<GbmDevice>, String), String> {
    let path = resolve_render_device(render_device);
    let display = libva::Display::open_drm_display(&path)
        .map_err(|e| format!("failed to open VA display on {path}: {e}"))?;
    let gbm =
        GbmDevice::open(&path).map_err(|e| format!("failed to open GBM device on {path}: {e}"))?;
    Ok((display, gbm, path))
}

/// Copy NV12 plane data from a GBM read-mapping into a flat `Vec<u8>` suitable
/// for a packed StreamKit [`VideoFrame`].
///
/// Handles stride != width by copying row-by-row.
fn read_nv12_from_mapping(
    mapping: &dyn ReadMapping<'_>,
    width: u32,
    height: u32,
    plane_pitches: &[usize],
) -> Vec<u8> {
    let planes = mapping.get();
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_h = h.div_ceil(2);
    let uv_size = w * uv_h;
    let mut data = vec![0u8; y_size + uv_size];

    // Y plane.
    if !planes.is_empty() {
        let y_stride = plane_pitches.first().copied().unwrap_or(w);
        if y_stride == w {
            let copy_len = y_size.min(planes[0].len());
            data[..copy_len].copy_from_slice(&planes[0][..copy_len]);
        } else {
            for row in 0..h {
                let dst_off = row * w;
                let src_off = row * y_stride;
                if src_off + w <= planes[0].len() && dst_off + w <= y_size {
                    data[dst_off..dst_off + w].copy_from_slice(&planes[0][src_off..src_off + w]);
                }
            }
        }
    }

    // UV plane (interleaved).
    if planes.len() > 1 {
        let uv_stride = plane_pitches.get(1).copied().unwrap_or(w);
        if uv_stride == w {
            let copy_len = uv_size.min(planes[1].len());
            data[y_size..y_size + copy_len].copy_from_slice(&planes[1][..copy_len]);
        } else {
            for row in 0..uv_h {
                let dst_off = y_size + row * w;
                let src_off = row * uv_stride;
                if src_off + w <= planes[1].len() && dst_off + w <= data.len() {
                    data[dst_off..dst_off + w].copy_from_slice(&planes[1][src_off..src_off + w]);
                }
            }
        }
    }

    data
}

/// Write NV12 data from a StreamKit [`VideoFrame`] into a GBM frame's
/// write-mapping.
///
/// If the source is I420, it is converted to NV12 on the fly (U/V planes
/// are interleaved into a single UV plane).
fn write_nv12_to_mapping(
    mapping: &dyn WriteMapping<'_>,
    frame: &VideoFrame,
    plane_pitches: &[usize],
) -> Result<(), String> {
    let planes = mapping.get();
    if planes.is_empty() {
        return Err("GBM mapping returned no planes".into());
    }

    let w = frame.width as usize;
    let h = frame.height as usize;
    let src = frame.data.as_ref().as_ref();

    match frame.pixel_format {
        PixelFormat::Nv12 => {
            let y_size = w * h;
            let uv_size = w * h.div_ceil(2);

            // Y plane.
            let y_stride = plane_pitches.first().copied().unwrap_or(w);
            {
                let mut y_plane = planes[0].borrow_mut();
                if y_stride == w {
                    let n = y_size.min(y_plane.len()).min(src.len());
                    y_plane[..n].copy_from_slice(&src[..n]);
                } else {
                    for row in 0..h {
                        let s = row * w;
                        let d = row * y_stride;
                        if s + w <= src.len() && d + w <= y_plane.len() {
                            y_plane[d..d + w].copy_from_slice(&src[s..s + w]);
                        }
                    }
                }
            }

            // UV plane.
            if planes.len() > 1 {
                let uv_stride = plane_pitches.get(1).copied().unwrap_or(w);
                let mut uv_plane = planes[1].borrow_mut();
                let src_uv = &src[y_size..];
                if uv_stride == w {
                    let n = uv_size.min(uv_plane.len()).min(src_uv.len());
                    uv_plane[..n].copy_from_slice(&src_uv[..n]);
                } else {
                    let uv_h = h.div_ceil(2);
                    for row in 0..uv_h {
                        let s = row * w;
                        let d = row * uv_stride;
                        if s + w <= src_uv.len() && d + w <= uv_plane.len() {
                            uv_plane[d..d + w].copy_from_slice(&src_uv[s..s + w]);
                        }
                    }
                }
            }
        },
        PixelFormat::I420 => {
            // Convert I420 → NV12: Y stays the same, U and V are interleaved.
            let y_size = w * h;
            let uv_w = w.div_ceil(2);
            let uv_h = h.div_ceil(2);
            let u_plane_size = uv_w * uv_h;

            // Y plane.
            let y_stride = plane_pitches.first().copied().unwrap_or(w);
            {
                let mut y_plane = planes[0].borrow_mut();
                if y_stride == w {
                    let n = y_size.min(y_plane.len()).min(src.len());
                    y_plane[..n].copy_from_slice(&src[..n]);
                } else {
                    for row in 0..h {
                        let s = row * w;
                        let d = row * y_stride;
                        if s + w <= src.len() && d + w <= y_plane.len() {
                            y_plane[d..d + w].copy_from_slice(&src[s..s + w]);
                        }
                    }
                }
            }

            // UV plane — interleave U and V from I420 into NV12 UV.
            if planes.len() > 1 {
                let uv_stride = plane_pitches.get(1).copied().unwrap_or(w);
                let mut uv_plane = planes[1].borrow_mut();
                for row in 0..uv_h {
                    for col in 0..uv_w {
                        let u_idx = y_size + row * uv_w + col;
                        let v_idx = y_size + u_plane_size + row * uv_w + col;
                        let dst_idx = row * uv_stride + col * 2;
                        if u_idx < src.len() && v_idx < src.len() && dst_idx + 1 < uv_plane.len() {
                            uv_plane[dst_idx] = src[u_idx];
                            uv_plane[dst_idx + 1] = src[v_idx];
                        }
                    }
                }
            }
        },
        _ => {
            return Err(format!(
                "VA-API AV1 encoder requires NV12 or I420 input, got {:?}",
                frame.pixel_format
            ));
        },
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Configuration for the VA-API AV1 hardware decoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VaapiAv1DecoderConfig {
    /// Path to the DRM render device (e.g. `/dev/dri/renderD128`).
    /// When `None`, auto-detects the first VA-API capable device.
    pub render_device: Option<String>,

    /// Hardware acceleration mode.
    #[serde(default)]
    pub hw_accel: HwAccelMode,
}

impl Default for VaapiAv1DecoderConfig {
    fn default() -> Self {
        Self { render_device: None, hw_accel: HwAccelMode::Auto }
    }
}

pub struct VaapiAv1DecoderNode {
    config: VaapiAv1DecoderConfig,
}

impl VaapiAv1DecoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub fn new(config: VaapiAv1DecoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VaapiAv1DecoderNode only supports hardware decoding; \
                 use video::av1::decoder for CPU decode"
                    .into(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VaapiAv1DecoderNode {
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

        tracing::info!("VaapiAv1DecoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("vaapi_av1_decoder_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("vaapi_av1_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let render_device = self.config.render_device.clone();
        let decode_task = tokio::task::spawn_blocking(move || {
            vaapi_av1_decode_loop(
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
                                "VaapiAv1DecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("VaapiAv1DecoderNode input stream closed");
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
            "VaapiAv1DecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("VaapiAv1DecoderNode finished");
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
fn vaapi_av1_decode_loop(
    render_device: Option<&String>,
    mut decode_rx: mpsc::Receiver<(Bytes, Option<PacketMetadata>)>,
    result_tx: &mpsc::Sender<Result<VideoFrame, String>>,
    duration_histogram: &opentelemetry::metrics::Histogram<f64>,
) {
    // ── Open VA display + GBM device ─────────────────────────────────────
    let (_display, gbm, path) = match open_va_and_gbm(render_device) {
        Ok(v) => v,
        Err(e) => {
            let _ = result_tx.blocking_send(Err(e));
            return;
        },
    };
    tracing::info!(device = %path, "VA-API AV1 decoder opened display");

    // ── Create stateless decoder ─────────────────────────────────────────
    //
    // Re-open the display for the decoder because `StatelessDecoder` takes
    // ownership via `Rc` and we need a separate `Rc` for the GBM device's
    // surface imports.
    let decoder_display = match libva::Display::open_drm_display(&path) {
        Ok(d) => d,
        Err(e) => {
            let _ = result_tx.blocking_send(Err(format!(
                "failed to open VA display for decoder on {path}: {e}"
            )));
            return;
        },
    };

    let mut decoder = match StatelessDecoder::<Av1, VaapiDecBackend<GbmVideoFrame>>::new_vaapi(
        decoder_display,
        BlockingMode::Blocking,
    ) {
        Ok(d) => d,
        Err(e) => {
            let _ =
                result_tx.blocking_send(Err(format!("failed to create VA-API AV1 decoder: {e}")));
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

        // Feed bitstream to the decoder.  The decoder may process it in
        // multiple chunks and may require event handling between calls.
        let mut offset = 0usize;
        let bitstream = data.as_ref();
        let mut eagain_empty_retries: u32 = 0;

        while offset < bitstream.len() {
            let gbm_ref = Arc::clone(&gbm);
            let cw = coded_width;
            let ch = coded_height;
            let mut alloc_cb = move || {
                gbm_ref
                    .clone()
                    .new_frame(
                        nv12_fourcc(),
                        CrosResolution { width: cw, height: ch },
                        CrosResolution { width: cw, height: ch },
                        GbmUsage::Decode,
                    )
                    .ok()
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
                    tracing::error!(error = %e, "VA-API AV1 decode error");
                    let _ = result_tx.blocking_send(Err(format!("VA-API AV1 decode error: {e}")));
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
                        "VA-API AV1 decoder stuck: no progress after {MAX_EAGAIN_EMPTY_RETRIES} retries"
                    );
                    let _ = result_tx.blocking_send(Err(
                        "VA-API AV1 decoder stuck in CheckEvents/NotEnoughOutputBuffers loop"
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
        tracing::warn!(error = %e, "VA-API AV1 decoder flush failed");
    }
    drain_decoder_events(&mut decoder, result_tx, None, &mut coded_width, &mut coded_height);
}

/// Drain all pending events from the decoder.
///
/// Returns `(should_exit, had_events)`:
/// - `should_exit`: the result channel is closed and the caller should return.
/// - `had_events`: at least one event (format change or frame) was processed.
fn drain_decoder_events(
    decoder: &mut StatelessDecoder<Av1, VaapiDecBackend<GbmVideoFrame>>,
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
                        "VA-API AV1 decoder stream format changed"
                    );
                }
            },
            DecoderEvent::FrameReady(handle) => {
                if let Err(e) = handle.sync() {
                    tracing::error!(error = %e, "VA-API AV1 frame sync failed");
                    continue;
                }

                let display_res = handle.display_resolution();
                let frame_w = display_res.width;
                let frame_h = display_res.height;

                let gbm_frame = handle.video_frame();
                let pitches = gbm_frame.get_plane_pitch();

                // Extract NV12 data while the mapping is alive, then drop the
                // mapping before `gbm_frame` to satisfy the borrow checker.
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

/// Configuration for the VA-API AV1 hardware encoder node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VaapiAv1EncoderConfig {
    /// Path to the DRM render device (e.g. `/dev/dri/renderD128`).
    /// When `None`, auto-detects the first VA-API capable device.
    pub render_device: Option<String>,

    /// Constant quality parameter (QP).  Lower values produce higher quality
    /// at the cost of larger bitstream.  Range depends on the driver; typical
    /// range is 0–255, default 128.
    ///
    /// Note: VA-API AV1 encoding via cros-codecs currently supports only the
    /// `ConstantQuality` rate control mode, not `ConstantBitrate`.
    #[serde(default = "default_quality")]
    pub quality: u32,

    /// Target framerate in frames per second (used for rate control hints).
    #[serde(default = "default_framerate")]
    pub framerate: u32,

    /// Use low-power encoding mode if the driver supports it.
    /// Low-power mode uses the GPU's fixed-function encoder (if available)
    /// rather than shader-based encoding, typically offering lower latency
    /// at reduced quality flexibility.
    #[serde(default)]
    pub low_power: bool,

    /// Hardware acceleration mode.
    #[serde(default)]
    pub hw_accel: HwAccelMode,
}

const fn default_quality() -> u32 {
    DEFAULT_QUALITY
}

const fn default_framerate() -> u32 {
    DEFAULT_FRAMERATE
}

impl Default for VaapiAv1EncoderConfig {
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

pub struct VaapiAv1EncoderNode {
    config: VaapiAv1EncoderConfig,
}

impl VaapiAv1EncoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub fn new(config: VaapiAv1EncoderConfig) -> Result<Self, StreamKitError> {
        if matches!(config.hw_accel, HwAccelMode::ForceCpu) {
            return Err(StreamKitError::Configuration(
                "VaapiAv1EncoderNode only supports hardware encoding; \
                 use video::av1::encoder for CPU encode"
                    .into(),
            ));
        }
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for VaapiAv1EncoderNode {
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

impl EncoderNodeRunner for VaapiAv1EncoderNode {
    const CONTENT_TYPE: &'static str = AV1_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "VaapiAv1EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "vaapi_av1_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "vaapi_av1_encode_duration";

    fn spawn_codec_task(
        self,
        encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
        encoder_trait::spawn_standard_encode_task::<VaapiAv1Encoder>(
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

/// Type alias for the full VA-API AV1 encoder with GBM-backed frames.
type CrosVaapiAv1Encoder = StatelessEncoder<
    cros_codecs::encoder::av1::AV1,
    GbmVideoFrame,
    cros_codecs::backend::vaapi::encoder::VaapiBackend<
        GbmExternalBufferDescriptor,
        libva::Surface<GbmExternalBufferDescriptor>,
    >,
>;

/// Internal encoder state wrapping the cros-codecs `StatelessEncoder`.
///
/// `!Send` due to internal `Rc<libva::Display>` — lives entirely inside
/// a `spawn_blocking` thread, matching the pattern in `av1.rs`.
struct VaapiAv1Encoder {
    encoder: CrosVaapiAv1Encoder,
    gbm: Arc<GbmDevice>,
    width: u32,
    height: u32,
    coded_width: u32,
    coded_height: u32,
    frame_count: u64,
}

impl StandardVideoEncoder for VaapiAv1Encoder {
    type Config = VaapiAv1EncoderConfig;
    const CODEC_NAME: &'static str = "VA-API AV1";

    fn new_encoder(width: u32, height: u32, config: &Self::Config) -> Result<Self, String> {
        let (display, gbm, path) = open_va_and_gbm(config.render_device.as_ref())?;
        tracing::info!(device = %path, width, height, "VA-API AV1 encoder opening");

        let coded_width = align_up_u32(width, AV1_SB_SIZE);
        let coded_height = align_up_u32(height, AV1_SB_SIZE);

        let cros_config = CrosEncoderConfig {
            profile: Av1Profile::Profile0,
            bit_depth: cros_codecs::codec::av1::parser::BitDepth::Depth8,
            resolution: CrosResolution { width: coded_width, height: coded_height },
            pred_structure: PredictionStructure::LowDelay { limit: 1024 },
            initial_tunings: Tunings {
                rate_control: RateControl::ConstantQuality(config.quality),
                framerate: config.framerate,
                min_quality: 0,
                max_quality: 255,
            },
        };

        let encoder = CrosVaapiAv1Encoder::new_vaapi(
            display,
            cros_config,
            nv12_fourcc(),
            CrosResolution { width: coded_width, height: coded_height },
            config.low_power,
            BlockingMode::Blocking,
        )
        .map_err(|e| format!("failed to create VA-API AV1 encoder: {e}"))?;

        tracing::info!(
            device = %path,
            width,
            height,
            coded_width,
            coded_height,
            quality = config.quality,
            "VA-API AV1 encoder created"
        );

        Ok(Self { encoder, gbm, width, height, coded_width, coded_height, frame_count: 0 })
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<PacketMetadata>,
    ) -> Result<Vec<EncodedPacket>, String> {
        if frame.pixel_format == PixelFormat::Rgba8 {
            return Err("VA-API AV1 encoder requires NV12 or I420 input; \
                 insert a video::pixel_convert node upstream"
                .into());
        }

        // Create a GBM frame and upload the raw video data.
        let mut gbm_frame = Arc::clone(&self.gbm)
            .new_frame(
                nv12_fourcc(),
                CrosResolution { width: self.width, height: self.height },
                CrosResolution { width: self.coded_width, height: self.coded_height },
                GbmUsage::Encode,
            )
            .map_err(|e| format!("failed to allocate GBM frame for encoding: {e}"))?;

        // Write frame data into the GBM buffer.
        let pitches = gbm_frame.get_plane_pitch();
        {
            let mapping = gbm_frame
                .map_mut()
                .map_err(|e| format!("failed to map GBM frame for writing: {e}"))?;
            write_nv12_to_mapping(mapping.as_ref(), frame, &pitches)?;
        }

        let is_keyframe = metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false);
        let timestamp = metadata.as_ref().and_then(|m| m.timestamp_us).unwrap_or(self.frame_count);

        let frame_layout = FrameLayout {
            format: (nv12_fourcc(), 0), // DRM_FORMAT_MOD_LINEAR
            size: CrosResolution { width: self.coded_width, height: self.coded_height },
            planes: vec![
                PlaneLayout {
                    buffer_index: 0,
                    offset: 0,
                    stride: pitches.first().copied().unwrap_or(self.width as usize),
                },
                PlaneLayout {
                    buffer_index: 0,
                    offset: pitches.first().copied().unwrap_or(self.width as usize)
                        * self.coded_height as usize,
                    stride: pitches.get(1).copied().unwrap_or(self.width as usize),
                },
            ],
        };

        let cros_meta =
            CrosFrameMetadata { timestamp, layout: frame_layout, force_keyframe: is_keyframe };

        self.encoder
            .encode(cros_meta, gbm_frame)
            .map_err(|e| format!("VA-API AV1 encode error: {e}"))?;

        self.frame_count += 1;

        // Poll for all available encoded output.
        let mut packets = Vec::new();
        loop {
            match self.encoder.poll() {
                Ok(Some(coded)) => {
                    packets.push(EncodedPacket {
                        data: Bytes::from(coded.bitstream),
                        metadata: metadata.clone(),
                    });
                },
                Ok(None) => break,
                Err(e) => return Err(format!("VA-API AV1 encoder poll error: {e}")),
            }
        }

        Ok(packets)
    }

    fn flush_encoder(&mut self) -> Result<Vec<EncodedPacket>, String> {
        self.encoder.drain().map_err(|e| format!("VA-API AV1 encoder drain error: {e}"))?;

        let mut packets = Vec::new();
        loop {
            match self.encoder.poll() {
                Ok(Some(coded)) => {
                    packets
                        .push(EncodedPacket { data: Bytes::from(coded.bitstream), metadata: None });
                },
                Ok(None) => break,
                Err(e) => return Err(format!("VA-API AV1 encoder poll error: {e}")),
            }
        }

        Ok(packets)
    }

    fn flush_on_dimension_change() -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_vaapi_av1_nodes(registry: &mut NodeRegistry) {
    let default_decoder = VaapiAv1DecoderNode::new(VaapiAv1DecoderConfig::default())
        .expect("default VA-API AV1 decoder config should be valid");
    registry.register_static_with_description(
        "video::vaapi::av1_decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VaapiAv1DecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VaapiAv1DecoderConfig))
            .expect("VaapiAv1DecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "av1".to_string(),
            "hw".to_string(),
            "vaapi".to_string(),
        ],
        false,
        "Decodes AV1-compressed packets into raw NV12 video frames using VA-API \
         hardware acceleration. Requires a VA-API capable GPU (Intel Arc+, AMD, \
         or NVIDIA with nvidia-vaapi-driver).",
    );

    let default_encoder = VaapiAv1EncoderNode::new(VaapiAv1EncoderConfig::default())
        .expect("default VA-API AV1 encoder config should be valid");
    registry.register_static_with_description(
        "video::vaapi::av1_encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(VaapiAv1EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(VaapiAv1EncoderConfig))
            .expect("VaapiAv1EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec![
            "video".to_string(),
            "codecs".to_string(),
            "av1".to_string(),
            "hw".to_string(),
            "vaapi".to_string(),
        ],
        false,
        "Encodes raw NV12/I420 video frames into AV1-compressed packets using VA-API \
         hardware acceleration. Uses constant-quality (CQP) rate control. Requires a \
         VA-API capable GPU with AV1 encode support (Intel Arc+, AMD).",
    );
}
