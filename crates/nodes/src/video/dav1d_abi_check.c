// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Build-time check: verify that the Rust-side opaque buffer layouts match the
// installed dav1d headers.  If a future dav1d release changes any of these
// structs, this file will fail to compile, alerting the developer to update
// the FFI bindings in dav1d_ffi.rs.

#include <stddef.h>
#include <dav1d/dav1d.h>

// --- Dav1dSettings (opaque buffer, 1024 bytes) ---
_Static_assert(
    sizeof(Dav1dSettings) <= 1024,
    "Dav1dSettings exceeds 1024 bytes — bump SETTINGS_SIZE in dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dSettings, n_threads) == 0,
    "Dav1dSettings::n_threads offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dSettings, max_frame_delay) == 4,
    "Dav1dSettings::max_frame_delay offset changed — update dav1d_ffi.rs"
);

// --- Dav1dData (opaque buffer, 128 bytes) ---
_Static_assert(
    sizeof(Dav1dData) <= 128,
    "Dav1dData exceeds 128 bytes — bump DATA_SIZE in dav1d_ffi.rs"
);

// --- Dav1dPicture (opaque buffer, 1024 bytes) ---
_Static_assert(
    sizeof(Dav1dPicture) <= 1024,
    "Dav1dPicture exceeds 1024 bytes — bump PICTURE_SIZE in dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPicture, data) == 16,
    "Dav1dPicture::data offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPicture, stride) == 40,
    "Dav1dPicture::stride offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPicture, p) == 56,
    "Dav1dPicture::p offset changed — update dav1d_ffi.rs"
);

// --- Dav1dPictureParameters (embedded in Dav1dPicture at offset 56) ---
_Static_assert(
    offsetof(Dav1dPictureParameters, w) == 0,
    "Dav1dPictureParameters::w offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPictureParameters, h) == 4,
    "Dav1dPictureParameters::h offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPictureParameters, layout) == 8,
    "Dav1dPictureParameters::layout offset changed — update dav1d_ffi.rs"
);
_Static_assert(
    offsetof(Dav1dPictureParameters, bpc) == 12,
    "Dav1dPictureParameters::bpc offset changed — update dav1d_ffi.rs"
);
