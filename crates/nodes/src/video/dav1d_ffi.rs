// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Minimal hand-written FFI bindings for libdav1d (≥ 1.0).
//!
//! Only the types and functions the dav1d decoder node actually calls are
//! declared here.  All structs are intentionally left opaque — we allocate
//! them as zeroed byte arrays of sufficient size and access individual
//! fields through offset-based helpers.  A companion C file
//! (`dav1d_abi_check.c`) validates sizes and offsets at build time via
//! `_Static_assert`.
//!
//! This is the same approach used for SVT-AV1 in `svt_av1_ffi.rs`.

use std::ffi::{c_int, c_void};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// dav1d error code for EAGAIN ("not ready, drain pictures first").
/// Matches `-(EAGAIN)` on Linux where `EAGAIN = 11`.
pub const DAV1D_EAGAIN: c_int = -11;

/// `DAV1D_PIXEL_LAYOUT_I420` — 4:2:0 planar (the layout AV1 always produces
/// for Main profile).
pub const DAV1D_PIXEL_LAYOUT_I420: c_int = 1;

// ---------------------------------------------------------------------------
// Opaque struct sizes
// ---------------------------------------------------------------------------
//
// Each size is a generous upper bound on the real C struct.
// `dav1d_abi_check.c` verifies `sizeof(…) <= *_SIZE` at build time.

const SETTINGS_SIZE: usize = 1024;
const DATA_SIZE: usize = 128;
const PICTURE_SIZE: usize = 1024;

// ---------------------------------------------------------------------------
// Dav1dSettings
// ---------------------------------------------------------------------------

/// Opaque wrapper for the C `Dav1dSettings` struct.
///
/// Populated by [`dav1d_default_settings`]; individual fields are set via
/// offset-based helpers whose correctness is verified at build time.
#[repr(C, align(8))]
pub struct Dav1dSettings {
    buf: [u8; SETTINGS_SIZE],
}

impl Dav1dSettings {
    /// Create a zeroed settings buffer.
    ///
    /// The caller **must** pass this to [`dav1d_default_settings`] before use.
    pub const fn zeroed() -> Self {
        Self { buf: [0u8; SETTINGS_SIZE] }
    }

    /// Set `n_threads` (C `int` at offset 0).
    pub fn set_n_threads(&mut self, val: c_int) {
        // SAFETY: offset 0 is validated by _Static_assert in dav1d_abi_check.c
        let bytes = val.to_ne_bytes();
        self.buf[..4].copy_from_slice(&bytes);
    }

    /// Set `max_frame_delay` (C `int` at offset 4).
    pub fn set_max_frame_delay(&mut self, val: c_int) {
        // SAFETY: offset 4 is validated by _Static_assert in dav1d_abi_check.c
        let bytes = val.to_ne_bytes();
        self.buf[4..8].copy_from_slice(&bytes);
    }
}

// ---------------------------------------------------------------------------
// Dav1dData
// ---------------------------------------------------------------------------

/// Opaque wrapper for the C `Dav1dData` struct.
///
/// Managed entirely through [`dav1d_data_create`] / [`dav1d_send_data`] /
/// [`dav1d_data_unref`] — no direct field access is needed.
#[repr(C, align(8))]
pub struct Dav1dData {
    buf: [u8; DATA_SIZE],
}

impl Dav1dData {
    /// Create a zeroed data buffer.
    pub const fn zeroed() -> Self {
        Self { buf: [0u8; DATA_SIZE] }
    }
}

// ---------------------------------------------------------------------------
// Dav1dPicture
// ---------------------------------------------------------------------------

/// Opaque wrapper for the C `Dav1dPicture` struct.
///
/// Decoded pictures are read through offset-based accessors whose correctness
/// is verified at build time by `dav1d_abi_check.c`.
///
/// ## Layout (x86_64, dav1d 1.5.3)
///
/// | Offset | Field                 | C type          |
/// |--------|-----------------------|-----------------|
/// |   0    | `seq_hdr`             | `void *`        |
/// |   8    | `frame_hdr`           | `void *`        |
/// |  16    | `data[3]`             | `void *[3]`     |
/// |  40    | `stride[2]`           | `ptrdiff_t[2]`  |
/// |  56    | `p.w`                 | `int`           |
/// |  60    | `p.h`                 | `int`           |
/// |  64    | `p.layout`            | `int` (enum)    |
/// |  68    | `p.bpc`               | `int`           |
#[repr(C, align(8))]
pub struct Dav1dPicture {
    buf: [u8; PICTURE_SIZE],
}

/// Byte offset of `Dav1dPicture::data[0]`.
const PIC_DATA_OFFSET: usize = 16;
/// Byte offset of `Dav1dPicture::stride[0]`.
const PIC_STRIDE_OFFSET: usize = 40;
/// Byte offset of `Dav1dPicture::p.w`.
const PIC_P_W_OFFSET: usize = 56;
/// Byte offset of `Dav1dPicture::p.h`.
const PIC_P_H_OFFSET: usize = 60;
/// Byte offset of `Dav1dPicture::p.layout`.
const PIC_P_LAYOUT_OFFSET: usize = 64;

/// Read a `c_int` (4 bytes, native endian) from `buf` at `offset`.
const fn read_i32(buf: &[u8; PICTURE_SIZE], offset: usize) -> c_int {
    c_int::from_ne_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
}

/// Read a `usize` (8 bytes, native endian) from `buf` at `offset`.
const fn read_usize(buf: &[u8; PICTURE_SIZE], offset: usize) -> usize {
    usize::from_ne_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

/// Read an `isize` (8 bytes, native endian) from `buf` at `offset`.
const fn read_isize(buf: &[u8; PICTURE_SIZE], offset: usize) -> isize {
    isize::from_ne_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

impl Dav1dPicture {
    /// Create a zeroed picture buffer.
    pub const fn zeroed() -> Self {
        Self { buf: [0u8; PICTURE_SIZE] }
    }

    /// Read `data[plane]` — a `void *` pointer to the plane's pixel data.
    ///
    /// # Panics
    ///
    /// Panics if `plane >= 3`.
    pub fn data_ptr(&self, plane: usize) -> *const u8 {
        assert!(plane < 3, "plane index must be 0, 1, or 2");
        let offset = PIC_DATA_OFFSET + plane * std::mem::size_of::<*const u8>();
        read_usize(&self.buf, offset) as *const u8
    }

    /// Read `stride[idx]` — a `ptrdiff_t` value (luma=0, chroma=1).
    ///
    /// # Panics
    ///
    /// Panics if `idx >= 2`.
    pub fn stride(&self, idx: usize) -> isize {
        assert!(idx < 2, "stride index must be 0 or 1");
        let offset = PIC_STRIDE_OFFSET + idx * std::mem::size_of::<isize>();
        read_isize(&self.buf, offset)
    }

    /// Read `p.w` — picture width in pixels.
    pub const fn width(&self) -> c_int {
        read_i32(&self.buf, PIC_P_W_OFFSET)
    }

    /// Read `p.h` — picture height in pixels.
    pub const fn height(&self) -> c_int {
        read_i32(&self.buf, PIC_P_H_OFFSET)
    }

    /// Read `p.layout` — pixel layout enum (`DAV1D_PIXEL_LAYOUT_*`).
    pub const fn layout(&self) -> c_int {
        read_i32(&self.buf, PIC_P_LAYOUT_OFFSET)
    }
}

// ---------------------------------------------------------------------------
// Extern functions
// ---------------------------------------------------------------------------

extern "C" {
    /// Initialize settings to default values.
    pub fn dav1d_default_settings(s: *mut Dav1dSettings);

    /// Allocate and open a decoder instance.
    ///
    /// Returns 0 on success, or a negative `DAV1D_ERR` code on error.
    pub fn dav1d_open(c_out: *mut *mut c_void, s: *const Dav1dSettings) -> c_int;

    /// Feed bitstream data to the decoder.
    ///
    /// Returns 0 on success (data consumed), `DAV1D_ERR(EAGAIN)` if the
    /// caller must drain pictures first, or another negative error code.
    pub fn dav1d_send_data(c: *mut c_void, data: *mut Dav1dData) -> c_int;

    /// Return a decoded picture.
    ///
    /// Returns 0 on success, `DAV1D_ERR(EAGAIN)` if not enough data to
    /// produce a picture, or another negative error code.
    pub fn dav1d_get_picture(c: *mut c_void, out: *mut Dav1dPicture) -> c_int;

    /// Flush all delayed frames and clear internal decoder state.
    pub fn dav1d_flush(c: *mut c_void);

    /// Close a decoder instance and free all associated memory.
    ///
    /// `*c_out` is set to NULL after this call.
    pub fn dav1d_close(c_out: *mut *mut c_void);

    /// Allocate a `Dav1dData` buffer of `sz` bytes.
    ///
    /// Returns a pointer to the allocated buffer, or NULL on error.
    pub fn dav1d_data_create(data: *mut Dav1dData, sz: usize) -> *mut u8;

    /// Free the data reference.
    pub fn dav1d_data_unref(data: *mut Dav1dData);

    /// Release reference to a decoded picture.
    pub fn dav1d_picture_unref(p: *mut Dav1dPicture);
}
