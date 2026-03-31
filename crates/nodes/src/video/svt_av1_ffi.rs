// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Minimal hand-written FFI bindings for libsvtav1enc (≥ 2.0).
//!
//! Only the types and functions the SVT-AV1 encoder node actually calls are
//! declared here.  The [`EbSvtAv1EncConfiguration`] struct is intentionally
//! left opaque — we allocate it as a zeroed byte array of sufficient size and
//! let `svt_av1_enc_init_handle` fill in defaults, then use
//! `svt_av1_enc_parse_parameter` to set individual fields by name (which is
//! ABI-stable across minor SVT-AV1 releases).

use std::ffi::c_void;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const EB_BUFFERFLAG_EOS: u32 = 0x0000_0001;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// SVT-AV1 error codes.  `EB_ErrorNone` (0) indicates success.
pub type EbErrorType = i32;

pub const EB_ERROR_NONE: EbErrorType = 0;
// `svt_av1_enc_get_packet` returns this when no packet is available yet.
pub const EB_NO_ERROR_EMPTY_QUEUE: EbErrorType = 0x8000_2033_u32.cast_signed();

// ---------------------------------------------------------------------------
// Opaque / repr(C) types used by the API
// ---------------------------------------------------------------------------

/// Encoder component handle.
#[repr(C)]
pub struct EbComponentType {
    pub size: u32,
    pub p_component_private: *mut c_void,
    pub p_application_private: *mut c_void,
}

/// Buffer header — wraps an input picture or an output packet.
#[repr(C)]
pub struct EbBufferHeaderType {
    pub size: u32,
    pub p_buffer: *mut u8,
    pub n_filled_len: u32,
    pub n_alloc_len: u32,
    pub p_app_private: *mut c_void,
    pub wrapper_ptr: *mut c_void,
    pub n_tick_count: u32,
    pub dts: i64,
    pub pts: i64,
    pub qp: u32,
    pub pic_type: u32, // EbAv1PictureType
    pub luma_sse: u64,
    pub cr_sse: u64,
    pub cb_sse: u64,
    pub flags: u32,
    pub luma_ssim: f64,
    pub cr_ssim: f64,
    pub cb_ssim: f64,
    pub metadata: *mut c_void, // *mut SvtMetadataArray
}

/// I/O format for picture planes (YUV420 planar).
///
/// In SVT-AV1 ≥ 2.0 the deprecated `luma_ext`/`cb_ext`/`cr_ext` fields are
/// removed, so the layout is: luma, cb, cr, then stride/dimension fields.
#[repr(C)]
pub struct EbSvtIOFormat {
    pub luma: *mut u8,
    pub cb: *mut u8,
    pub cr: *mut u8,
    pub y_stride: u32,
    pub cr_stride: u32,
    pub cb_stride: u32,
    pub width: u32,
    pub height: u32,
    pub org_x: u32,
    pub org_y: u32,
    pub color_fmt: u32, // EbColorFormat
    pub bit_depth: u32, // EbBitDepth
}

/// Encoder configuration — intentionally opaque.
///
/// The struct layout changes across SVT-AV1 minor versions (conditional
/// compilation, padding bytes, deprecated fields).  Instead of reproducing
/// the full 2 KB struct field-by-field, we:
///
/// 1. Allocate a generously-sized zeroed buffer (`CONFIG_SIZE` bytes).
/// 2. Let `svt_av1_enc_init_handle` populate it with sane defaults.
/// 3. Use `svt_av1_enc_parse_parameter` to override fields by name.
///
/// This is the same approach the SVT-AV1 `SvtAv1EncApp` uses and is stable
/// across 2.x releases.
///
/// The 8192-byte size is ~4× the actual struct size in 2.3.0 (≈2 KB) to
/// provide ample headroom for future additions.
const CONFIG_SIZE: usize = 8192;

#[repr(C, align(8))]
pub struct EbSvtAv1EncConfiguration {
    _data: [u8; CONFIG_SIZE],
}

impl EbSvtAv1EncConfiguration {
    /// Create a zeroed configuration buffer.
    ///
    /// The caller **must** pass this to `svt_av1_enc_init_handle` before use
    /// — the library fills in all default values.
    pub const fn zeroed() -> Self {
        Self { _data: [0u8; CONFIG_SIZE] }
    }
}

// ---------------------------------------------------------------------------
// Extern functions
// ---------------------------------------------------------------------------

extern "C" {
    /// Step 1: Construct an encoder handle and fill `config_ptr` with defaults.
    pub fn svt_av1_enc_init_handle(
        p_handle: *mut *mut EbComponentType,
        p_app_data: *mut c_void,
        config_ptr: *mut EbSvtAv1EncConfiguration,
    ) -> EbErrorType;

    /// Step 2: Apply configuration to the encoder.
    pub fn svt_av1_enc_set_parameter(
        svt_enc_component: *mut EbComponentType,
        config_ptr: *mut EbSvtAv1EncConfiguration,
    ) -> EbErrorType;

    /// Set a single configuration parameter by name (string key/value).
    ///
    /// This is ABI-stable across SVT-AV1 minor versions and avoids
    /// depending on the exact struct field offsets.
    pub fn svt_av1_enc_parse_parameter(
        config_ptr: *mut EbSvtAv1EncConfiguration,
        name: *const std::ffi::c_char,
        value: *const std::ffi::c_char,
    ) -> EbErrorType;

    /// Step 3: Initialize the encoder (allocates internal buffers).
    pub fn svt_av1_enc_init(svt_enc_component: *mut EbComponentType) -> EbErrorType;

    /// Step 4: Send a picture to the encoder.
    pub fn svt_av1_enc_send_picture(
        svt_enc_component: *mut EbComponentType,
        p_buffer: *mut EbBufferHeaderType,
    ) -> EbErrorType;

    /// Step 5: Receive an encoded packet.
    ///
    /// Returns `EB_ErrorNone` when a packet is available, or
    /// `EB_NoErrorEmptyQueue` when no packet is ready yet.
    /// Set `pic_send_done` to 1 after all pictures have been sent (EOS).
    pub fn svt_av1_enc_get_packet(
        svt_enc_component: *mut EbComponentType,
        p_buffer: *mut *mut EbBufferHeaderType,
        pic_send_done: u8,
    ) -> EbErrorType;

    /// Step 5-1: Release an output buffer back to the encoder's pool.
    pub fn svt_av1_enc_release_out_buffer(p_buffer: *mut *mut EbBufferHeaderType);

    /// Step 6: De-initialize the encoder.
    pub fn svt_av1_enc_deinit(svt_enc_component: *mut EbComponentType) -> EbErrorType;

    /// Step 7: Destroy the encoder handle.
    pub fn svt_av1_enc_deinit_handle(svt_enc_component: *mut EbComponentType) -> EbErrorType;
}
