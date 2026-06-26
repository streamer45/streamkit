// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * RFC 6381 codec string (`av01.P.LLT.DD`) for StreamKit's AV1 output:
 * profile 0 (Main), level idx 8 (4.0), Main tier, 8-bit, 4:2:0.
 *
 * Single source of truth on the TS side. Must stay in sync with the Rust
 * encoder constants in `crates/nodes/src/video/mod.rs` (`AV1_PROFILE`,
 * `AV1_LEVEL`, `AV1_TIER`, `AV1_BIT_DEPTH`), which `av1_codec_string()` formats
 * into the same token advertised by the WebM and MP4 muxers. Both sides must be
 * updated together if the profile/level/tier/bit-depth ever changes.
 */
export const AV1_CODEC_STRING = 'av01.0.08M.08';
