// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Build-time check: verify that the Rust-side struct layouts match the
// installed SVT-AV1 headers.  If a future SVT-AV1 release changes any of
// these structs, this file will fail to compile, alerting the developer to
// update the FFI bindings in svt_av1_ffi.rs.

#include <stddef.h>
#include <svt-av1/EbSvtAv1Enc.h>

// --- EbSvtAv1EncConfiguration (opaque buffer) ---
// Our Rust-side opaque buffer is 8192 bytes.  Make sure the real struct fits.
_Static_assert(
    sizeof(EbSvtAv1EncConfiguration) <= 8192,
    "EbSvtAv1EncConfiguration exceeds 8192 bytes — bump CONFIG_SIZE in svt_av1_ffi.rs"
);

// --- EbSvtIOFormat (explicit repr(C) layout) ---
_Static_assert(
    sizeof(EbSvtIOFormat) == 40,
    "EbSvtIOFormat size changed — update svt_av1_ffi.rs"
);
_Static_assert(
    offsetof(EbSvtIOFormat, luma) == 0
    && offsetof(EbSvtIOFormat, cb) == 8
    && offsetof(EbSvtIOFormat, cr) == 16
    && offsetof(EbSvtIOFormat, y_stride) == 24
    && offsetof(EbSvtIOFormat, cr_stride) == 28
    && offsetof(EbSvtIOFormat, cb_stride) == 32,
    "EbSvtIOFormat layout changed — update svt_av1_ffi.rs"
);

// --- EbBufferHeaderType (explicit repr(C) layout) ---
_Static_assert(
    sizeof(EbBufferHeaderType) == 144,
    "EbBufferHeaderType size changed — update svt_av1_ffi.rs"
);
_Static_assert(
    offsetof(EbBufferHeaderType, size) == 0
    && offsetof(EbBufferHeaderType, p_buffer) == 8
    && offsetof(EbBufferHeaderType, n_filled_len) == 16
    && offsetof(EbBufferHeaderType, n_alloc_len) == 20
    && offsetof(EbBufferHeaderType, p_app_private) == 24
    && offsetof(EbBufferHeaderType, wrapper_ptr) == 32
    && offsetof(EbBufferHeaderType, n_tick_count) == 40
    && offsetof(EbBufferHeaderType, dts) == 48
    && offsetof(EbBufferHeaderType, pts) == 56
    && offsetof(EbBufferHeaderType, temporal_layer_index) == 64
    && offsetof(EbBufferHeaderType, qp) == 68
    && offsetof(EbBufferHeaderType, avg_qp) == 72
    && offsetof(EbBufferHeaderType, pic_type) == 76
    && offsetof(EbBufferHeaderType, luma_sse) == 80
    && offsetof(EbBufferHeaderType, cr_sse) == 88
    && offsetof(EbBufferHeaderType, cb_sse) == 96
    && offsetof(EbBufferHeaderType, flags) == 104
    && offsetof(EbBufferHeaderType, luma_ssim) == 112
    && offsetof(EbBufferHeaderType, cr_ssim) == 120
    && offsetof(EbBufferHeaderType, cb_ssim) == 128
    && offsetof(EbBufferHeaderType, metadata) == 136,
    "EbBufferHeaderType layout changed — update svt_av1_ffi.rs"
);
