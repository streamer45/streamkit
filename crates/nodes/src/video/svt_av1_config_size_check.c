// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Build-time check: verify that the Rust-side opaque buffer (8192 bytes) is
// large enough for the real EbSvtAv1EncConfiguration struct.  If a future
// SVT-AV1 release grows the struct beyond this limit, this file will fail
// to compile, alerting the developer to bump CONFIG_SIZE in svt_av1_ffi.rs.

#include <svt-av1/EbSvtAv1Enc.h>

_Static_assert(
    sizeof(EbSvtAv1EncConfiguration) <= 8192,
    "EbSvtAv1EncConfiguration exceeds 8192 bytes — bump CONFIG_SIZE in svt_av1_ffi.rs"
);
