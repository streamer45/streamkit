// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Custom VA-API H.264 encoder shim.
//!
//! Drives `libva` directly for H.264 encoding — no dependency on the
//! `cros-codecs` encoder infrastructure.  This lets the H.264 encoder use the
//! VA-API Image API path (upload NV12 via `vaCreateImage`/`vaPutImage`) instead
//! of GBM buffer allocation, which Mesa's iris driver rejects for NV12 on some
//! hardware (e.g. Intel Tiger Lake with Mesa ≤ 23.x).
//!
//! The encoder implements a simple IPP low-delay prediction structure:
//! periodic IDR keyframes with single-reference P frames in between.
//!
//! # Why not use cros-codecs?
//!
//! `cros_codecs::encoder::stateless::StatelessEncoder::new_vaapi()` requires
//! the input frame type to implement `VideoFrame`, which in turn requires
//! `Send + Sync`.  `libva::Surface<()>` contains `Rc<Display>` and therefore
//! cannot satisfy those bounds.  The only workaround was to call the
//! crate-private `new_h264()` constructor, requiring either a vendored copy or
//! a fork.  This shim avoids that entirely by calling `libva` directly.

use std::rc::Rc;

use cros_codecs::libva::{
    self, BufferType, Context, Display, EncCodedBuffer, EncMiscParameter,
    EncMiscParameterFrameRate, EncMiscParameterRateControl, EncPictureParameter,
    EncPictureParameterBufferH264, EncSequenceParameter, EncSequenceParameterBufferH264,
    EncSliceParameter, EncSliceParameterBufferH264, H264EncFrameCropOffsets, H264EncPicFields,
    H264EncSeqFields, H264VuiFields, MappedCodedBuffer, Picture, PictureH264, RcFlags, Surface,
    UsageHint, VAEntrypoint, VAProfile, VA_INVALID_ID, VA_PICTURE_H264_INVALID,
    VA_PICTURE_H264_SHORT_TERM_REFERENCE, VA_RT_FORMAT_YUV420,
};

use super::vaapi_av1::write_nv12_to_va_surface;
use streamkit_core::types::{PixelFormat, VideoFrame};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// H.264 macroblock size in pixels.
const MB_SIZE: u32 = 16;

/// Minimum QP value (H.264 spec).
const MIN_QP: u32 = 1;

/// Maximum QP value (H.264 spec).
const MAX_QP: u32 = 51;

/// Initial scratch surface pool size for reconstructed reference frames.
const SCRATCH_POOL_SIZE: usize = 4;

/// Default coded buffer size when bitrate is not explicitly set (CQP mode).
const DEFAULT_CODED_BUF_SIZE: usize = 1_500_000;

/// H.264 slice type constants (Table 7-6).
const SLICE_TYPE_P: u8 = 0;
const SLICE_TYPE_I: u8 = 2;

// ---------------------------------------------------------------------------
// Encoder configuration
// ---------------------------------------------------------------------------

/// Configuration for the custom VA-API H.264 encoder.
pub(super) struct H264EncConfig {
    /// Display (visible) width.
    pub width: u32,
    /// Display (visible) height.
    pub height: u32,
    /// Constant quality parameter (0–51).
    pub quality: u32,
    /// Framerate in FPS (used for rate-control hints and VUI timing).
    pub framerate: u32,
    /// Use low-power encoding entrypoint if `true`.
    pub low_power: bool,
}

// ---------------------------------------------------------------------------
// Reference frame tracking
// ---------------------------------------------------------------------------

/// Metadata for a reference frame in the DPB.
struct RefPic {
    /// VA surface used as the reconstructed reference.
    surface: Surface<()>,
    /// Picture order count.
    poc: u16,
    /// `frame_num` in the H.264 bitstream.
    frame_num: u32,
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// A self-contained VA-API H.264 encoder.
///
/// Manages its own VA config, context, scratch surface pool, and reference
/// frame.  Call [`encode_frame`] for each input frame and collect the returned
/// bitstream bytes.
pub(super) struct VaH264Encoder {
    display: Rc<Display>,
    context: Rc<Context>,

    /// Display (visible) resolution.
    width: u32,
    height: u32,

    /// Macroblock-aligned coded resolution.
    coded_width: u32,
    coded_height: u32,

    /// Pool of scratch surfaces for reconstructed reference frames.
    scratch_surfaces: Vec<Surface<()>>,

    /// Current reference frame (most recent reconstructed P or I frame).
    reference: Option<RefPic>,

    /// Monotonically increasing frame counter.
    frame_count: u64,

    /// IDR period — number of frames between IDR keyframes.
    idr_period: u32,

    /// Constant quality parameter (QP).
    qp: u32,

    /// Framerate for rate-control / VUI.
    framerate: u32,

    /// Number of macroblocks per frame.
    num_mbs: u32,

    /// Width in macroblocks.
    width_in_mbs: u16,

    /// Height in macroblocks.
    height_in_mbs: u16,

    /// Frame cropping offsets (for non-MB-aligned resolutions).
    frame_crop: Option<H264EncFrameCropOffsets>,

    /// `log2_max_frame_num_minus4` derived from `idr_period`.
    log2_max_frame_num_minus4: u32,

    /// `log2_max_pic_order_cnt_lsb_minus4` derived from `idr_period`.
    log2_max_pic_order_cnt_lsb_minus4: u32,
}

impl VaH264Encoder {
    /// Create a new encoder.
    ///
    /// Opens the VA display, creates config + context, and pre-allocates
    /// scratch surfaces for reference frame reconstruction.
    pub fn new(display: Rc<Display>, config: &H264EncConfig) -> Result<Self, String> {
        let coded_width = align_up(config.width, MB_SIZE);
        let coded_height = align_up(config.height, MB_SIZE);

        let low_power = resolve_low_power(&display, config.low_power)?;

        let entrypoint = if low_power {
            VAEntrypoint::VAEntrypointEncSliceLP
        } else {
            VAEntrypoint::VAEntrypointEncSlice
        };

        let va_config = display
            .create_config(
                vec![
                    libva::VAConfigAttrib {
                        type_: libva::VAConfigAttribType::VAConfigAttribRTFormat,
                        value: VA_RT_FORMAT_YUV420,
                    },
                    libva::VAConfigAttrib {
                        type_: libva::VAConfigAttribType::VAConfigAttribRateControl,
                        value: libva::VA_RC_CQP,
                    },
                ],
                VAProfile::VAProfileH264Main,
                entrypoint,
            )
            .map_err(|e| format!("failed to create VA config: {e}"))?;

        let context = display
            .create_context::<()>(&va_config, coded_width, coded_height, None, true)
            .map_err(|e| format!("failed to create VA context: {e}"))?;

        // Pre-allocate scratch surfaces for reference frame reconstruction.
        let scratch_surfaces = display
            .create_surfaces(
                VA_RT_FORMAT_YUV420,
                None,
                coded_width,
                coded_height,
                Some(UsageHint::USAGE_HINT_ENCODER),
                vec![(); SCRATCH_POOL_SIZE],
            )
            .map_err(|e| format!("failed to create scratch surfaces: {e}"))?;

        let width_in_mbs = (coded_width / MB_SIZE) as u16;
        let height_in_mbs = (coded_height / MB_SIZE) as u16;
        let num_mbs = u32::from(width_in_mbs) * u32::from(height_in_mbs);

        // Compute frame cropping if the display resolution is not MB-aligned.
        let frame_crop = if coded_width != config.width || coded_height != config.height {
            // H.264 spec: crop offsets are in units of 2 pixels for 4:2:0.
            let right = (coded_width - config.width) / 2;
            let bottom = (coded_height - config.height) / 2;
            Some(H264EncFrameCropOffsets::new(0, right, 0, bottom))
        } else {
            None
        };

        // IDR period: one keyframe every 1024 frames (~34 s at 30 fps).
        // Not yet exposed in H264EncConfig; a reasonable default for
        // low-latency streaming.  Callers can still force an IDR at any
        // time via the `force_keyframe` parameter on `encode_frame`.
        let idr_period: u32 = 1024;
        let qp = config.quality.clamp(MIN_QP, MAX_QP);

        // Compute log2 values for max_frame_num and max_pic_order_cnt_lsb.
        let log2_max_frame_num_minus4 = log2_ceil(idr_period).saturating_sub(4);
        let log2_max_pic_order_cnt_lsb_minus4 = log2_ceil(idr_period * 2).saturating_sub(4);

        Ok(Self {
            display,
            context,
            width: config.width,
            height: config.height,
            coded_width,
            coded_height,
            scratch_surfaces,
            reference: None,
            frame_count: 0,
            idr_period,
            qp,
            framerate: config.framerate,
            num_mbs,
            width_in_mbs,
            height_in_mbs,
            frame_crop,
            log2_max_frame_num_minus4,
            log2_max_pic_order_cnt_lsb_minus4,
        })
    }

    /// Encode a single frame, returning the H.264 bitstream bytes.
    ///
    /// The caller is responsible for providing NV12 or I420 pixel data in the
    /// `VideoFrame`.  The frame is uploaded to a VA surface via the Image API
    /// before encoding.
    pub fn encode_frame(
        &mut self,
        frame: &VideoFrame,
        force_keyframe: bool,
    ) -> Result<Vec<u8>, String> {
        if frame.pixel_format == PixelFormat::Rgba8 {
            return Err("VA-API H.264 encoder requires NV12 or I420 input; \
                 insert a video::pixel_convert node upstream"
                .into());
        }

        // Determine whether this frame is an IDR.
        let frame_in_gop = (self.frame_count % u64::from(self.idr_period)) as u32;
        let is_idr = self.frame_count == 0 || frame_in_gop == 0 || force_keyframe;
        let is_i_frame = is_idr;

        // Reset reference on IDR.
        if is_idr {
            self.reference = None;
        }

        let frame_num = if is_idr { 0 } else { frame_in_gop };
        let poc = ((frame_num * 2) & 0xFFFF) as u16;

        // Create input surface and upload pixel data via Image API.
        let nv12_fourcc: u32 = super::vaapi_av1::nv12_fourcc().into();
        let mut input_surfaces = self
            .display
            .create_surfaces(
                VA_RT_FORMAT_YUV420,
                Some(nv12_fourcc),
                self.coded_width,
                self.coded_height,
                Some(UsageHint::USAGE_HINT_ENCODER),
                vec![()],
            )
            .map_err(|e| format!("failed to create input surface: {e}"))?;
        let input_surface =
            input_surfaces.pop().ok_or_else(|| "create_surfaces returned empty vec".to_string())?;

        write_nv12_to_va_surface(&self.display, &input_surface, frame)?;

        // Get a scratch surface for the reconstructed frame.
        let recon_surface = self.get_scratch_surface()?;

        // Create coded buffer.
        let coded_buf = self
            .context
            .create_enc_coded(DEFAULT_CODED_BUF_SIZE)
            .map_err(|e| format!("failed to create coded buffer: {e}"))?;

        // Build VA parameter buffers.
        let seq_param = self.build_seq_param();
        let pic_param =
            self.build_pic_param(is_idr, &recon_surface, &coded_buf, frame_num as u16, poc);
        let slice_param = self.build_slice_param(is_i_frame, is_idr, poc, frame_num);
        let rc_param = self.build_rc_param();
        let framerate_param = BufferType::EncMiscParameter(EncMiscParameter::FrameRate(
            EncMiscParameterFrameRate::new(self.framerate, 0),
        ));

        // Create picture, attach buffers, and submit.
        let mut picture = Picture::new(self.frame_count, Rc::clone(&self.context), input_surface);

        picture.add_buffer(
            self.context
                .create_buffer(seq_param)
                .map_err(|e| format!("failed to create seq param buffer: {e}"))?,
        );
        picture.add_buffer(
            self.context
                .create_buffer(pic_param)
                .map_err(|e| format!("failed to create pic param buffer: {e}"))?,
        );
        picture.add_buffer(
            self.context
                .create_buffer(slice_param)
                .map_err(|e| format!("failed to create slice param buffer: {e}"))?,
        );
        picture.add_buffer(
            self.context
                .create_buffer(rc_param)
                .map_err(|e| format!("failed to create rc param buffer: {e}"))?,
        );
        picture.add_buffer(
            self.context
                .create_buffer(framerate_param)
                .map_err(|e| format!("failed to create framerate param buffer: {e}"))?,
        );

        let picture = picture.begin().map_err(|e| format!("vaBeginPicture failed: {e}"))?;
        let picture = picture.render().map_err(|e| format!("vaRenderPicture failed: {e}"))?;
        let picture = picture.end().map_err(|e| format!("vaEndPicture failed: {e}"))?;

        // Sync and read coded output.
        let _synced = picture.sync().map_err(|(e, _)| format!("vaSyncSurface failed: {e}"))?;

        let mapped = MappedCodedBuffer::new(&coded_buf)
            .map_err(|e| format!("failed to map coded buffer: {e}"))?;

        let mut coded_data = Vec::new();
        for segment in mapped.segments() {
            coded_data.extend_from_slice(segment.buf);
        }

        // For IDR frames, ensure SPS/PPS NALUs are present in the bitstream.
        // Some VA-API drivers (notably Intel iHD) do not auto-generate SPS/PPS
        // in the coded output — the `cros-libva` crate does not expose packed
        // header buffer types, so we cannot request them via the VA-API.
        // Instead we generate the NALUs ourselves and prepend them.
        let bitstream = if is_idr {
            let has_sps_pps = bitstream_contains_sps_pps(&coded_data);
            if has_sps_pps {
                tracing::debug!("IDR frame already contains SPS/PPS from driver");
                coded_data
            } else {
                tracing::debug!("IDR frame missing SPS/PPS — prepending generated NALUs");
                let mut out = Vec::with_capacity(coded_data.len() + 128);
                out.extend_from_slice(&self.build_sps_nalu());
                out.extend_from_slice(&self.build_pps_nalu());
                out.extend_from_slice(&coded_data);
                out
            }
        } else {
            coded_data
        };

        // Update reference frame, returning the old surface to the pool.
        if let Some(old_ref) = self.reference.take() {
            self.scratch_surfaces.push(old_ref.surface);
        }
        self.reference = Some(RefPic { surface: recon_surface, poc, frame_num });

        self.frame_count += 1;

        Ok(bitstream)
    }

    /// Flush the encoder — no-op for this synchronous implementation.
    ///
    /// Each `encode_frame` call produces output immediately, so there is
    /// nothing to drain.
    pub fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Get a scratch surface for the reconstructed reference frame.
    ///
    /// Rotates through the pre-allocated pool.
    fn get_scratch_surface(&mut self) -> Result<Surface<()>, String> {
        if self.scratch_surfaces.is_empty() {
            // Replenish the pool.
            let new_surfaces = self
                .display
                .create_surfaces(
                    VA_RT_FORMAT_YUV420,
                    None,
                    self.coded_width,
                    self.coded_height,
                    Some(UsageHint::USAGE_HINT_ENCODER),
                    vec![(); SCRATCH_POOL_SIZE],
                )
                .map_err(|e| format!("failed to replenish scratch surfaces: {e}"))?;
            self.scratch_surfaces = new_surfaces;
        }
        self.scratch_surfaces.pop().ok_or_else(|| "scratch surface pool exhausted".to_string())
    }

    /// Build the sequence parameter buffer (SPS-derived fields).
    fn build_seq_param(&self) -> BufferType {
        let seq_fields = H264EncSeqFields::new(
            1, // chroma_format_idc = 1 (4:2:0)
            1, // frame_mbs_only_flag
            0, // mb_adaptive_frame_field_flag
            0, // seq_scaling_matrix_present_flag
            1, // direct_8x8_inference_flag (required for Level >= 3.0)
            self.log2_max_frame_num_minus4,
            0, // pic_order_cnt_type = 0
            self.log2_max_pic_order_cnt_lsb_minus4,
            0, // delta_pic_order_always_zero_flag
        );

        let vui_fields = H264VuiFields::new(
            1, // aspect_ratio_info_present_flag
            1, // timing_info_present_flag
            0, // bitstream_restriction_flag
            0, // log2_max_mv_length_horizontal
            0, // log2_max_mv_length_vertical
            0, // fixed_frame_rate_flag
            0, // low_delay_hrd_flag
            0, // motion_vectors_over_pic_boundaries_flag
        );

        BufferType::EncSequenceParameter(EncSequenceParameter::H264(
            EncSequenceParameterBufferH264::new(
                0,                  // seq_parameter_set_id
                41,                 // level_idc (Level 4.1)
                self.idr_period,    // intra_period
                self.idr_period,    // intra_idr_period
                0,                  // ip_period (no B frames)
                0,                  // bits_per_second (CQP mode)
                1,                  // max_num_ref_frames
                self.width_in_mbs,  // picture_width_in_mbs
                self.height_in_mbs, // picture_height_in_mbs
                &seq_fields,
                0,           // bit_depth_luma_minus8
                0,           // bit_depth_chroma_minus8
                0,           // num_ref_frames_in_pic_order_cnt_cycle
                0,           // offset_for_non_ref_pic
                0,           // offset_for_top_to_bottom_field
                [0i32; 256], // offset_for_ref_frame
                self.frame_crop
                    .as_ref()
                    .map(|c| H264EncFrameCropOffsets::new(c.left, c.right, c.top, c.bottom)), // frame_crop
                Some(vui_fields),   // vui_fields
                1,                  // aspect_ratio_idc (1:1 SAR)
                1,                  // sar_width
                1,                  // sar_height
                1,                  // num_units_in_tick
                self.framerate * 2, // time_scale (2× framerate for field timing)
            ),
        ))
    }

    /// Build the picture parameter buffer.
    fn build_pic_param(
        &self,
        is_idr: bool,
        recon_surface: &Surface<()>,
        coded_buf: &EncCodedBuffer,
        frame_num: u16,
        poc: u16,
    ) -> BufferType {
        let is_reference = true; // All frames are used as references.

        // Current picture.
        let curr_pic = PictureH264::new(
            recon_surface.id(),
            u32::from(frame_num),
            VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            i32::from(poc),
            i32::from(poc),
        );

        // Reference frames array (up to 16 slots).
        let mut reference_frames: [PictureH264; 16] = std::array::from_fn(|_| build_invalid_pic());

        if let Some(ref ref_pic) = self.reference {
            reference_frames[0] = PictureH264::new(
                ref_pic.surface.id(),
                ref_pic.frame_num,
                VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                i32::from(ref_pic.poc),
                i32::from(ref_pic.poc),
            );
        }

        let pic_fields = H264EncPicFields::new(
            u32::from(is_idr),       // idr_pic_flag
            u32::from(is_reference), // reference_pic_flag
            0,                       // entropy_coding_mode_flag (CAVLC)
            0,                       // weighted_pred_flag
            0,                       // weighted_bipred_idc
            0,                       // constrained_intra_pred_flag
            0,                       // transform_8x8_mode_flag
            1,                       // deblocking_filter_control_present_flag
            0,                       // redundant_pic_cnt_present_flag
            0,                       // pic_order_present_flag
            0,                       // pic_scaling_matrix_present_flag
        );

        BufferType::EncPictureParameter(EncPictureParameter::H264(
            EncPictureParameterBufferH264::new(
                curr_pic,
                reference_frames,
                coded_buf.id(),
                0,             // pic_parameter_set_id
                0,             // seq_parameter_set_id
                0,             // last_picture (not EOS)
                frame_num,     // frame_num
                self.qp as u8, // pic_init_qp
                0,             // num_ref_idx_l0_active_minus1
                0,             // num_ref_idx_l1_active_minus1
                0,             // chroma_qp_index_offset
                0,             // second_chroma_qp_index_offset
                &pic_fields,
            ),
        ))
    }

    /// Build the slice parameter buffer.
    fn build_slice_param(
        &self,
        is_i_frame: bool,
        is_idr: bool,
        poc: u16,
        frame_num: u32,
    ) -> BufferType {
        let slice_type = if is_i_frame { SLICE_TYPE_I } else { SLICE_TYPE_P };

        // Reference picture lists.
        let mut ref_pic_list_0: [PictureH264; 32] = std::array::from_fn(|_| build_invalid_pic());
        let mut num_ref_idx_l0_active_minus1: u8 = 0;
        let mut num_ref_idx_active_override_flag: u8 = 0;

        if !is_i_frame {
            if let Some(ref ref_pic) = self.reference {
                ref_pic_list_0[0] = PictureH264::new(
                    ref_pic.surface.id(),
                    ref_pic.frame_num,
                    VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                    i32::from(ref_pic.poc),
                    i32::from(ref_pic.poc),
                );
                num_ref_idx_l0_active_minus1 = 0; // 1 reference frame
                num_ref_idx_active_override_flag = 1;
            }
        }

        let ref_pic_list_1: [PictureH264; 32] = std::array::from_fn(|_| build_invalid_pic());

        let idr_pic_id =
            if is_idr { (self.frame_count / u64::from(self.idr_period)) as u16 } else { 0 };

        // Compute slice_qp_delta so that pic_init_qp + slice_qp_delta = target QP.
        // Since we set pic_init_qp = self.qp, slice_qp_delta = 0.
        let slice_qp_delta: i8 = 0;

        BufferType::EncSliceParameter(EncSliceParameter::H264(EncSliceParameterBufferH264::new(
            0,             // macroblock_address (start of slice)
            self.num_mbs,  // num_macroblocks
            VA_INVALID_ID, // macroblock_info (unused)
            slice_type,
            0, // pic_parameter_set_id
            idr_pic_id,
            poc,       // pic_order_cnt_lsb
            0,         // delta_pic_order_cnt_bottom
            [0i32; 2], // delta_pic_order_cnt
            0,         // direct_spatial_mv_pred_flag
            num_ref_idx_active_override_flag,
            num_ref_idx_l0_active_minus1,
            0, // num_ref_idx_l1_active_minus1
            ref_pic_list_0,
            ref_pic_list_1,
            0,               // luma_log2_weight_denom
            0,               // chroma_log2_weight_denom
            0,               // luma_weight_l0_flag
            [0i16; 32],      // luma_weight_l0
            [0i16; 32],      // luma_offset_l0
            0,               // chroma_weight_l0_flag
            [[0i16; 2]; 32], // chroma_weight_l0
            [[0i16; 2]; 32], // chroma_offset_l0
            0,               // luma_weight_l1_flag
            [0i16; 32],      // luma_weight_l1
            [0i16; 32],      // luma_offset_l1
            0,               // chroma_weight_l1_flag
            [[0i16; 2]; 32], // chroma_weight_l1
            [[0i16; 2]; 32], // chroma_offset_l1
            0,               // cabac_init_idc (CAVLC)
            slice_qp_delta,
            0, // disable_deblocking_filter_idc (enabled)
            0, // slice_alpha_c0_offset_div2
            0, // slice_beta_offset_div2
        )))
    }

    /// Build the rate-control miscellaneous parameter buffer (CQP mode).
    fn build_rc_param(&self) -> BufferType {
        let rc_flags = RcFlags::new(
            0, // reset
            1, // disable_frame_skip
            0, // disable_bit_stuffing
            0, // mb_rate_control
            0, // temporal_id
            0, // cfs_i_frames
            0, // enable_parallel_brc
            0, // enable_dynamic_scaling
            0, // frame_tolerance_mode
        );

        BufferType::EncMiscParameter(EncMiscParameter::RateControl(
            EncMiscParameterRateControl::new(
                0,       // bits_per_second (CQP → 0)
                100,     // target_percentage
                1500,    // window_size (ms)
                self.qp, // initial_qp
                MIN_QP,  // min_qp
                0,       // basic_unit_size
                rc_flags, 0,      // icq_quality_factor
                MAX_QP, // max_qp
                0,      // quality_factor
                0,      // target_frame_size
            ),
        ))
    }

    /// Accessors for integration with the node layer.
    pub fn coded_width(&self) -> u32 {
        self.coded_width
    }
    pub fn coded_height(&self) -> u32 {
        self.coded_height
    }
}

// ---------------------------------------------------------------------------
// H.264 NALU generation (SPS / PPS)
// ---------------------------------------------------------------------------

/// Minimal bitstream writer for constructing H.264 NALUs with exp-Golomb
/// coded fields.
struct BitWriter {
    buf: Vec<u8>,
    byte: u8,
    bits: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self { buf: Vec::with_capacity(64), byte: 0, bits: 0 }
    }

    /// Write `n` bits from the low end of `val`.
    fn write_bits(&mut self, val: u32, n: u8) {
        for i in (0..n).rev() {
            self.byte = (self.byte << 1) | (((val >> i) & 1) as u8);
            self.bits += 1;
            if self.bits == 8 {
                self.buf.push(self.byte);
                self.byte = 0;
                self.bits = 0;
            }
        }
    }

    /// Write a single bit.
    fn write_bit(&mut self, val: bool) {
        self.write_bits(u32::from(val), 1);
    }

    /// Write an unsigned exp-Golomb code (ue(v)).
    fn write_ue(&mut self, val: u32) {
        let x = val + 1;
        let len = 32 - x.leading_zeros(); // number of bits in x
                                          // Leading zeros: len - 1
        for _ in 0..len - 1 {
            self.write_bit(false);
        }
        self.write_bits(x, len as u8);
    }

    /// Write a signed exp-Golomb code (se(v)).
    fn write_se(&mut self, val: i32) {
        let mapped = if val > 0 { (val as u32) * 2 - 1 } else { ((-val) as u32) * 2 };
        self.write_ue(mapped);
    }

    /// Finish the NALU: add RBSP stop bit + trailing zero bits.
    fn finish(mut self) -> Vec<u8> {
        // RBSP stop bit.
        self.write_bit(true);
        // Pad remaining bits to byte boundary.
        if self.bits > 0 {
            self.byte <<= 8 - self.bits;
            self.buf.push(self.byte);
        }
        self.buf
    }
}

impl VaH264Encoder {
    /// Generate a complete SPS NALU (Annex B start code + RBSP).
    fn build_sps_nalu(&self) -> Vec<u8> {
        let mut w = BitWriter::new();

        // NAL header: forbidden_zero_bit(1), nal_ref_idc(2)=3, nal_unit_type(5)=7 (SPS)
        w.write_bits(0x67, 8);

        // profile_idc = 77 (Main)
        w.write_bits(77, 8);

        // constraint_set0..3_flags, reserved_zero_4bits
        // constraint_set0_flag=0, constraint_set1_flag=1 (Main), set2=0, set3=0, reserved=0000
        w.write_bits(0b0100_0000, 8);

        // level_idc = 41
        w.write_bits(41, 8);

        // seq_parameter_set_id
        w.write_ue(0);

        // log2_max_frame_num_minus4
        w.write_ue(self.log2_max_frame_num_minus4);

        // pic_order_cnt_type = 0
        w.write_ue(0);

        // log2_max_pic_order_cnt_lsb_minus4
        w.write_ue(self.log2_max_pic_order_cnt_lsb_minus4);

        // max_num_ref_frames
        w.write_ue(1);

        // gaps_in_frame_num_value_allowed_flag
        w.write_bit(false);

        // pic_width_in_mbs_minus1
        w.write_ue(u32::from(self.width_in_mbs) - 1);

        // pic_height_in_map_units_minus1
        w.write_ue(u32::from(self.height_in_mbs) - 1);

        // frame_mbs_only_flag = 1
        w.write_bit(true);

        // (mb_adaptive_frame_field_flag omitted when frame_mbs_only_flag=1)

        // direct_8x8_inference_flag = 1
        w.write_bit(true);

        // frame_cropping_flag + offsets
        if let Some(ref crop) = self.frame_crop {
            w.write_bit(true);
            w.write_ue(crop.left);
            w.write_ue(crop.right);
            w.write_ue(crop.top);
            w.write_ue(crop.bottom);
        } else {
            w.write_bit(false);
        }

        // vui_parameters_present_flag = 1
        w.write_bit(true);

        // --- VUI ---
        // aspect_ratio_info_present_flag = 1
        w.write_bit(true);
        // aspect_ratio_idc = 1 (1:1 SAR)
        w.write_bits(1, 8);

        // overscan_info_present_flag = 0
        w.write_bit(false);

        // video_signal_type_present_flag = 0
        w.write_bit(false);

        // chroma_loc_info_present_flag = 0
        w.write_bit(false);

        // timing_info_present_flag = 1
        w.write_bit(true);
        // num_units_in_tick = 1
        w.write_bits(1, 32);
        // time_scale = framerate * 2
        w.write_bits(self.framerate * 2, 32);
        // fixed_frame_rate_flag = 0
        w.write_bit(false);

        // nal_hrd_parameters_present_flag = 0
        w.write_bit(false);
        // vcl_hrd_parameters_present_flag = 0
        w.write_bit(false);

        // pic_struct_present_flag = 0
        w.write_bit(false);

        // bitstream_restriction_flag = 0
        w.write_bit(false);

        // Build the NALU with Annex B start code.
        let rbsp = w.finish();
        let mut nalu = Vec::with_capacity(rbsp.len() + 4);
        nalu.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        nalu.extend_from_slice(&rbsp);
        nalu
    }

    /// Generate a complete PPS NALU (Annex B start code + RBSP).
    fn build_pps_nalu(&self) -> Vec<u8> {
        let mut w = BitWriter::new();

        // NAL header: forbidden_zero_bit(1), nal_ref_idc(2)=3, nal_unit_type(5)=8 (PPS)
        w.write_bits(0x68, 8);

        // pic_parameter_set_id
        w.write_ue(0);

        // seq_parameter_set_id
        w.write_ue(0);

        // entropy_coding_mode_flag = 0 (CAVLC)
        w.write_bit(false);

        // bottom_field_pic_order_in_frame_present_flag = 0
        w.write_bit(false);

        // num_slice_groups_minus1 = 0
        w.write_ue(0);

        // num_ref_idx_l0_default_active_minus1 = 0
        w.write_ue(0);

        // num_ref_idx_l1_default_active_minus1 = 0
        w.write_ue(0);

        // weighted_pred_flag = 0
        w.write_bit(false);

        // weighted_bipred_idc = 0
        w.write_bits(0, 2);

        // pic_init_qp_minus26
        w.write_se(self.qp as i32 - 26);

        // pic_init_qs_minus26
        w.write_se(0);

        // chroma_qp_index_offset
        w.write_se(0);

        // deblocking_filter_control_present_flag = 1
        w.write_bit(true);

        // constrained_intra_pred_flag = 0
        w.write_bit(false);

        // redundant_pic_cnt_present_flag = 0
        w.write_bit(false);

        // Build the NALU with Annex B start code.
        let rbsp = w.finish();
        let mut nalu = Vec::with_capacity(rbsp.len() + 4);
        nalu.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        nalu.extend_from_slice(&rbsp);
        nalu
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Build an invalid `PictureH264` placeholder (fills unused reference slots).
fn build_invalid_pic() -> PictureH264 {
    PictureH264::new(VA_INVALID_ID, 0, VA_PICTURE_H264_INVALID, 0, 0)
}

/// Check whether an Annex B bitstream contains both SPS and PPS NALUs.
fn bitstream_contains_sps_pps(data: &[u8]) -> bool {
    let mut has_sps = false;
    let mut has_pps = false;
    let len = data.len();
    let mut i = 0;
    while i + 2 < len {
        let sc_len = if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else if i + 3 < len
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            4
        } else {
            0
        };
        if sc_len > 0 {
            let nal_pos = i + sc_len;
            if nal_pos < len {
                let nal_type = data[nal_pos] & 0x1F;
                if nal_type == 7 {
                    has_sps = true;
                }
                if nal_type == 8 {
                    has_pps = true;
                }
            }
            i = nal_pos;
        } else {
            i += 1;
        }
    }
    has_sps && has_pps
}

/// Round `value` up to the next multiple of `alignment`.
fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) / alignment * alignment
}

/// Compute ceil(log2(n)), minimum 0.
fn log2_ceil(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

/// Resolve whether to use the low-power entrypoint.
///
/// Auto-detects when the driver only supports `VAEntrypointEncSliceLP`.
fn resolve_low_power(display: &Display, requested: bool) -> Result<bool, String> {
    let entrypoints = display
        .query_config_entrypoints(VAProfile::VAProfileH264Main)
        .map_err(|e| format!("failed to query H.264 entrypoints: {e}"))?;

    let has_lp = entrypoints.contains(&VAEntrypoint::VAEntrypointEncSliceLP);
    let has_full = entrypoints.contains(&VAEntrypoint::VAEntrypointEncSlice);

    if !has_lp && !has_full {
        return Err("VA-API driver does not support H.264 encoding (no EncSlice entrypoint)".into());
    }

    if requested {
        if !has_lp {
            return Err(
                "low_power=true requested but VAEntrypointEncSliceLP is not supported".into()
            );
        }
        Ok(true)
    } else if has_lp && !has_full {
        tracing::info!("auto-selecting low-power H.264 encoder (VAEntrypointEncSliceLP)");
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(1, 16), 16);
        assert_eq!(align_up(16, 16), 16);
        assert_eq!(align_up(17, 16), 32);
        assert_eq!(align_up(1280, 16), 1280);
        assert_eq!(align_up(720, 16), 720);
        assert_eq!(align_up(1080, 16), 1088);
    }

    #[test]
    fn test_log2_ceil() {
        assert_eq!(log2_ceil(0), 0);
        assert_eq!(log2_ceil(1), 0);
        assert_eq!(log2_ceil(2), 1);
        assert_eq!(log2_ceil(3), 2);
        assert_eq!(log2_ceil(4), 2);
        assert_eq!(log2_ceil(5), 3);
        assert_eq!(log2_ceil(1024), 10);
        assert_eq!(log2_ceil(2048), 11);
    }

    #[test]
    fn test_bitwriter_bits() {
        let mut w = BitWriter::new();
        w.write_bits(0b1010, 4);
        w.write_bits(0b1100, 4);
        let out = w.finish();
        // 1010 1100 + stop bit 1 + 0000000 padding
        assert_eq!(out[0], 0b1010_1100);
        assert_eq!(out[1], 0b1000_0000);
    }

    #[test]
    fn test_bitwriter_ue() {
        // ue(0) = 1 (1 bit)
        let mut w = BitWriter::new();
        w.write_ue(0);
        let out = w.finish();
        assert_eq!(out[0], 0b1_1000000); // 1 + stop + pad

        // ue(1) = 010 (3 bits)
        let mut w = BitWriter::new();
        w.write_ue(1);
        let out = w.finish();
        assert_eq!(out[0], 0b010_1_0000); // 010 + stop + pad

        // ue(5) = 00110 (5 bits)
        let mut w = BitWriter::new();
        w.write_ue(5);
        let out = w.finish();
        assert_eq!(out[0], 0b00110_1_00); // 00110 + stop + pad
    }

    #[test]
    fn test_bitwriter_se() {
        // se(0) → ue(0) = 1
        let mut w = BitWriter::new();
        w.write_se(0);
        let out = w.finish();
        assert_eq!(out[0], 0b1_1000000);

        // se(1) → ue(1) = 010
        let mut w = BitWriter::new();
        w.write_se(1);
        let out = w.finish();
        assert_eq!(out[0], 0b010_1_0000);

        // se(-1) → ue(2) = 011
        let mut w = BitWriter::new();
        w.write_se(-1);
        let out = w.finish();
        assert_eq!(out[0], 0b011_1_0000);
    }

    #[test]
    fn test_bitstream_contains_sps_pps() {
        // Bitstream with SPS (type 7) and PPS (type 8).
        let data = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f, // SPS
            0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x38, 0x80, // PPS
            0x00, 0x00, 0x00, 0x01, 0x65, 0x11, 0x22, 0x33, // IDR slice
        ];
        assert!(bitstream_contains_sps_pps(&data));

        // Bitstream without SPS/PPS (only IDR slice).
        let data_no_ps = [0x00, 0x00, 0x00, 0x01, 0x65, 0x11, 0x22, 0x33];
        assert!(!bitstream_contains_sps_pps(&data_no_ps));

        // 3-byte start codes.
        let data_3byte = [
            0x00, 0x00, 0x01, 0x67, 0x42, // SPS
            0x00, 0x00, 0x01, 0x68, 0xce, // PPS
        ];
        assert!(bitstream_contains_sps_pps(&data_3byte));

        // Only SPS, no PPS.
        let data_sps_only = [0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0];
        assert!(!bitstream_contains_sps_pps(&data_sps_only));
    }
}
