// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * RFC 6381 codec string (`av01.P.LLT.DD`) for StreamKit's AV1 output:
 * profile 0 (Main), level idx 8 (4.0), Main tier, 8-bit, 4:2:0.
 *
 * Single source of truth on the TS side. WebCodecs configs run in the browser
 * and can't import the Rust value, so this literal mirrors the Rust encoder
 * constants in `crates/nodes/src/video/mod.rs` (`AV1_PROFILE`, `AV1_LEVEL`,
 * `AV1_TIER`, `AV1_BIT_DEPTH`) that `av1_codec_string()` formats into the token
 * advertised by the WebM and MP4 muxers. The Rust test
 * `ui_av1_codec_constant_matches_rust_source` reads this file and fails if the
 * two ever diverge, so they must be updated together.
 */
export const AV1_CODEC_STRING = 'av01.0.08M.08';
