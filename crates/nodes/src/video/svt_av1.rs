// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! SVT-AV1 video encoder node (CPU).
//!
//! Uses [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (≥ 4.0) via
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
use schemars::JsonSchema;
use serde::Deserialize;
use std::ffi::CString;
use std::time::{Duration, Instant};
use streamkit_core::types::{
    EncodedVideoFormat, PacketMetadata, PacketType, PixelFormat, RawVideoFormat, VideoCodec,
    VideoFrame,
};
use streamkit_core::{
    config_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode,
    StreamKitError,
};
use tokio::sync::mpsc;

use crate::codec_utils::{bounded_thread_join, ThreadJoin};

use super::svt_av1_ffi::{
    self, EbBufferHeaderType, EbComponentType, EbSvtAv1EncConfiguration, EbSvtIOFormat,
    EB_AV1_KEY_PICTURE, EB_BUFFERFLAG_EOS, EB_ERROR_NONE, EB_NO_ERROR_EMPTY_QUEUE,
};

use super::AV1_CONTENT_TYPE;

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
/// Default frame rate (matches rav1e encoder default).
const SVT_AV1_DEFAULT_FPS: u32 = 30;

// Configuration struct
#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
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
    /// Frame rate in fps.  Used by SVT-AV1's rate-control and bitrate
    /// allocation — should match the actual source frame rate for accurate
    /// rate control.  Default: 30.
    pub fps: u32,
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
            fps: SVT_AV1_DEFAULT_FPS,
        }
    }
}

// Encoder node
pub struct SvtAv1EncoderNode {
    config: SvtAv1EncoderConfig,
}

impl SvtAv1EncoderNode {
    pub const fn new(config: SvtAv1EncoderConfig) -> Self {
        Self { config }
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

    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        encoder_trait::run_encoder(*self, context).await
    }
}

impl EncoderNodeRunner for SvtAv1EncoderNode {
    const CONTENT_TYPE: &'static str = AV1_CONTENT_TYPE;
    const NODE_LABEL: &'static str = "SvtAv1EncoderNode";
    const PACKETS_COUNTER_NAME: &'static str = "svt_av1_encoder_packets_processed";
    const DURATION_HISTOGRAM_NAME: &'static str = "svt_av1_send_duration";

    fn spawn_codec_task(
        self,
        mut encode_rx: mpsc::Receiver<(VideoFrame, Option<PacketMetadata>)>,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
        send_duration_histogram: opentelemetry::metrics::Histogram<f64>,
    ) -> tokio::task::JoinHandle<()> {
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
        tokio::task::spawn_blocking(move || {
            let mut encoder: Option<SvtAv1Encoder> = None;
            let mut current_dimensions: Option<(u32, u32)> = None;
            let mut recv_thread: Option<ReceiveThread> = None;
            // Monitor send_picture latency as a proxy for encoder saturation.
            // The actual encode happens asynchronously on SVT-AV1's internal
            // threads; send_picture blocks when the input FIFO is full, which
            // correlates with the encoder falling behind real-time.
            let mut budget_monitor = encoder_trait::FrameBudgetMonitor::new("SVT-AV1");

            while let Some((frame, metadata)) = encode_rx.blocking_recv() {
                if result_tx.is_closed() {
                    // Drain the receive thread before returning so the encoder
                    // handle is not freed while it is still in use.
                    if let Some(enc) = encoder.take() {
                        Self::flush_and_join(enc, recv_thread.take());
                    }
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
                    // Flush + stop the old receive thread before re-creating the
                    // encoder at the new resolution.
                    if let Some(old_encoder) = encoder.take() {
                        Self::flush_and_join(old_encoder, recv_thread.take());
                    }

                    match SvtAv1Encoder::new(frame.width, frame.height, &encoder_config) {
                        Ok(new_encoder) => {
                            recv_thread = Some(ReceiveThread::spawn(
                                SendableHandle(new_encoder.handle),
                                result_tx.clone(),
                            ));
                            encoder = Some(new_encoder);
                            current_dimensions = Some(frame_dimensions);
                        },
                        Err(err) => {
                            current_dimensions = None;
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

                let frame_duration_us = metadata.as_ref().and_then(|m| m.duration_us);
                let send_start_time = Instant::now();
                let result = enc.send_frame(&frame, metadata.as_ref());
                let send_elapsed = send_start_time.elapsed();
                send_duration_histogram.record(send_elapsed.as_secs_f64(), &[]);
                budget_monitor.record(send_elapsed, frame_duration_us);

                if let Err(err) = result {
                    let _ = result_tx.blocking_send(Err(err));
                }
            }

            // Input channel closed — flush the encoder, bounded like every
            // other flush site so a wedged native flush can't hang the task.
            if let Some(enc) = encoder.take() {
                Self::flush_and_join(enc, recv_thread.take());
            }
        })
    }
}

// Internal codec types
use super::encoder_trait::{self, EncodedPacket, EncoderNodeRunner};

/// Wrapper around `*mut EbComponentType` that can be sent across threads.
///
/// SVT-AV1's encoder handle is designed for concurrent access from separate
/// `send_picture` and `get_packet` threads — the library provides internal
/// synchronisation via FIFOs and semaphores.  This newtype makes the
/// `Send` impl explicit and avoids the raw `handle as usize` pattern.
#[derive(Clone, Copy)]
struct SendableHandle(*mut EbComponentType);

// SAFETY: SVT-AV1's encoder handle uses internal locking; concurrent
// `send_picture` / `get_packet` from different threads is the intended
// usage pattern (see `SvtAv1EncApp`).
unsafe impl Send for SendableHandle {}

/// The OS thread that drains encoded packets out of the encoder via
/// `get_packet`. Owned (and joined) by the flush helper thread.
struct ReceiveThread {
    handle: std::thread::JoinHandle<()>,
}

impl ReceiveThread {
    fn spawn(
        handle: SendableHandle,
        result_tx: mpsc::Sender<Result<EncodedPacket, String>>,
    ) -> Self {
        let join_handle = std::thread::spawn(move || {
            receive_loop(handle, &result_tx);
        });
        Self { handle: join_handle }
    }
}

impl SvtAv1EncoderNode {
    /// Deadline for a blocking encoder flush. The single bound for every
    /// SVT-AV1 flush site — mid-stream dimension change, downstream-close, and
    /// input-close (#540).
    const RECEIVE_THREAD_JOIN_TIMEOUT: Duration = Duration::from_secs(30);

    /// Flush `encoder` (send EOS), drain its receive thread, and drop the
    /// encoder — the whole sequence bounded by
    /// [`Self::RECEIVE_THREAD_JOIN_TIMEOUT`].
    ///
    /// SVT-AV1 flushes through a blocking two-thread FFI boundary
    /// (`send_picture` + `get_packet`) that, under rare scheduling, can
    /// deadlock inside the library (#537). *Both* halves can wedge — `send_eos`
    /// on a full input FIFO and `get_packet` on the receive thread — so the
    /// entire flush runs on a helper thread that this call joins with a bound.
    /// That is the single mechanism that lets the codec task finalize instead
    /// of hanging to the request timeout (#540); bounding only the
    /// receive-thread join would leave a wedged `send_eos` unguarded.
    ///
    /// A healthy flush drains in well under a second. If the helper thread is
    /// still wedged after the budget it is abandoned (its `JoinHandle` dropped,
    /// detaching it). Because that thread **owns** the encoder and its receive
    /// thread, abandoning it leaks both: the encoder handle is never freed, so
    /// the still-running native calls cannot hit a use-after-free (running
    /// `Drop`'s `deinit` + `deinit_handle` would). Leaking native resources is
    /// the deliberate, lesser evil (the tradeoff raised in #539); the stall is
    /// logged at error level. An abandoned receive thread also keeps its
    /// `result_tx` clone alive forever; the input-close drain closes its
    /// receiver once the codec task ends (see `drain_codec_results`) so that
    /// leaked sender can't stall it.
    fn flush_and_join(encoder: SvtAv1Encoder, recv_thread: Option<ReceiveThread>) {
        let (done_tx, done) = std::sync::mpsc::channel::<()>();
        let flush = std::thread::spawn(move || {
            send_eos(encoder.handle);
            if let Some(rt) = recv_thread {
                let _ = rt.handle.join();
            }
            // The receive thread has exited and no longer touches the handle,
            // so dropping the encoder (deinit + deinit_handle) is safe.
            drop(encoder);
            let _ = done_tx.send(());
        });

        if bounded_thread_join(&done, flush, Self::RECEIVE_THREAD_JOIN_TIMEOUT)
            == ThreadJoin::Abandoned
        {
            tracing::error!(
                "SVT-AV1 EOS flush did not complete within {:?}; abandoning the flush \
                 and leaking the encoder handle to avoid a use-after-free \
                 (likely native encoder deadlock)",
                Self::RECEIVE_THREAD_JOIN_TIMEOUT
            );
        }
    }
}

// Receive loop (runs on its own OS thread)
/// Blocking receive loop: calls `svt_av1_enc_get_packet` in a loop until
/// the encoder signals EOS or `EB_NoErrorEmptyQueue`.
///
/// SVT-AV1 in low-delay mode uses a blocking FIFO pop for `get_packet`,
/// so this thread will sleep until encoded data is available — it does NOT
/// busy-wait.
///
/// The caller must ensure the encoder handle remains live for the duration
/// of this function (i.e. do not drop the `SvtAv1Encoder` before joining
/// the receive thread).
fn receive_loop(handle: SendableHandle, result_tx: &mpsc::Sender<Result<EncodedPacket, String>>) {
    let handle = handle.0;

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
            let is_keyframe = buf.pic_type == EB_AV1_KEY_PICTURE;
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

// SVT-AV1 encoder wrapper (send-side only)
struct SvtAv1Encoder {
    handle: *mut EbComponentType,
    next_pts: i64,
    /// Reusable Y plane buffer.
    y_plane: Vec<u8>,
    /// Reusable U plane buffer.
    u_plane: Vec<u8>,
    /// Reusable V plane buffer.
    v_plane: Vec<u8>,
}

// SAFETY: The SVT-AV1 encoder handle uses internal locking for thread-safety.
// `SvtAv1Encoder` is only used from the send thread (inside `spawn_blocking`);
// the handle pointer is shared with the receive thread via `SendableHandle`.
unsafe impl Send for SvtAv1Encoder {}

impl SvtAv1Encoder {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn new(width: u32, height: u32, config: &SvtAv1EncoderConfig) -> Result<Self, String> {
        let mut enc_config = EbSvtAv1EncConfiguration::zeroed();
        let mut handle: *mut EbComponentType = std::ptr::null_mut();

        // Step 1: Init handle — fills enc_config with defaults.
        // SAFETY: We pass valid pointers.  `handle` is written by the library.
        // NOTE: SVT-AV1 4.x removed the `p_app_data` parameter from the 2.x API.
        let ret =
            unsafe { svt_av1_ffi::svt_av1_enc_init_handle(&raw mut handle, &raw mut enc_config) };
        if ret != EB_ERROR_NONE {
            return Err(format!("svt_av1_enc_init_handle failed: error code {ret:#X}"));
        }

        // From here on, if anything fails we must call deinit_handle to avoid
        // leaking the native encoder handle.  We use a helper closure so that
        // all error paths are covered by a single cleanup call.
        let configure_result =
            Self::configure_and_init(handle, &mut enc_config, width, height, config);
        if let Err(err) = configure_result {
            tracing::debug!("SVT-AV1 init failed ({err}), releasing handle");
            // SAFETY: handle was successfully created by init_handle above.
            unsafe { svt_av1_ffi::svt_av1_enc_deinit_handle(handle) };
            return Err(err);
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
        // The two-thread architecture (send thread + receive thread) relies on
        // SVT-AV1's blocking `get_packet` in low-delay mode.  In random-access
        // mode (`pred_struct=2`), `get_packet` is non-blocking and returns
        // `EB_NO_ERROR_EMPTY_QUEUE` immediately when the output queue is empty,
        // which would cause the receive thread to exit prematurely.
        if !config.low_latency {
            return Err("SVT-AV1 encoder currently only supports low_latency=true (low-delay \
                 prediction structure). Random-access mode (low_latency=false) is not \
                 supported because the receive thread relies on blocking get_packet."
                .to_string());
        }

        let preset = config.preset.min(13);
        if preset != config.preset {
            tracing::warn!(
                requested = config.preset,
                clamped = preset,
                "SVT-AV1 preset clamped to valid range 0–13"
            );
        }
        let crf = config.crf.clamp(1, 63);
        if crf != config.crf {
            tracing::warn!(
                requested = config.crf,
                clamped = crf,
                "SVT-AV1 CRF clamped to valid range 1–63"
            );
        }

        set_param(enc_config, "preset", &preset.to_string())?;
        set_param(enc_config, "width", &width.to_string())?;
        set_param(enc_config, "height", &height.to_string())?;
        let fps = config.fps.max(1);
        set_param(enc_config, "fps-num", &fps.to_string())?;
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
            temporal_layer_index: 0,
            qp: 0,
            avg_qp: 0,
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

        // Clamp u64 timestamps to i64::MAX to avoid sign flip.
        let pts = metadata
            .and_then(|meta| meta.timestamp_us)
            .map_or(self.next_pts, |ts| i64::try_from(ts).unwrap_or(i64::MAX));

        self.next_pts = if duration > 0 {
            let d = i64::try_from(duration).unwrap_or(i64::MAX);
            pts.saturating_add(d)
        } else {
            pts.saturating_add(1)
        };
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
        temporal_layer_index: 0,
        qp: 0,
        avg_qp: 0,
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

// Helpers
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

// Registration
use streamkit_core::registry::StaticPins;

pub fn register_svt_av1_nodes(registry: &mut NodeRegistry) {
    let default_encoder = SvtAv1EncoderNode::new(SvtAv1EncoderConfig::default());
    register_static_node!(
        registry,
        "video::svt_av1::encoder",
        |params| {
            let config = config_helpers::parse_config_optional(params)?;
            Ok(Box::new(SvtAv1EncoderNode::new(config)))
        },
        SvtAv1EncoderConfig,
        StaticPins { inputs: default_encoder.input_pins(), outputs: default_encoder.output_pins() },
        ["video", "codecs", "av1"],
        "Encodes raw video frames (NV12 or I420) into AV1 packets using SVT-AV1 (Intel/AOMedia). \
         Higher performance than the rav1e encoder, especially at presets 10-13. \
         Insert a video::pixel_convert node upstream if the source outputs RGBA8.",
    );
}

// Tests
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped, create_test_context,
        create_test_video_frame,
    };
    use std::collections::HashMap;
    use streamkit_core::types::Packet;
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
            fps: 30,
        };
        let encoder = SvtAv1EncoderNode::new(encoder_config);

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
            fps: 30,
        });

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
            fps: 30,
        });
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

    /// Regression for #540: a mid-stream resolution change triggers an in-task
    /// flush + receive-thread teardown of the old encoder. This drives that
    /// path through the real encoder and asserts the node finalizes (rather
    /// than hanging) and keeps producing packets across the change.
    #[tokio::test]
    async fn test_svt_av1_encode_dimension_change() {
        let (enc_input_tx, enc_input_rx) = mpsc::channel(16);
        let mut enc_inputs = HashMap::new();
        enc_inputs.insert("in".to_string(), enc_input_rx);

        let (enc_context, enc_sender, mut enc_state_rx) = create_test_context(enc_inputs, 16);
        let encoder = SvtAv1EncoderNode::new(SvtAv1EncoderConfig {
            keyframe_interval: 1,
            bitrate_kbps: 0,
            preset: 12,
            crf: 35,
            parallelism: 1,
            low_latency: true,
            fps: 30,
        });
        let enc_handle = tokio::spawn(async move { Box::new(encoder).run(enc_context).await });

        assert_state_initializing(&mut enc_state_rx).await;
        assert_state_running(&mut enc_state_rx).await;

        // Two frames at 64x64, then two at 128x128 to force a mid-stream
        // re-create (and the old encoder's flush/join), then back to 64x64.
        let dims = [(64, 64), (64, 64), (128, 128), (128, 128), (64, 64)];
        for (index, (width, height)) in dims.into_iter().enumerate() {
            let mut frame = create_test_video_frame(width, height, PixelFormat::Nv12, 16);
            frame.metadata = Some(PacketMetadata {
                timestamp_us: Some(33_333 * index as u64),
                duration_us: Some(33_333),
                sequence: Some(index as u64),
                keyframe: Some(true),
            });
            enc_input_tx.send(Packet::Video(frame)).await.unwrap();
        }
        drop(enc_input_tx);

        assert_state_stopped(&mut enc_state_rx).await;
        enc_handle.await.unwrap().unwrap();

        let encoded_packets = enc_sender.get_packets_for_pin("out").await;
        assert!(
            !encoded_packets.is_empty(),
            "SVT-AV1 encoder produced no packets across a dimension change"
        );
    }
}
