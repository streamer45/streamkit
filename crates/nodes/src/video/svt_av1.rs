// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! SVT-AV1 video encoder node (CPU).
//!
//! Uses [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (≥ 2.0) via
//! hand-written FFI bindings for encoding.
//!
//! **Requires `libsvtav1enc` to be installed on the build host.**
//! See `crates/nodes/SVT_AV1.md` for installation instructions.
//!
//! ## Architecture
//!
//! SVT-AV1 in low-delay mode (`SVT_AV1_PRED_LOW_DELAY_B`) uses a **blocking**
//! `get_packet` call — unlike rav1e's non-blocking `receive_packet`.  To avoid
//! a deadlock between send and receive, this node uses **two** blocking threads
//! (matching SVT-AV1's own `SvtAv1EncApp` design):
//!
//! - **Send thread**: receives `VideoFrame`s from the async input task,
//!   converts NV12 → I420, and calls `svt_av1_enc_send_picture`.
//! - **Receive thread**: loops on `svt_av1_enc_get_packet` (which blocks
//!   until output is available in low-delay mode), copies encoded data,
//!   and forwards results to the async `codec_forward_loop`.
//!
//! SVT-AV1's `send_picture` and `get_packet` are explicitly designed for
//! concurrent use from different threads — the library provides internal
//! synchronisation via FIFOs and semaphores.
//!
//! Output packets are `Packet::Binary` with `content_type = "video/av1"`,
//! making this a drop-in replacement for the rav1e encoder in any pipeline.

use async_trait::async_trait;
use bytes::Bytes;
use opentelemetry::global;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::ffi::CString;
use std::time::Instant;
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

use super::svt_av1_ffi::{
    self, EbBufferHeaderType, EbComponentType, EbSvtAv1EncConfiguration, EbSvtIOFormat,
    EB_BUFFERFLAG_EOS, EB_ERROR_NONE, EB_NO_ERROR_EMPTY_QUEUE,
};

const AV1_CONTENT_TYPE: &str = "video/av1";

// ── Default config values ────────────────────────────────────────────────────

/// Default to constant-quality mode (CRF).  In bitrate mode SVT-AV1 may
/// buffer frames for rate-control look-ahead.  CRF mode with low-delay
/// prediction structure emits packets with minimal latency.
const SVT_AV1_DEFAULT_BITRATE_KBPS: u32 = 0;
const SVT_AV1_DEFAULT_KF_INTERVAL: u32 = 120;
/// SVT-AV1 preset (0 = slowest/best quality, 13 = fastest).
/// Default 12 matches SVT-AV1's own default and is suitable for real-time.
const SVT_AV1_DEFAULT_PRESET: u32 = 12;
/// CRF quality (1–63 range, lower = better quality).
/// 35 is a reasonable real-time default.
const SVT_AV1_DEFAULT_CRF: u32 = 35;
/// 0 = auto-detect thread count (SVT-AV1 picks based on core count).
const SVT_AV1_DEFAULT_PARALLELISM: u32 = 0;

// ── YUV420 color format constant ─────────────────────────────────────────────
const EB_YUV420: u32 = 1;

// ---------------------------------------------------------------------------
// Configuration struct
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct SvtAv1EncoderConfig {
    /// Target bitrate in kbps.  `0` = CRF mode (constant quality).
    pub bitrate_kbps: u32,
    /// Keyframe interval in frames.
    pub keyframe_interval: u32,
    /// SVT-AV1 preset (0–13).  Lower = better quality / slower.
    pub preset: u32,
    /// CRF quality level (1–63).  Only used when `bitrate_kbps` is 0.
    pub crf: u32,
    /// Number of encoder threads.  `0` = auto-detect.
    pub parallelism: u32,
    /// Use low-delay prediction structure (minimal latency).
    pub low_latency: bool,
}

impl Default for SvtAv1EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_kbps: SVT_AV1_DEFAULT_BITRATE_KBPS,
            keyframe_interval: SVT_AV1_DEFAULT_KF_INTERVAL,
            preset: SVT_AV1_DEFAULT_PRESET,
            crf: SVT_AV1_DEFAULT_CRF,
            parallelism: SVT_AV1_DEFAULT_PARALLELISM,
            low_latency: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Encoder node
// ---------------------------------------------------------------------------

pub struct SvtAv1EncoderNode {
    config: SvtAv1EncoderConfig,
}

impl SvtAv1EncoderNode {
    #[allow(clippy::missing_errors_doc)]
    pub const fn new(config: SvtAv1EncoderConfig) -> Result<Self, StreamKitError> {
        Ok(Self { config })
    }
}

#[async_trait]
impl ProcessorNode for SvtAv1EncoderNode {
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

        tracing::info!("SvtAv1EncoderNode starting");
        let mut input_rx = context.take_input("in")?;

        let meter = global::meter("skit_nodes");
        let packets_processed_counter =
            meter.u64_counter("svt_av1_encoder_packets_processed").build();
        let encode_duration_histogram = meter
            .f64_histogram("svt_av1_encode_duration")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CODEC_PACKET.to_vec())
            .build();

        let (encode_tx, mut encode_rx) =
            mpsc::channel::<(VideoFrame, Option<PacketMetadata>)>(get_codec_channel_capacity());
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<EncodedPacket, String>>(get_codec_channel_capacity());

        // ── Codec task ───────────────────────────────────────────────────
        //
        // SVT-AV1 in low-delay mode blocks on `get_packet` until a packet
        // is available.  To avoid deadlocking (send_picture cannot proceed
        // if get_packet is blocked on the same thread), we split into two
        // OS threads — matching SVT-AV1's own SvtAv1EncApp architecture.
        //
        // The send thread owns the `SvtAv1Encoder` struct (and its plane
        // buffers).  The receive thread only needs the raw `handle` pointer;
        // SVT-AV1 provides internal synchronisation for concurrent
        // `send_picture` / `get_packet` calls from different threads.
        let encoder_config = self.config;
        let encode_task = tokio::task::spawn_blocking(move || {
            let mut encoder: Option<SvtAv1Encoder> = None;
            let mut current_dimensions: Option<(u32, u32)> = None;
            let mut recv_thread: Option<std::thread::JoinHandle<()>> = None;

            while let Some((frame, metadata)) = encode_rx.blocking_recv() {
                if result_tx.is_closed() {
                    return;
                }

                if frame.pixel_format == PixelFormat::Rgba8 {
                    let _ = result_tx.blocking_send(Err(
                        "SVT-AV1 encoder requires NV12 or I420 input; \
                         insert a video::pixel_convert node upstream"
                            .to_string(),
                    ));
                    continue;
                }

                let frame_dimensions = (frame.width, frame.height);
                if current_dimensions != Some(frame_dimensions) {
                    // Stop old receive thread + flush old encoder.
                    if let Some(old_encoder) = encoder.take() {
                        // Send EOS to the old encoder so the receive thread
                        // sees the EOS-flagged packet and exits.
                        send_eos(old_encoder.handle);
                        if let Some(t) = recv_thread.take() {
                            let _ = t.join();
                        }
                        // Drop old encoder (calls deinit + deinit_handle).
                        drop(old_encoder);
                    }

                    match SvtAv1Encoder::new(frame.width, frame.height, &encoder_config) {
                        Ok(new_encoder) => {
                            // Start a new receive thread for this encoder.
                            let handle_raw = new_encoder.handle as usize;
                            let recv_result_tx = result_tx.clone();
                            recv_thread = Some(std::thread::spawn(move || {
                                receive_loop(handle_raw, &recv_result_tx);
                            }));
                            encoder = Some(new_encoder);
                            current_dimensions = Some(frame_dimensions);
                        },
                        Err(err) => {
                            tracing::warn!(
                                width = frame.width,
                                height = frame.height,
                                "SVT-AV1 encoder re-creation failed, dropping frame: {err}"
                            );
                            let _ = result_tx.blocking_send(Err(err));
                            continue;
                        },
                    }
                }

                let Some(enc) = encoder.as_mut() else {
                    let _ =
                        result_tx.blocking_send(Err("SVT-AV1 encoder not initialized".to_string()));
                    continue;
                };

                let encode_start_time = Instant::now();
                let result = enc.send_frame(&frame, metadata.as_ref());
                encode_duration_histogram.record(encode_start_time.elapsed().as_secs_f64(), &[]);

                if let Err(err) = result {
                    let _ = result_tx.blocking_send(Err(err));
                }
            }

            // Input channel closed — flush the encoder.
            if let Some(enc) = encoder.take() {
                send_eos(enc.handle);
                if let Some(t) = recv_thread.take() {
                    let _ = t.join();
                }
                // Drop encoder (calls deinit + deinit_handle).
                drop(enc);
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
                                "SvtAv1EncoderNode encode task has shut down unexpectedly"
                            );
                            return;
                        }
                    }
                }
            }
            tracing::info!("SvtAv1EncoderNode input stream closed");
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
            "SvtAv1EncoderNode",
        )
        .await;

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");
        tracing::info!("SvtAv1EncoderNode finished");
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
// Receive loop (runs on its own OS thread)
// ---------------------------------------------------------------------------

/// Blocking receive loop: calls `svt_av1_enc_get_packet` in a loop until
/// the encoder signals EOS or `EB_NoErrorEmptyQueue`.
///
/// SVT-AV1 in low-delay mode uses a blocking FIFO pop for `get_packet`,
/// so this thread will sleep until encoded data is available — it does NOT
/// busy-wait.
///
/// # Safety
///
/// `handle_raw` must be a valid `*mut EbComponentType` cast to `usize`.
/// The caller must ensure the encoder handle remains live for the duration
/// of this function.
fn receive_loop(handle_raw: usize, result_tx: &mpsc::Sender<Result<EncodedPacket, String>>) {
    let handle = handle_raw as *mut EbComponentType;

    loop {
        let mut out_buf: *mut EbBufferHeaderType = std::ptr::null_mut();

        // SAFETY: handle is valid (guaranteed by caller), out_buf is written
        // by the library.  `pic_send_done = 0` — the EOS flag on the output
        // buffer signals completion instead.
        let ret = unsafe { svt_av1_ffi::svt_av1_enc_get_packet(handle, &raw mut out_buf, 0) };

        if ret == EB_NO_ERROR_EMPTY_QUEUE {
            // Encoder has been fully drained (eos_sent is set internally).
            break;
        }
        if ret != EB_ERROR_NONE {
            let _ = result_tx
                .blocking_send(Err(format!("svt_av1_enc_get_packet failed: error code {ret:#X}")));
            break;
        }

        // SAFETY: When ret == EB_ERROR_NONE, out_buf is a valid pointer to
        // an encoder-owned buffer.
        let (data, is_keyframe, is_eos, pts) = unsafe {
            let buf = &*out_buf;
            let is_eos = (buf.flags & EB_BUFFERFLAG_EOS) != 0;
            let is_keyframe = buf.pic_type == 3; // EB_AV1_KEY_PICTURE
            let pts = buf.pts;
            let data = if buf.p_buffer.is_null() || buf.n_filled_len == 0 {
                Bytes::new()
            } else {
                let slice = std::slice::from_raw_parts(buf.p_buffer, buf.n_filled_len as usize);
                Bytes::copy_from_slice(slice)
            };
            (data, is_keyframe, is_eos, pts)
        };

        // Release the buffer back to the encoder's pool.
        // SAFETY: out_buf was returned by svt_av1_enc_get_packet.
        unsafe { svt_av1_ffi::svt_av1_enc_release_out_buffer(&raw mut out_buf) };

        // SVT-AV1 emits a final EOS sentinel packet with no encoded data.
        // Skip it — only forward packets that carry actual bitstream bytes.
        if !data.is_empty() {
            let metadata = PacketMetadata {
                timestamp_us: if pts >= 0 { Some(pts.cast_unsigned()) } else { None },
                duration_us: None,
                sequence: None,
                keyframe: Some(is_keyframe),
            };

            if result_tx
                .blocking_send(Ok(EncodedPacket { data, metadata: Some(metadata) }))
                .is_err()
            {
                // Async side dropped the receiver — stop.
                break;
            }
        }

        if is_eos {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// SVT-AV1 encoder wrapper (send-side only)
// ---------------------------------------------------------------------------

struct SvtAv1Encoder {
    handle: *mut EbComponentType,
    width: u32,
    height: u32,
    next_pts: i64,
    /// Reusable Y plane buffer.
    y_plane: Vec<u8>,
    /// Reusable U plane buffer.
    u_plane: Vec<u8>,
    /// Reusable V plane buffer.
    v_plane: Vec<u8>,
}

// SAFETY: The SVT-AV1 encoder handle uses internal locking for thread-safety.
// `send_picture` and `get_packet` are designed to be called from separate
// threads concurrently.  Our `SvtAv1Encoder` struct is only used from the
// send thread; the handle pointer is also passed to the receive thread via
// `receive_loop`.
unsafe impl Send for SvtAv1Encoder {}

impl SvtAv1Encoder {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn new(width: u32, height: u32, config: &SvtAv1EncoderConfig) -> Result<Self, String> {
        let mut enc_config = EbSvtAv1EncConfiguration::zeroed();
        let mut handle: *mut EbComponentType = std::ptr::null_mut();

        // Step 1: Init handle — fills enc_config with defaults.
        // SAFETY: We pass valid pointers.  `handle` is written by the library.
        let ret = unsafe {
            svt_av1_ffi::svt_av1_enc_init_handle(
                &raw mut handle,
                std::ptr::null_mut(),
                &raw mut enc_config,
            )
        };
        if ret != EB_ERROR_NONE {
            return Err(format!("svt_av1_enc_init_handle failed: error code {ret:#X}"));
        }

        // From here on, if anything fails we must call deinit_handle to avoid
        // leaking the native encoder handle.  We use a helper closure so that
        // all error paths are covered by a single cleanup call.
        let configure_result =
            Self::configure_and_init(handle, &mut enc_config, width, height, config);
        if let Err(err) = &configure_result {
            tracing::debug!("SVT-AV1 init failed ({err}), releasing handle");
            // SAFETY: handle was successfully created by init_handle above.
            unsafe { svt_av1_ffi::svt_av1_enc_deinit_handle(handle) };
            return Err(configure_result.unwrap_err());
        }

        // Pre-allocate plane buffers for NV12→I420 de-interleaving.
        let y_size = (width * height) as usize;
        let chroma_w = width.div_ceil(2) as usize;
        let chroma_h = height.div_ceil(2) as usize;
        let uv_size = chroma_w * chroma_h;

        tracing::info!(
            width,
            height,
            preset = config.preset.min(13),
            crf = config.crf.clamp(1, 63),
            bitrate_kbps = config.bitrate_kbps,
            low_latency = config.low_latency,
            parallelism = config.parallelism,
            "SVT-AV1 encoder initialized"
        );

        Ok(Self {
            handle,
            width,
            height,
            next_pts: 0,
            y_plane: vec![0u8; y_size],
            u_plane: vec![0u8; uv_size],
            v_plane: vec![0u8; uv_size],
        })
    }

    /// Configure and initialize an encoder handle.  This is factored out of
    /// [`new`] so that the caller can release the handle on any error —
    /// avoiding native handle leaks when `set_param` or `svt_av1_enc_init`
    /// fails.
    #[allow(clippy::cast_possible_wrap)]
    fn configure_and_init(
        handle: *mut EbComponentType,
        enc_config: &mut EbSvtAv1EncConfiguration,
        width: u32,
        height: u32,
        config: &SvtAv1EncoderConfig,
    ) -> Result<(), String> {
        let preset = config.preset.min(13);
        let crf = config.crf.clamp(1, 63);

        set_param(enc_config, "preset", &preset.to_string())?;
        set_param(enc_config, "width", &width.to_string())?;
        set_param(enc_config, "height", &height.to_string())?;
        set_param(enc_config, "fps-num", "30")?;
        set_param(enc_config, "fps-denom", "1")?;
        set_param(enc_config, "input-depth", "8")?;
        set_param(enc_config, "color-format", "1")?; // YUV420

        // Keyframe interval: SVT-AV1 uses intra_period_length = interval - 1
        // (0 means every frame is a keyframe, -1 means no intra refresh, -2 = auto).
        let intra_period = if config.keyframe_interval == 0 {
            0_i32
        } else {
            i32::try_from(config.keyframe_interval).unwrap_or(i32::MAX) - 1
        };
        set_param(enc_config, "keyint", &intra_period.to_string())?;

        // Prediction structure: 1 = low-delay B, 2 = random access.
        let pred_struct = if config.low_latency { "1" } else { "2" };
        set_param(enc_config, "pred-struct", pred_struct)?;

        // Rate control.
        if config.bitrate_kbps == 0 {
            // CRF mode: rc 0 + adaptive quantization
            set_param(enc_config, "rc", "0")?;
            set_param(enc_config, "crf", &crf.to_string())?;
            set_param(enc_config, "aq-mode", "2")?;
        } else {
            // VBR mode
            set_param(enc_config, "rc", "1")?;
            let target_bps = u64::from(config.bitrate_kbps) * 1000;
            set_param(enc_config, "tbr", &target_bps.to_string())?;
        }

        // Thread parallelism.
        set_param(enc_config, "lp", &config.parallelism.to_string())?;

        // Apply configuration.
        // SAFETY: handle and config are valid; handle was created by init_handle.
        let ret = unsafe { svt_av1_ffi::svt_av1_enc_set_parameter(handle, &raw mut *enc_config) };
        if ret != EB_ERROR_NONE {
            return Err(format!("svt_av1_enc_set_parameter failed: error code {ret:#X}"));
        }

        // Initialize the encoder.
        // SAFETY: handle is valid and configured.
        let ret = unsafe { svt_av1_ffi::svt_av1_enc_init(handle) };
        if ret != EB_ERROR_NONE {
            return Err(format!("svt_av1_enc_init failed: error code {ret:#X}"));
        }

        Ok(())
    }

    /// Prepare and send a single frame to the encoder.
    ///
    /// Does NOT drain output — the receive thread handles that concurrently.
    fn send_frame(
        &mut self,
        frame: &VideoFrame,
        metadata: Option<&PacketMetadata>,
    ) -> Result<(), String> {
        if !matches!(frame.pixel_format, PixelFormat::I420 | PixelFormat::Nv12) {
            return Err(format!(
                "SVT-AV1 encoder expects I420 or NV12 input, got {:?}",
                frame.pixel_format
            ));
        }

        let layout = frame.layout();
        if frame.data_len() < layout.total_bytes() {
            return Err(format!(
                "SVT-AV1 encoder expected {} bytes, got {}",
                layout.total_bytes(),
                frame.data_len()
            ));
        }

        let width = frame.width as usize;
        let height = frame.height as usize;
        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);
        let data = frame.data.as_slice();
        let planes = layout.planes();

        // Fill reusable plane buffers with I420 data.
        match frame.pixel_format {
            PixelFormat::I420 => {
                for row in 0..height {
                    let src = planes[0].offset + row * planes[0].stride;
                    let dst = row * width;
                    self.y_plane[dst..dst + width].copy_from_slice(&data[src..src + width]);
                }
                for row in 0..chroma_h {
                    let src = planes[1].offset + row * planes[1].stride;
                    let dst = row * chroma_w;
                    self.u_plane[dst..dst + chroma_w].copy_from_slice(&data[src..src + chroma_w]);
                }
                for row in 0..chroma_h {
                    let src = planes[2].offset + row * planes[2].stride;
                    let dst = row * chroma_w;
                    self.v_plane[dst..dst + chroma_w].copy_from_slice(&data[src..src + chroma_w]);
                }
            },
            PixelFormat::Nv12 => {
                for row in 0..height {
                    let src = planes[0].offset + row * planes[0].stride;
                    let dst = row * width;
                    self.y_plane[dst..dst + width].copy_from_slice(&data[src..src + width]);
                }
                let uv_plane = &planes[1];
                for row in 0..chroma_h {
                    let src_start = uv_plane.offset + row * uv_plane.stride;
                    let dst_row = row * chroma_w;
                    for col in 0..chroma_w {
                        self.u_plane[dst_row + col] = data[src_start + col * 2];
                        self.v_plane[dst_row + col] = data[src_start + col * 2 + 1];
                    }
                }
            },
            _ => unreachable!("already checked above"),
        }

        let pts = self.next_pts(metadata);

        // Build the SVT-AV1 I/O format pointing at our plane buffers.
        #[allow(clippy::cast_possible_truncation)]
        let mut io_format = EbSvtIOFormat {
            luma: self.y_plane.as_mut_ptr(),
            cb: self.u_plane.as_mut_ptr(),
            cr: self.v_plane.as_mut_ptr(),
            y_stride: width as u32,
            cb_stride: chroma_w as u32,
            cr_stride: chroma_w as u32,
            width: self.width,
            height: self.height,
            org_x: 0,
            org_y: 0,
            color_fmt: EB_YUV420,
            bit_depth: 8,
        };

        // SVT-AV1 validates n_filled_len >= expected frame size.
        // For 8-bit YUV420: Y + U + V = w*h + w*h/4 + w*h/4 = w*h*3/2.
        #[allow(clippy::cast_possible_truncation)]
        let frame_size = (self.y_plane.len() + self.u_plane.len() + self.v_plane.len()) as u32;

        let mut buf_header = EbBufferHeaderType {
            // EbBufferHeaderType is always small enough for u32.
            #[allow(clippy::cast_possible_truncation)]
            size: std::mem::size_of::<EbBufferHeaderType>() as u32,
            p_buffer: std::ptr::from_mut(&mut io_format).cast::<u8>(),
            n_filled_len: frame_size,
            n_alloc_len: frame_size,
            p_app_private: std::ptr::null_mut(),
            wrapper_ptr: std::ptr::null_mut(),
            n_tick_count: 0,
            dts: 0,
            pts,
            qp: 0,
            pic_type: 0,
            luma_sse: 0,
            cr_sse: 0,
            cb_sse: 0,
            flags: 0,
            luma_ssim: 0.0,
            cr_ssim: 0.0,
            cb_ssim: 0.0,
            metadata: std::ptr::null_mut(),
        };

        // SAFETY: handle is valid, buf_header points to valid io_format with
        // valid plane pointers.
        let ret =
            unsafe { svt_av1_ffi::svt_av1_enc_send_picture(self.handle, &raw mut buf_header) };
        if ret != EB_ERROR_NONE {
            return Err(format!("svt_av1_enc_send_picture failed: error code {ret:#X}"));
        }

        Ok(())
    }

    fn next_pts(&mut self, metadata: Option<&PacketMetadata>) -> i64 {
        let duration = metadata.and_then(|meta| meta.duration_us).unwrap_or(1);

        let pts =
            metadata.and_then(|meta| meta.timestamp_us).map_or(self.next_pts, u64::cast_signed);

        self.next_pts = if duration > 0 { pts + duration.cast_signed() } else { pts + 1 };
        pts
    }
}

impl Drop for SvtAv1Encoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: handle is valid.  By the time we reach Drop, the send
            // thread has sent EOS and the receive thread has joined, so
            // deinit will not need to drain any remaining packets.
            unsafe {
                svt_av1_ffi::svt_av1_enc_deinit(self.handle);
                svt_av1_ffi::svt_av1_enc_deinit_handle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Send an EOS (end-of-stream) signal to the encoder.
///
/// The receive thread will see the `EB_BUFFERFLAG_EOS` flag on the last
/// output buffer and exit its loop.
fn send_eos(handle: *mut EbComponentType) {
    // EbBufferHeaderType is always small enough for u32.
    #[allow(clippy::cast_possible_truncation)]
    let mut eos_header = EbBufferHeaderType {
        size: std::mem::size_of::<EbBufferHeaderType>() as u32,
        p_buffer: std::ptr::null_mut(),
        n_filled_len: 0,
        n_alloc_len: 0,
        p_app_private: std::ptr::null_mut(),
        wrapper_ptr: std::ptr::null_mut(),
        n_tick_count: 0,
        dts: 0,
        pts: 0,
        qp: 0,
        pic_type: 0,
        luma_sse: 0,
        cr_sse: 0,
        cb_sse: 0,
        flags: EB_BUFFERFLAG_EOS,
        luma_ssim: 0.0,
        cr_ssim: 0.0,
        cb_ssim: 0.0,
        metadata: std::ptr::null_mut(),
    };

    // SAFETY: handle is valid.
    let ret = unsafe { svt_av1_ffi::svt_av1_enc_send_picture(handle, &raw mut eos_header) };
    if ret != EB_ERROR_NONE {
        tracing::warn!("svt_av1_enc_send_picture (EOS) failed: error code {ret:#X}");
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Set a single SVT-AV1 config parameter by name (string API).
fn set_param(config: &mut EbSvtAv1EncConfiguration, name: &str, value: &str) -> Result<(), String> {
    let c_name = CString::new(name).map_err(|_| format!("invalid parameter name: {name}"))?;
    let c_value =
        CString::new(value).map_err(|_| format!("invalid parameter value for {name}: {value}"))?;

    // SAFETY: config is a valid pointer, c_name and c_value are valid C strings.
    let ret = unsafe {
        svt_av1_ffi::svt_av1_enc_parse_parameter(config, c_name.as_ptr(), c_value.as_ptr())
    };
    if ret != EB_ERROR_NONE {
        return Err(format!(
            "svt_av1_enc_parse_parameter({name}={value}) failed: error code {ret:#X}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

use schemars::schema_for;
use streamkit_core::registry::StaticPins;

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_svt_av1_nodes(registry: &mut NodeRegistry) {
    let default_encoder = SvtAv1EncoderNode::new(SvtAv1EncoderConfig::default())
        .expect("default SVT-AV1 encoder config should be valid");
    registry.register_static_with_description(
        "video::svt_av1::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(SvtAv1EncoderNode::new(config)?))
        },
        serde_json::to_value(schema_for!(SvtAv1EncoderConfig))
            .expect("SvtAv1EncoderConfig schema should serialize to JSON"),
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        vec!["video".to_string(), "codecs".to_string(), "av1".to_string()],
        false,
        "Encodes raw video frames (NV12 or I420) into AV1 packets using SVT-AV1 (Intel/AOMedia). \
         Higher performance than the rav1e encoder, especially at presets 10-13. \
         Insert a video::pixel_convert node upstream if the source outputs RGBA8.",
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
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_svt_av1_encode_basic() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder_config = SvtAv1EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 0,
            preset: 12,
            crf: 35,
            parallelism: 1,
            low_latency: true,
        };
        let encoder = SvtAv1EncoderNode::new(encoder_config).unwrap();

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
        assert!(!encoded_packets.is_empty(), "SVT-AV1 encoder produced no packets");

        // Verify all output packets are Binary with the expected content type.
        for packet in &encoded_packets {
            match packet {
                Packet::Binary { content_type, data, .. } => {
                    assert_eq!(
                        content_type.as_deref(),
                        Some(AV1_CONTENT_TYPE),
                        "Expected content_type 'video/av1'"
                    );
                    assert!(!data.is_empty(), "Encoded packet data should not be empty");
                },
                _ => panic!("Expected Binary packet from SVT-AV1 encoder"),
            }
        }
    }

    #[tokio::test]
    async fn test_svt_av1_encode_i420_input() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(10);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 10);
        let encoder = SvtAv1EncoderNode::new(SvtAv1EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 0,
            preset: 12,
            crf: 35,
            parallelism: 1,
            low_latency: true,
        })
        .unwrap();

        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        for index in 0_u64..3 {
            let mut frame = create_test_video_frame(64, 64, PixelFormat::I420, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * index),
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
        assert!(!encoded_packets.is_empty(), "SVT-AV1 encoder should encode I420 input");
    }

    #[tokio::test]
    async fn test_svt_av1_encode_many_frames() {
        const FRAME_COUNT: u64 = 10;

        let (enc_input_tx, enc_input_rx) = mpsc::channel(32);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 32);
        let encoder = SvtAv1EncoderNode::new(SvtAv1EncoderConfig {
            keyframe_interval: 10,
            bitrate_kbps: 0,
            preset: 12,
            crf: 35,
            parallelism: 1,
            low_latency: true,
        })
        .unwrap();
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
            "SVT-AV1 encoder produced no packets for {FRAME_COUNT} input frames"
        );
    }
}
