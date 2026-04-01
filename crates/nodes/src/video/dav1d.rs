// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! C dav1d AV1 decoder node.
//!
//! Uses the C [dav1d](https://code.videolan.org/videolan/dav1d) library via
//! hand-written FFI bindings ([`super::dav1d_ffi`]) for decoding.  Unlike the
//! pure-Rust rav1d port (in [`super::av1`]), C dav1d handles corrupt /
//! truncated bitstreams gracefully via negative error codes — it never panics.
//!
//! This node is an **alternative** to the rav1d-based `video::av1::decoder`;
//! both coexist and register different node kinds:
//!
//! - `video::av1::decoder` — rav1d (pure Rust, no C deps)
//! - `video::dav1d::decoder` — C dav1d (requires libdav1d at link time)
//!
//! The architecture mirrors the rav1d decoder: `spawn_blocking` + `mpsc`
//! channels + [`crate::codec_utils::codec_forward_loop`].

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::Deserialize;
use std::ffi::{c_int, c_void};
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

use super::dav1d_ffi::{self, DAV1D_EAGAIN, DAV1D_PIXEL_LAYOUT_I420};

/// Default to auto-detect (`0`).  dav1d picks a thread count based on the
/// number of logical cores.
const DAV1D_DEFAULT_THREADS: u32 = 0;

/// Maximum number of consecutive EAGAIN retries in `decode_packet` where
/// `drain_pictures` returns no frames.  Prevents an infinite busy-loop if the
/// decoder gets into a pathological state.
const MAX_EAGAIN_EMPTY_RETRIES: u32 = 1000;

/// After this many consecutive empty EAGAIN retries, switch from
/// `thread::yield_now()` to `thread::sleep(1ms)` to avoid a tight
/// spin-loop on lightly-loaded systems where `yield_now` returns
/// immediately.
const EAGAIN_YIELD_THRESHOLD: u32 = 10;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct Dav1dDecoderConfig {
    /// Number of decoder threads.  `0` = auto-detect (dav1d picks a
    /// thread count based on the number of logical cores).
    pub threads: u32,
}

impl Default for Dav1dDecoderConfig {
    fn default() -> Self {
        Self { threads: DAV1D_DEFAULT_THREADS }
    }
}

// ---------------------------------------------------------------------------
// Decoder node
// ---------------------------------------------------------------------------

pub struct Dav1dDecoderNode {
    config: Dav1dDecoderConfig,
}

impl Dav1dDecoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: Dav1dDecoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for Dav1dDecoderNode {
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

        tracing::info!("Dav1dDecoderNode starting");
        let mut input_rx = context.take_input("in")?;
        let video_pool = context.video_pool.clone();

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("dav1d_decoder_packets_processed").build();
        let decode_duration_histogram = meter
            .f64_histogram("dav1d_decode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (decode_tx, mut decode_rx) =
            mpsc::channel::<(Bytes, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<VideoFrame, String>>(get_codec_channel_capacity());

        let decoder_threads = self.config.threads;
        let decode_task = tokio::task::spawn_blocking(move || {
            let mut decoder = match Dav1dDecoder::new(decoder_threads) {
                Ok(decoder) => decoder,
                Err(err) => {
                    let _ = result_tx.blocking_send(Err(err));
                    return;
                },
            };

            while let Some((data, metadata)) = decode_rx.blocking_recv() {
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

            // Flush remaining frames from the decoder.
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
                                "Dav1dDecoderNode decode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("Dav1dDecoderNode input stream closed");
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
            "Dav1dDecoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("Dav1dDecoderNode finished");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// C dav1d decoder wrapper
// ---------------------------------------------------------------------------

struct Dav1dDecoder {
    /// Opaque dav1d context handle.  Owned; closed in [`Drop`].
    ctx: *mut c_void,
}

// `Dav1dDecoder` is intentionally `!Send` (raw pointer).
// It is created and used entirely inside a single `spawn_blocking` closure
// in `Dav1dDecoderNode::run`, so no `Send` bound is required.

impl Dav1dDecoder {
    fn new(threads: u32) -> Result<Self, String> {
        let mut settings = dav1d_ffi::Dav1dSettings::zeroed();
        // SAFETY: `dav1d_default_settings` writes valid defaults into `settings`.
        unsafe {
            dav1d_ffi::dav1d_default_settings(&raw mut settings);
        }

        let n_threads = c_int::try_from(threads).unwrap_or(0);
        settings.set_n_threads(n_threads);
        // Optimise for low latency: emit frames as soon as possible.
        settings.set_max_frame_delay(1);

        let mut ctx: *mut c_void = std::ptr::null_mut();
        // SAFETY: `ctx` is valid to write into; `settings` is valid to read.
        let res = unsafe { dav1d_ffi::dav1d_open(&raw mut ctx, &raw const settings) };
        if res < 0 {
            return Err(format!("dav1d: dav1d_open failed with code {res}"));
        }
        if ctx.is_null() {
            return Err("dav1d: dav1d_open returned null context".to_string());
        }
        Ok(Self { ctx })
    }

    fn decode_packet(
        &mut self,
        data: &[u8],
        metadata: Option<&PacketMetadata>,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // No OBU pre-validation needed — C dav1d handles corrupt data
        // gracefully via negative error codes (unlike rav1d which panics).

        // Wrap the input data in a Dav1dData.
        let mut dav1d_data = dav1d_ffi::Dav1dData::zeroed();
        let buf_ptr =
            unsafe { dav1d_ffi::dav1d_data_create(&raw mut dav1d_data, data.len()) };
        if buf_ptr.is_null() {
            return Err("dav1d: failed to allocate Dav1dData buffer".to_string());
        }
        // Copy our data into the dav1d-managed buffer.
        // SAFETY: `dav1d_data_create` returned a valid buffer of `data.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, data.len());
        }

        // RAII guard ensures `dav1d_data_unref` is called on all error paths.
        let mut data_guard = Dav1dDataGuard::new(&raw mut dav1d_data);

        let mut all_frames = Vec::new();

        // Feed data to the decoder in a retry loop.  The dav1d API contract
        // states that when `dav1d_send_data` returns EAGAIN the input data
        // was **not** consumed.  The caller must drain pending pictures via
        // `dav1d_get_picture` and then retry with the same `Dav1dData`.
        let mut eagain_empty_retries: u32 = 0;

        loop {
            let res = unsafe { dav1d_ffi::dav1d_send_data(self.ctx, &raw mut dav1d_data) };

            if res == 0 {
                // Data consumed successfully — defuse the guard.
                data_guard.defuse();
                break;
            }

            if res == DAV1D_EAGAIN {
                // EAGAIN — drain buffered pictures, then retry send.
                let mut drained = self.drain_pictures(metadata, video_pool)?;
                if drained.is_empty() {
                    eagain_empty_retries += 1;
                    if eagain_empty_retries > MAX_EAGAIN_EMPTY_RETRIES {
                        return Err(
                            "dav1d: dav1d_send_data stuck in EAGAIN loop \
                             (no pictures produced after 1000 retries)"
                                .to_string(),
                        );
                    }
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

            // Real error — guard will call dav1d_data_unref on drop.
            return Err(format!("dav1d: dav1d_send_data failed with code {res}"));
        }

        // Drain any pictures produced by the successful send.
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
        let mut frames = Vec::with_capacity(1);
        loop {
            let mut pic = dav1d_ffi::Dav1dPicture::zeroed();
            let res = unsafe { dav1d_ffi::dav1d_get_picture(self.ctx, &raw mut pic) };

            if res == DAV1D_EAGAIN {
                // No more pictures available right now.
                break;
            }
            if res < 0 {
                if !frames.is_empty() {
                    tracing::warn!(
                        "dav1d: dav1d_get_picture error {res} after draining {} frame(s) — \
                         returning buffered frames",
                        frames.len(),
                    );
                    break;
                }
                return Err(format!("dav1d: dav1d_get_picture failed with code {res}"));
            }

            let meta = metadata.cloned();

            match copy_dav1d_picture(&pic, meta, video_pool) {
                Ok(frame) => frames.push(frame),
                Err(err) => {
                    unsafe {
                        dav1d_ffi::dav1d_picture_unref(&raw mut pic);
                    }
                    if frames.is_empty() {
                        return Err(err);
                    }
                    tracing::warn!(
                        "dav1d: copy_dav1d_picture error after draining {} frame(s) — \
                         returning buffered frames: {err}",
                        frames.len(),
                    );
                    break;
                },
            }

            unsafe {
                dav1d_ffi::dav1d_picture_unref(&raw mut pic);
            }
        }

        Ok(frames)
    }

    /// Drain remaining buffered pictures and reset the decoder.
    fn flush(
        &mut self,
        video_pool: Option<&Arc<VideoFramePool>>,
    ) -> Result<Vec<VideoFrame>, String> {
        let frames = self.drain_pictures(None, video_pool)?;
        unsafe {
            dav1d_ffi::dav1d_flush(self.ctx);
        }
        Ok(frames)
    }
}

impl Drop for Dav1dDecoder {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            let mut ctx = self.ctx;
            // SAFETY: ctx is valid and was returned by dav1d_open.
            // dav1d_close sets *c_out to NULL.
            unsafe {
                dav1d_ffi::dav1d_close(&raw mut ctx);
            }
            self.ctx = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// Dav1dData RAII guard
// ---------------------------------------------------------------------------

/// RAII guard for a `Dav1dData` buffer.  Calls `dav1d_data_unref` on drop
/// unless explicitly defused (e.g. after `dav1d_send_data` consumes the data).
///
/// # Safety invariant
///
/// The raw pointer must remain valid for the guard's entire lifetime.
struct Dav1dDataGuard {
    ptr: *mut dav1d_ffi::Dav1dData,
    active: bool,
}

impl Dav1dDataGuard {
    const fn new(ptr: *mut dav1d_ffi::Dav1dData) -> Self {
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
            // SAFETY: ptr is valid (guaranteed by caller's stack lifetime).
            unsafe {
                dav1d_ffi::dav1d_data_unref(self.ptr);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Picture copy (dav1d opaque → NV12 VideoFrame)
// ---------------------------------------------------------------------------

/// Copy a decoded `Dav1dPicture` (I420) into an NV12 [`VideoFrame`].
///
/// dav1d always decodes AV1 to I420 (three separate Y, U, V planes).
/// We convert to NV12 on the fly by copying the Y plane as-is and
/// interleaving the U and V planes into a single UV plane.
fn copy_dav1d_picture(
    pic: &dav1d_ffi::Dav1dPicture,
    metadata: Option<PacketMetadata>,
    video_pool: Option<&Arc<VideoFramePool>>,
) -> Result<VideoFrame, String> {
    // The chroma copy below assumes 4:2:0 subsampling.
    if pic.layout() != DAV1D_PIXEL_LAYOUT_I420 {
        return Err(format!(
            "dav1d decoder produced unsupported pixel layout {} (expected I420 = {DAV1D_PIXEL_LAYOUT_I420})",
            pic.layout(),
        ));
    }

    let width_i = pic.width();
    let height_i = pic.height();
    if width_i <= 0 || height_i <= 0 {
        return Err("dav1d decoder produced empty frame".to_string());
    }

    #[allow(clippy::cast_sign_loss)]
    let width = width_i as u32;
    #[allow(clippy::cast_sign_loss)]
    let height = height_i as u32;

    // Y plane
    let y_ptr = pic.data_ptr(0);
    if y_ptr.is_null() {
        return Err("dav1d decoder returned null Y plane".to_string());
    }
    let y_stride = pic.stride(0);
    if y_stride <= 0 {
        return Err("dav1d decoder returned invalid Y stride".to_string());
    }

    // U plane
    let u_ptr = pic.data_ptr(1);
    if u_ptr.is_null() {
        return Err("dav1d decoder returned null U plane".to_string());
    }
    let u_stride = pic.stride(1);
    if u_stride <= 0 {
        return Err("dav1d decoder returned invalid U stride".to_string());
    }

    // V plane — shares chroma stride with U.
    let v_ptr = pic.data_ptr(2);
    if v_ptr.is_null() {
        return Err("dav1d decoder returned null V plane".to_string());
    }
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

    // Copy Y plane — reuse the helper from av1.rs.
    #[allow(clippy::cast_possible_truncation)]
    super::av1::copy_dav1d_plane(
        &mut data_slice[y_plane.offset..y_plane.offset + y_plane.stride * y_plane.height as usize],
        y_plane.stride,
        y_ptr,
        y_stride as c_int,
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

    if u_stride_usize < chroma_w {
        return Err(format!(
            "dav1d decoder U plane stride ({u_stride_usize}) < chroma width ({chroma_w})"
        ));
    }
    if v_stride_usize < chroma_w {
        return Err(format!(
            "dav1d decoder V plane stride ({v_stride_usize}) < chroma width ({chroma_w})"
        ));
    }

    for row in 0..chroma_h {
        // SAFETY: stride >= chroma_w (checked above) and dav1d allocates
        // at least stride * chroma_h bytes per plane.
        let u_row = unsafe {
            std::slice::from_raw_parts(u_ptr.add(row * u_stride_usize), chroma_w)
        };
        let v_row = unsafe {
            std::slice::from_raw_parts(v_ptr.add(row * v_stride_usize), chroma_w)
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

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_dav1d_nodes(registry: &mut NodeRegistry) {
    let default_decoder = Dav1dDecoderNode::new(Dav1dDecoderConfig::default())
        .expect("default dav1d decoder config should be valid");
    registry.register_static_with_description(
        "video::dav1d::decoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(Dav1dDecoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(Dav1dDecoderConfig))
            .expect("Dav1dDecoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_decoder.input_pins(), outputs: default_decoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "av1".to_string()],
        false,
        "Decodes AV1-compressed packets into raw NV12 video frames using the C dav1d library. \
         Unlike the rav1d (pure-Rust) decoder, dav1d handles corrupt/truncated bitstreams \
         gracefully via error codes instead of panicking.",
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    use streamkit_core::types::Packet;
    use tokio::sync::mpsc;

    use super::super::AV1_CONTENT_TYPE;

    /// Encode frames with rav1e, then decode with C dav1d — validates
    /// cross-codec round-trip.
    #[cfg(feature = "av1")]
    #[tokio::test]
    async fn test_dav1d_decode_from_rav1e_encoded() {
        use super::super::av1::{Av1EncoderConfig, Av1EncoderNode};

        // --- Encode with rav1e ---
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
            let mut frame = create_test_video_frame(64, 64, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp),
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
        assert!(!encoded_packets.is_empty(), "rav1e encoder produced no packets");

        // --- Decode with C dav1d ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = Dav1dDecoderNode::new(Dav1dDecoderConfig { threads: 1 }).unwrap();
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
        assert!(!decoded_packets.is_empty(), "dav1d decoder produced no frames");

        for packet in decoded_packets {
            match packet {
                Packet::Video(frame) => {
                    assert_eq!(frame.width, 64);
                    assert_eq!(frame.height, 64);
                    assert_eq!(frame.pixel_format, PixelFormat::Nv12);
                    assert!(!frame.data().is_empty(), "Decoded frame should have data");
                },
                _ => panic!("Expected Video packet from dav1d decoder"),
            }
        }
    }

    /// Encode many frames, decode with dav1d — exercises the EAGAIN retry loop.
    #[cfg(feature = "av1")]
    #[tokio::test]
    async fn test_dav1d_decode_many_frames_no_data_loss() {
        use super::super::av1::{Av1EncoderConfig, Av1EncoderNode};

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
            "rav1e encoder produced no packets for {FRAME_COUNT} input frames"
        );

        // --- Decode with dav1d ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(32);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 32);
        let decoder = Dav1dDecoderNode::new(Dav1dDecoderConfig { threads: 1 }).unwrap();
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
                _ => panic!("Expected Video packet from dav1d decoder"),
            }
        }
    }

    /// Verify that decoded frames preserve metadata from the input packet.
    #[cfg(feature = "av1")]
    #[tokio::test]
    async fn test_dav1d_metadata_propagation() {
        use super::super::av1::{Av1EncoderConfig, Av1EncoderNode};

        // --- Encode ---
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder = Av1EncoderNode::new(Av1EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 0,
            threads: 1,
            speed: 10,
            low_latency: true,
            quantizer: 80,
        })
        .unwrap();
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

        // --- Decode with dav1d ---
        let (dec_input_tx, dec_input_rx) = mpsc::channel(10);
        let mut dec_inputs = HashMap::new();
        dec_inputs.insert("in".to_string(), dec_input_rx);

        let (dec_context, dec_sender, mut dec_state_rx) = create_test_context(dec_inputs, 10);
        let decoder = Dav1dDecoderNode::new(Dav1dDecoderConfig::default()).unwrap();
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
        assert!(!decoded_packets.is_empty(), "dav1d decoder should produce at least one frame");

        for (i, packet) in decoded_packets.iter().enumerate() {
            match packet {
                Packet::Video(frame) => {
                    assert!(
                        frame.metadata.is_some(),
                        "Decoded frame {i} should have metadata"
                    );
                },
                _ => panic!("Expected Video packet from dav1d decoder"),
            }
        }
    }
}
