// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! x86-64 SIMD kernels for pixel-level operations.
//!
//! This module is only compiled on `x86_64` targets (gated at the module level
//! in `mod.rs`).  It contains SSE2, SSE4.1 and AVX2 kernels for:
//!
//! - RGBA8 alpha blending (used by the blit functions)
//! - I420 / NV12 → RGBA8 colour-space conversion
//! - RGBA8 → I420 / NV12 colour-space conversion (Y-plane and chroma)
//!
//! SSE2 and SSE4.1 variants share the same algorithmic structure, differing
//! only in how they perform 32-bit integer multiplies.  The `impl_yuv_to_rgba`
//! and `impl_rgba_to_y` macros generate both variants from a single body,
//! parameterised on the multiply strategy.

// ── SSE2 alpha-blend helpers ────────────────────────────────────────────────
//
// Process 4 RGBA pixels at a time using SSE2 integer arithmetic.
// Source pixels are gathered (non-contiguous via x_map), destination pixels
// are contiguous.  The blend formula is identical to the scalar `blend_u8`:
//   result = ((src*alpha + dst*(255-alpha) + 128) + ((…) >> 8)) >> 8
//
// For the alpha channel we set source-alpha to 255 before blending so that
// `blend_u8(255, dst_alpha, src_alpha)` naturally computes the standard
// over-composite alpha `a_src + a_dst*(1-a_src)` (within ±1 of the scalar
// approximation — both are approximate divisions by 255).

/// Read 4 bytes from `src` at `offset` as a native-endian `u32`.
///
/// # Safety
///
/// Caller must ensure `offset + 3 < src.len()`.
#[inline]
pub(super) const unsafe fn read_rgba_u32(src: &[u8], offset: usize) -> u32 {
    std::ptr::read_unaligned(src.as_ptr().add(offset).cast::<u32>())
}

/// Blend 4 gathered source RGBA pixels onto 4 contiguous destination pixels
/// using SSE2 "over" compositing (no opacity modifier).
///
/// # Safety
///
/// `dst_ptr` must point to at least 16 writable bytes.  Source pixel values
/// in `src_pixels` must be valid RGBA `u32` values.
//
// NOTE: no `#[target_feature(enable = "sse2")]` here — SSE2 is baseline on
// x86_64 so the attribute is unnecessary, and omitting it allows
// `#[inline(always)]` which is required for the hot inner-loop call sites.
// (`#[target_feature]` and `#[inline(always)]` are mutually exclusive on
// stable Rust.)
#[inline(always)]
#[allow(clippy::cast_ptr_alignment)] // _mm_storeu/loadu_si128 do not require alignment
pub(super) unsafe fn blend_4px_opaque_sse2(dst_ptr: *mut u8, src_pixels: [u32; 4]) {
    use std::arch::x86_64::{
        __m128i, _mm_add_epi16, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8,
        _mm_mullo_epi16, _mm_or_si128, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32,
        _mm_set_epi32, _mm_setzero_si128, _mm_shufflehi_epi16, _mm_shufflelo_epi16, _mm_srli_epi16,
        _mm_storeu_si128, _mm_sub_epi16, _mm_unpackhi_epi8, _mm_unpacklo_epi8,
    };

    let zero = _mm_setzero_si128();
    let c255 = _mm_set1_epi16(255);
    let c128 = _mm_set1_epi16(128);

    // Assemble 4 gathered source pixels into one register.
    let src4 = _mm_set_epi32(
        src_pixels[3].cast_signed(),
        src_pixels[2].cast_signed(),
        src_pixels[1].cast_signed(),
        src_pixels[0].cast_signed(),
    );

    // Mask with 0xFF at each pixel's alpha-byte position (bytes 3,7,11,15).
    let alpha_byte_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());

    // Fast path: all 4 source pixels fully opaque → direct copy.
    let alpha_bytes = _mm_and_si128(src4, alpha_byte_mask);
    if _mm_movemask_epi8(_mm_cmpeq_epi8(alpha_bytes, alpha_byte_mask)) == 0xFFFF {
        _mm_storeu_si128(dst_ptr.cast::<__m128i>(), src4);
        return;
    }

    // Fast path: all 4 source pixels fully transparent → nothing to do.
    if _mm_movemask_epi8(_mm_cmpeq_epi8(alpha_bytes, zero)) == 0xFFFF {
        return;
    }

    let dst4 = _mm_loadu_si128(dst_ptr.cast::<__m128i>().cast_const());

    // Replace source alpha channel with 255 for correct composite-alpha
    // via blend_u8(255, dst_alpha, src_alpha).
    let src_blend = _mm_or_si128(src4, alpha_byte_mask);

    // --- Low 2 pixels (u16 arithmetic) ---
    let src_lo = _mm_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm_unpacklo_epi8(dst4, zero);

    // Extract original source alpha and broadcast within each 4-u16 pixel group.
    let src_orig_lo = _mm_unpacklo_epi8(src4, zero);
    // _MM_SHUFFLE(3,3,3,3) = 0xFF → replicate element 3 (alpha) to all 4 positions.
    let alpha_lo = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);

    let inv_alpha_lo = _mm_sub_epi16(c255, alpha_lo);
    let val_lo = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_lo, alpha_lo), _mm_mullo_epi16(dst_lo, inv_alpha_lo)),
        c128,
    );
    let result_lo = _mm_srli_epi16(_mm_add_epi16(val_lo, _mm_srli_epi16(val_lo, 8)), 8);

    // --- High 2 pixels ---
    let src_hi = _mm_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm_unpackhi_epi8(dst4, zero);
    let src_orig_hi = _mm_unpackhi_epi8(src4, zero);
    let alpha_hi = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);

    let inv_alpha_hi = _mm_sub_epi16(c255, alpha_hi);
    let val_hi = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_hi, alpha_hi), _mm_mullo_epi16(dst_hi, inv_alpha_hi)),
        c128,
    );
    let result_hi = _mm_srli_epi16(_mm_add_epi16(val_hi, _mm_srli_epi16(val_hi, 8)), 8);

    // Pack back to u8 and store.
    _mm_storeu_si128(dst_ptr.cast::<__m128i>(), _mm_packus_epi16(result_lo, result_hi));
}

/// Blend 4 gathered source RGBA pixels onto 4 contiguous destination pixels
/// using SSE2 "over" compositing **with** an opacity multiplier applied to
/// each pixel's source alpha.
///
/// # Safety
///
/// `dst_ptr` must point to at least 16 writable bytes.
//
// NOTE: no `#[target_feature(enable = "sse2")]` — see comment on
// `blend_4px_opaque_sse2` for rationale.
#[inline(always)]
#[allow(clippy::cast_ptr_alignment)] // _mm_storeu/loadu_si128 do not require alignment
pub(super) unsafe fn blend_4px_alpha_sse2(dst_ptr: *mut u8, src_pixels: [u32; 4], opacity: u16) {
    use std::arch::x86_64::{
        __m128i, _mm_add_epi16, _mm_loadu_si128, _mm_mullo_epi16, _mm_or_si128, _mm_packus_epi16,
        _mm_set1_epi16, _mm_set1_epi32, _mm_set_epi32, _mm_setzero_si128, _mm_shufflehi_epi16,
        _mm_shufflelo_epi16, _mm_srli_epi16, _mm_storeu_si128, _mm_sub_epi16, _mm_unpackhi_epi8,
        _mm_unpacklo_epi8,
    };

    let zero = _mm_setzero_si128();
    let c255 = _mm_set1_epi16(255);
    let c128 = _mm_set1_epi16(128);
    let opacity_v = _mm_set1_epi16(opacity.cast_signed());

    let src4 = _mm_set_epi32(
        src_pixels[3].cast_signed(),
        src_pixels[2].cast_signed(),
        src_pixels[1].cast_signed(),
        src_pixels[0].cast_signed(),
    );

    let dst4 = _mm_loadu_si128(dst_ptr.cast::<__m128i>().cast_const());
    let alpha_byte_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
    let src_blend = _mm_or_si128(src4, alpha_byte_mask);

    // --- Low 2 pixels ---
    let src_lo = _mm_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm_unpacklo_epi8(dst4, zero);

    // Extract original alpha, apply opacity: sa_eff = (sa * opacity + 128) >> 8.
    // Max value: (255*255+128)>>8 = 254, so no clamping needed.
    let src_orig_lo = _mm_unpacklo_epi8(src4, zero);
    let raw_alpha_lo = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);
    let alpha_lo = _mm_srli_epi16(_mm_add_epi16(_mm_mullo_epi16(raw_alpha_lo, opacity_v), c128), 8);

    let inv_alpha_lo = _mm_sub_epi16(c255, alpha_lo);
    let val_lo = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_lo, alpha_lo), _mm_mullo_epi16(dst_lo, inv_alpha_lo)),
        c128,
    );
    let result_lo = _mm_srli_epi16(_mm_add_epi16(val_lo, _mm_srli_epi16(val_lo, 8)), 8);

    // --- High 2 pixels ---
    let src_hi = _mm_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm_unpackhi_epi8(dst4, zero);
    let src_orig_hi = _mm_unpackhi_epi8(src4, zero);
    let raw_alpha_hi = _mm_shufflehi_epi16(_mm_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);
    let alpha_hi = _mm_srli_epi16(_mm_add_epi16(_mm_mullo_epi16(raw_alpha_hi, opacity_v), c128), 8);

    let inv_alpha_hi = _mm_sub_epi16(c255, alpha_hi);
    let val_hi = _mm_add_epi16(
        _mm_add_epi16(_mm_mullo_epi16(src_hi, alpha_hi), _mm_mullo_epi16(dst_hi, inv_alpha_hi)),
        c128,
    );
    let result_hi = _mm_srli_epi16(_mm_add_epi16(val_hi, _mm_srli_epi16(val_hi, 8)), 8);

    _mm_storeu_si128(dst_ptr.cast::<__m128i>(), _mm_packus_epi16(result_lo, result_hi));
}

// ── AVX2 alpha-blend helpers ─────────────────────────────────────────────────
//
// Process 8 RGBA pixels at a time using AVX2 integer arithmetic.
// Same algorithm as the SSE2 helpers above, widened to 256-bit registers.

/// Blend 8 gathered source RGBA pixels onto 8 contiguous destination pixels
/// using AVX2 "over" compositing (no opacity modifier).
///
/// # Safety
///
/// `dst_ptr` must point to at least 32 writable bytes.  Source pixel values
/// in `src_pixels` must be valid RGBA `u32` values.
#[target_feature(enable = "avx2")]
#[inline]
#[allow(clippy::cast_ptr_alignment)]
pub(super) unsafe fn blend_8px_opaque_avx2(dst_ptr: *mut u8, src_pixels: [u32; 8]) {
    use std::arch::x86_64::{
        __m256i, _mm256_add_epi16, _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256,
        _mm256_movemask_epi8, _mm256_mullo_epi16, _mm256_or_si256, _mm256_packus_epi16,
        _mm256_set1_epi16, _mm256_set1_epi32, _mm256_set_epi32, _mm256_setzero_si256,
        _mm256_shufflehi_epi16, _mm256_shufflelo_epi16, _mm256_srli_epi16, _mm256_storeu_si256,
        _mm256_sub_epi16, _mm256_unpackhi_epi8, _mm256_unpacklo_epi8,
    };

    let zero = _mm256_setzero_si256();
    let c255 = _mm256_set1_epi16(255);
    let c128 = _mm256_set1_epi16(128);

    // Assemble 8 gathered source pixels into one 256-bit register.
    let src8 = _mm256_set_epi32(
        src_pixels[7].cast_signed(),
        src_pixels[6].cast_signed(),
        src_pixels[5].cast_signed(),
        src_pixels[4].cast_signed(),
        src_pixels[3].cast_signed(),
        src_pixels[2].cast_signed(),
        src_pixels[1].cast_signed(),
        src_pixels[0].cast_signed(),
    );

    let alpha_byte_mask = _mm256_set1_epi32(0xFF00_0000_u32.cast_signed());

    // Fast path: all 8 source pixels fully opaque → direct copy.
    let alpha_bytes = _mm256_and_si256(src8, alpha_byte_mask);
    if _mm256_movemask_epi8(_mm256_cmpeq_epi8(alpha_bytes, alpha_byte_mask)) == -1i32 {
        _mm256_storeu_si256(dst_ptr.cast::<__m256i>(), src8);
        return;
    }

    // Fast path: all 8 source pixels fully transparent → nothing to do.
    if _mm256_movemask_epi8(_mm256_cmpeq_epi8(alpha_bytes, zero)) == -1i32 {
        return;
    }

    let dst8 = _mm256_loadu_si256(dst_ptr.cast::<__m256i>().cast_const());
    let src_blend = _mm256_or_si256(src8, alpha_byte_mask);

    // --- Low 4 pixels (within each 128-bit lane) ---
    let src_lo = _mm256_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm256_unpacklo_epi8(dst8, zero);
    let src_orig_lo = _mm256_unpacklo_epi8(src8, zero);
    let alpha_lo = _mm256_shufflehi_epi16(_mm256_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);

    let inv_alpha_lo = _mm256_sub_epi16(c255, alpha_lo);
    let val_lo = _mm256_add_epi16(
        _mm256_add_epi16(
            _mm256_mullo_epi16(src_lo, alpha_lo),
            _mm256_mullo_epi16(dst_lo, inv_alpha_lo),
        ),
        c128,
    );
    let result_lo = _mm256_srli_epi16(_mm256_add_epi16(val_lo, _mm256_srli_epi16(val_lo, 8)), 8);

    // --- High 4 pixels (within each 128-bit lane) ---
    let src_hi = _mm256_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm256_unpackhi_epi8(dst8, zero);
    let src_orig_hi = _mm256_unpackhi_epi8(src8, zero);
    let alpha_hi = _mm256_shufflehi_epi16(_mm256_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);

    let inv_alpha_hi = _mm256_sub_epi16(c255, alpha_hi);
    let val_hi = _mm256_add_epi16(
        _mm256_add_epi16(
            _mm256_mullo_epi16(src_hi, alpha_hi),
            _mm256_mullo_epi16(dst_hi, inv_alpha_hi),
        ),
        c128,
    );
    let result_hi = _mm256_srli_epi16(_mm256_add_epi16(val_hi, _mm256_srli_epi16(val_hi, 8)), 8);

    // Pack back to u8.  `_mm256_packus_epi16` packs within each 128-bit
    // lane independently: lane0 gets [result_lo lanes 0-1], lane1 gets
    // [result_hi lanes 0-1].  This produces correct pixel order because
    // `unpacklo/unpackhi_epi8` also operated within lanes — so lane0
    // always holds pixels 0-3 and lane1 always holds pixels 4-7.
    let packed = _mm256_packus_epi16(result_lo, result_hi);
    _mm256_storeu_si256(dst_ptr.cast::<__m256i>(), packed);
}

/// Blend 8 gathered source RGBA pixels onto 8 contiguous destination pixels
/// using AVX2 "over" compositing **with** an opacity multiplier applied to
/// each pixel's source alpha.
///
/// # Safety
///
/// `dst_ptr` must point to at least 32 writable bytes.
#[target_feature(enable = "avx2")]
#[inline]
#[allow(clippy::cast_ptr_alignment)]
pub(super) unsafe fn blend_8px_alpha_avx2(dst_ptr: *mut u8, src_pixels: [u32; 8], opacity: u16) {
    use std::arch::x86_64::{
        __m256i, _mm256_add_epi16, _mm256_loadu_si256, _mm256_mullo_epi16, _mm256_or_si256,
        _mm256_packus_epi16, _mm256_set1_epi16, _mm256_set1_epi32, _mm256_set_epi32,
        _mm256_setzero_si256, _mm256_shufflehi_epi16, _mm256_shufflelo_epi16, _mm256_srli_epi16,
        _mm256_storeu_si256, _mm256_sub_epi16, _mm256_unpackhi_epi8, _mm256_unpacklo_epi8,
    };

    let zero = _mm256_setzero_si256();
    let c255 = _mm256_set1_epi16(255);
    let c128 = _mm256_set1_epi16(128);
    let opacity_v = _mm256_set1_epi16(opacity.cast_signed());

    let src8 = _mm256_set_epi32(
        src_pixels[7].cast_signed(),
        src_pixels[6].cast_signed(),
        src_pixels[5].cast_signed(),
        src_pixels[4].cast_signed(),
        src_pixels[3].cast_signed(),
        src_pixels[2].cast_signed(),
        src_pixels[1].cast_signed(),
        src_pixels[0].cast_signed(),
    );

    let dst8 = _mm256_loadu_si256(dst_ptr.cast::<__m256i>().cast_const());
    let alpha_byte_mask = _mm256_set1_epi32(0xFF00_0000_u32.cast_signed());
    let src_blend = _mm256_or_si256(src8, alpha_byte_mask);

    // --- Low 4 pixels ---
    let src_lo = _mm256_unpacklo_epi8(src_blend, zero);
    let dst_lo = _mm256_unpacklo_epi8(dst8, zero);
    let src_orig_lo = _mm256_unpacklo_epi8(src8, zero);
    let raw_alpha_lo = _mm256_shufflehi_epi16(_mm256_shufflelo_epi16(src_orig_lo, 0xFF), 0xFF);
    let alpha_lo =
        _mm256_srli_epi16(_mm256_add_epi16(_mm256_mullo_epi16(raw_alpha_lo, opacity_v), c128), 8);

    let inv_alpha_lo = _mm256_sub_epi16(c255, alpha_lo);
    let val_lo = _mm256_add_epi16(
        _mm256_add_epi16(
            _mm256_mullo_epi16(src_lo, alpha_lo),
            _mm256_mullo_epi16(dst_lo, inv_alpha_lo),
        ),
        c128,
    );
    let result_lo = _mm256_srli_epi16(_mm256_add_epi16(val_lo, _mm256_srli_epi16(val_lo, 8)), 8);

    // --- High 4 pixels ---
    let src_hi = _mm256_unpackhi_epi8(src_blend, zero);
    let dst_hi = _mm256_unpackhi_epi8(dst8, zero);
    let src_orig_hi = _mm256_unpackhi_epi8(src8, zero);
    let raw_alpha_hi = _mm256_shufflehi_epi16(_mm256_shufflelo_epi16(src_orig_hi, 0xFF), 0xFF);
    let alpha_hi =
        _mm256_srli_epi16(_mm256_add_epi16(_mm256_mullo_epi16(raw_alpha_hi, opacity_v), c128), 8);

    let inv_alpha_hi = _mm256_sub_epi16(c255, alpha_hi);
    let val_hi = _mm256_add_epi16(
        _mm256_add_epi16(
            _mm256_mullo_epi16(src_hi, alpha_hi),
            _mm256_mullo_epi16(dst_hi, inv_alpha_hi),
        ),
        c128,
    );
    let result_hi = _mm256_srli_epi16(_mm256_add_epi16(val_hi, _mm256_srli_epi16(val_hi, 8)), 8);

    // Same lane-local pack logic as `blend_8px_opaque_avx2`: unpack and
    // pack both operate within 128-bit lanes, preserving pixel order.
    let packed = _mm256_packus_epi16(result_lo, result_hi);
    _mm256_storeu_si256(dst_ptr.cast::<__m256i>(), packed);
}

// ── SIMD alpha-opaqueness check ─────────────────────────────────────────────

/// Check if all alpha bytes in an RGBA8 row are 0xFF using SSE2.
///
/// Processes 4 pixels (16 bytes) per iteration.  Returns `true` if every
/// pixel's alpha channel is 255.
///
/// # Safety
///
/// Caller must ensure `row` length is a multiple of 4 bytes (always true
/// for valid RGBA8 data).
#[target_feature(enable = "sse2")]
#[inline]
pub(super) unsafe fn all_alpha_opaque_sse2(row: &[u8]) -> bool {
    use std::arch::x86_64::{
        _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi32,
    };

    let alpha_mask = _mm_set1_epi32(0xFF00_0000_u32.cast_signed());
    let len = row.len();
    let simd_end = len & !15; // round down to multiple of 16 (4 pixels)
    let mut i = 0;

    while i < simd_end {
        let chunk = _mm_loadu_si128(row.as_ptr().add(i).cast());
        let alpha_bytes = _mm_and_si128(chunk, alpha_mask);
        // Check that all alpha-position bytes equal 0xFF.
        // After AND with mask, alpha positions have 0xFF if opaque.
        // cmpeq + movemask: if all 16 bytes match, mask == 0xFFFF.
        // But we only care about bytes 3,7,11,15 (alpha positions).
        // After AND, non-alpha bytes are 0x00; cmpeq with mask will set
        // those to 0x00 as well.  We want the alpha-position bits of the
        // movemask: bits 3,7,11,15 = 0x8888.
        if _mm_movemask_epi8(_mm_cmpeq_epi8(alpha_bytes, alpha_mask)) & 0x8888 != 0x8888 {
            return false;
        }
        i += 16;
    }

    // Scalar tail.
    while i + 3 < len {
        if row[i + 3] != 255 {
            return false;
        }
        i += 4;
    }
    true
}

/// Check if all alpha bytes in an RGBA8 row are 0xFF using AVX2.
///
/// Processes 8 pixels (32 bytes) per iteration.
///
/// # Safety
///
/// Caller must ensure `row` length is a multiple of 4 bytes.
#[target_feature(enable = "avx2")]
#[inline]
pub(super) unsafe fn all_alpha_opaque_avx2(row: &[u8]) -> bool {
    use std::arch::x86_64::{
        _mm256_and_si256, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8,
        _mm256_set1_epi32,
    };

    let alpha_mask = _mm256_set1_epi32(0xFF00_0000_u32.cast_signed());
    let len = row.len();
    let simd_end = len & !31; // round down to multiple of 32 (8 pixels)
    let mut i = 0;

    while i < simd_end {
        let chunk = _mm256_loadu_si256(row.as_ptr().add(i).cast());
        let alpha_bytes = _mm256_and_si256(chunk, alpha_mask);
        // Alpha-position bits: bytes 3,7,11,15,19,23,27,31 → mask bits 0x88888888.
        let cmp_mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(alpha_bytes, alpha_mask));
        if cmp_mask & 0x8888_8888_u32.cast_signed() != 0x8888_8888_u32.cast_signed() {
            return false;
        }
        i += 32;
    }

    // Scalar tail.
    while i + 3 < len {
        if row[i + 3] != 255 {
            return false;
        }
        i += 4;
    }
    true
}

// ── SSE2-compatible i32 multiply helper ─────────────────────────────────────

/// SSE2-compatible signed 32-bit multiply (low 32 bits of each lane).
///
/// SSE2 only has `_mm_mul_epu32` which multiplies lanes 0 and 2 as
/// unsigned 32-bit → 64-bit.  We use it twice (even + odd lanes) and
/// re-interleave to get all four i32 products.  The unsigned multiply
/// gives the correct low-32 result for signed operands (two's complement).
#[target_feature(enable = "sse2")]
#[inline]
pub(super) unsafe fn mul32_sse2(
    a: std::arch::x86_64::__m128i,
    b: std::arch::x86_64::__m128i,
) -> std::arch::x86_64::__m128i {
    use std::arch::x86_64::{_mm_mul_epu32, _mm_shuffle_epi32, _mm_unpacklo_epi32};
    // Multiply even lanes (0, 2) → 64-bit results.
    let even = _mm_mul_epu32(a, b);
    // Shuffle odd lanes (1, 3) into even positions, then multiply.
    let odd =
        _mm_mul_epu32(_mm_shuffle_epi32(a, 0b11_11_01_01), _mm_shuffle_epi32(b, 0b11_11_01_01));
    // Extract the low 32 bits of each 64-bit product:
    //   even = [p0_lo, p0_hi, p2_lo, p2_hi]
    //   odd  = [p1_lo, p1_hi, p3_lo, p3_hi]
    // shuffle 0b00_00_10_00 picks dwords 0 and 2 → [p_lo0, p_lo2, ?, ?]
    let even_lo = _mm_shuffle_epi32(even, 0b00_00_10_00);
    let odd_lo = _mm_shuffle_epi32(odd, 0b00_00_10_00);
    // Interleave low halves → [p0, p1, p2, p3].
    _mm_unpacklo_epi32(even_lo, odd_lo)
}

// ── I420 → RGBA8 SSE2/SSE4.1 (macro-generated) ─────────────────────────────

/// Generate an I420 → RGBA8 row conversion function for a given SIMD tier.
///
/// The multiply strategy is injected via `$mul32`: either `mul32_sse2`
/// (7-instruction emulation) or the native `_mm_mullo_epi32` wrapper.
macro_rules! impl_i420_to_rgba8_row {
    ($name:ident, $feature:literal, $mul32:expr) => {
        #[doc = concat!("Convert up to `width` I420 pixels from one row to RGBA8 using ", $feature, ".")]
        ///
        /// Returns the number of pixels converted (always a multiple of 4).
        /// The caller must handle the remaining `width - returned` tail pixels
        /// with the scalar path.
        #[target_feature(enable = $feature)]
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
        pub(super) unsafe fn $name(
            y_row: &[u8],
            u_row: &[u8],
            v_row: &[u8],
            rgba_out: &mut [u8],
            width: usize,
        ) -> usize {
            use std::arch::x86_64::{
                _mm_add_epi32, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi32,
                _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_storeu_si128,
                _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
            };

            let simd_width = width & !3;
            if simd_width == 0 {
                return 0;
            }

            let coeff_298 = _mm_set1_epi32(298);
            let coeff_409 = _mm_set1_epi32(409);
            let coeff_n100 = _mm_set1_epi32(-100);
            let coeff_n208 = _mm_set1_epi32(-208);
            let coeff_516 = _mm_set1_epi32(516);
            let bias_16 = _mm_set1_epi32(16);
            let bias_128 = _mm_set1_epi32(128);
            let rounding = _mm_set1_epi32(128);
            let zero = _mm_setzero_si128();

            let mul32 = $mul32;

            let mut col = 0usize;
            while col < simd_width {
                let y0 = i32::from(y_row[col]);
                let y1 = i32::from(y_row[col + 1]);
                let y2 = i32::from(y_row[col + 2]);
                let y3 = i32::from(y_row[col + 3]);
                let y32 = _mm_set_epi32(y3, y2, y1, y0);

                let chroma_col = col / 2;
                let u0 = i32::from(u_row[chroma_col]);
                let u1 = i32::from(u_row[chroma_col + 1]);
                let v0 = i32::from(v_row[chroma_col]);
                let v1 = i32::from(v_row[chroma_col + 1]);
                let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
                let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

                let c = _mm_sub_epi32(y32, bias_16);
                let d = _mm_sub_epi32(u32x4, bias_128);
                let e = _mm_sub_epi32(v32x4, bias_128);

                let r32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_409, e)),
                        rounding,
                    ),
                    8,
                );

                let g32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_n100, d)),
                            mul32(coeff_n208, e),
                        ),
                        rounding,
                    ),
                    8,
                );

                let b32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_516, d)),
                        rounding,
                    ),
                    8,
                );

                let r16 = _mm_packs_epi32(r32, zero);
                let g16 = _mm_packs_epi32(g32, zero);
                let b16 = _mm_packs_epi32(b32, zero);
                let r8 = _mm_packus_epi16(r16, zero);
                let g8 = _mm_packus_epi16(g16, zero);
                let b8 = _mm_packus_epi16(b16, zero);

                // Alpha channel is already 0xFF: `_mm_set1_epi8(-1)` in the
                // `ba` interleave places 0xFF in every alpha byte position.
                let rg = _mm_unpacklo_epi8(r8, g8);
                let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
                let rgba = _mm_unpacklo_epi16(rg, ba);

                let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
                _mm_storeu_si128(out_ptr.cast(), rgba);

                col += 4;
            }
            simd_width
        }
    };
}

// SSE2 wrapper: calls mul32_sse2 (7-instruction emulation).
impl_i420_to_rgba8_row!(i420_to_rgba8_row_sse2, "sse2", |a, b| mul32_sse2(a, b));

// SSE4.1 wrapper: uses native _mm_mullo_epi32.
impl_i420_to_rgba8_row!(i420_to_rgba8_row_sse41, "sse4.1", |a, b| {
    std::arch::x86_64::_mm_mullo_epi32(a, b)
});

// ── NV12 → RGBA8 SSE2/SSE4.1 (macro-generated) ─────────────────────────────

/// Generate an NV12 → RGBA8 row conversion function for a given SIMD tier.
macro_rules! impl_nv12_to_rgba8_row {
    ($name:ident, $feature:literal, $mul32:expr) => {
        #[doc = concat!("Convert up to `width` NV12 pixels from one row to RGBA8 using ", $feature, ".")]
        ///
        /// Returns the number of pixels converted (always a multiple of 4).
        #[target_feature(enable = $feature)]
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
        pub(super) unsafe fn $name(
            y_row: &[u8],
            uv_row: &[u8],
            rgba_out: &mut [u8],
            width: usize,
        ) -> usize {
            use std::arch::x86_64::{
                _mm_add_epi32, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi32,
                _mm_set1_epi8, _mm_set_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_storeu_si128,
                _mm_sub_epi32, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
            };

            let simd_width = width & !3;
            if simd_width == 0 {
                return 0;
            }

            let coeff_298 = _mm_set1_epi32(298);
            let coeff_409 = _mm_set1_epi32(409);
            let coeff_n100 = _mm_set1_epi32(-100);
            let coeff_n208 = _mm_set1_epi32(-208);
            let coeff_516 = _mm_set1_epi32(516);
            let bias_16 = _mm_set1_epi32(16);
            let bias_128 = _mm_set1_epi32(128);
            let rounding = _mm_set1_epi32(128);
            let zero = _mm_setzero_si128();

            let mul32 = $mul32;

            let mut col = 0usize;
            while col < simd_width {
                let y0 = i32::from(y_row[col]);
                let y1 = i32::from(y_row[col + 1]);
                let y2 = i32::from(y_row[col + 2]);
                let y3 = i32::from(y_row[col + 3]);
                let y32 = _mm_set_epi32(y3, y2, y1, y0);

                let chroma_byte = (col / 2) * 2;
                let u0 = i32::from(uv_row[chroma_byte]);
                let v0 = i32::from(uv_row[chroma_byte + 1]);
                let u1 = i32::from(uv_row[chroma_byte + 2]);
                let v1 = i32::from(uv_row[chroma_byte + 3]);
                let u32x4 = _mm_set_epi32(u1, u1, u0, u0);
                let v32x4 = _mm_set_epi32(v1, v1, v0, v0);

                let c = _mm_sub_epi32(y32, bias_16);
                let d = _mm_sub_epi32(u32x4, bias_128);
                let e = _mm_sub_epi32(v32x4, bias_128);

                let r32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_409, e)),
                        rounding,
                    ),
                    8,
                );

                let g32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(
                            _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_n100, d)),
                            mul32(coeff_n208, e),
                        ),
                        rounding,
                    ),
                    8,
                );

                let b32 = _mm_srai_epi32(
                    _mm_add_epi32(
                        _mm_add_epi32(mul32(coeff_298, c), mul32(coeff_516, d)),
                        rounding,
                    ),
                    8,
                );

                let r16 = _mm_packs_epi32(r32, zero);
                let g16 = _mm_packs_epi32(g32, zero);
                let b16 = _mm_packs_epi32(b32, zero);
                let r8 = _mm_packus_epi16(r16, zero);
                let g8 = _mm_packus_epi16(g16, zero);
                let b8 = _mm_packus_epi16(b16, zero);

                // Alpha channel is already 0xFF: `_mm_set1_epi8(-1)` in the
                // `ba` interleave places 0xFF in every alpha byte position.
                let rg = _mm_unpacklo_epi8(r8, g8);
                let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
                let rgba = _mm_unpacklo_epi16(rg, ba);

                let out_ptr = rgba_out.as_mut_ptr().add(col * 4);
                _mm_storeu_si128(out_ptr.cast(), rgba);

                col += 4;
            }
            simd_width
        }
    };
}

impl_nv12_to_rgba8_row!(nv12_to_rgba8_row_sse2, "sse2", |a, b| mul32_sse2(a, b));
impl_nv12_to_rgba8_row!(nv12_to_rgba8_row_sse41, "sse4.1", |a, b| {
    std::arch::x86_64::_mm_mullo_epi32(a, b)
});

// ── NV12 → RGBA8 AVX2 (8 pixels / iter) ────────────────────────────────────

/// Convert up to `width` NV12 pixels from one row to RGBA8 using AVX2.
///
/// Processes 8 pixels per iteration (256-bit registers) — double the
/// throughput of the SSE4.1 variant.  The heavy i32 multiplies run in
/// 256-bit lanes while the final u8 pack + RGBA interleave drops to
/// 128-bit SSE to avoid AVX2 lane-crossing headaches.
///
/// Returns the number of pixels converted (always a multiple of 8).
#[target_feature(enable = "avx2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn nv12_to_rgba8_row_avx2(
    y_row: &[u8],
    uv_row: &[u8],
    rgba_out: &mut [u8],
    width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm256_add_epi32, _mm256_castsi256_si128, _mm256_cvtepu8_epi32, _mm256_extracti128_si256,
        _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_srai_epi32, _mm256_sub_epi32,
        _mm_loadl_epi64, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi8, _mm_set_epi8,
        _mm_setzero_si128, _mm_shuffle_epi8, _mm_storeu_si128, _mm_unpacklo_epi16,
        _mm_unpacklo_epi8,
    };

    let simd_width = width & !7; // round down to multiple of 8
    if simd_width == 0 {
        return 0;
    }

    let coeff_298 = _mm256_set1_epi32(298);
    let coeff_409 = _mm256_set1_epi32(409);
    let coeff_n100 = _mm256_set1_epi32(-100);
    let coeff_n208 = _mm256_set1_epi32(-208);
    let coeff_516 = _mm256_set1_epi32(516);
    let bias_16 = _mm256_set1_epi32(16);
    let bias_128 = _mm256_set1_epi32(128);
    let rounding = _mm256_set1_epi32(128);
    let zero = _mm_setzero_si128();

    // Shuffle controls for deinterleaving + duplicating NV12 UV pairs.
    let u_shuf = _mm_set_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 6, 6, 4, 4, 2, 2, 0, 0);
    let v_shuf = _mm_set_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 7, 7, 5, 5, 3, 3, 1, 1);

    let mut col = 0usize;
    while col < simd_width {
        let y8 = _mm_loadl_epi64(y_row.as_ptr().add(col).cast());
        let y32 = _mm256_cvtepu8_epi32(y8);

        let chroma_byte = (col / 2) * 2;
        let uv8 = _mm_loadl_epi64(uv_row.as_ptr().add(chroma_byte).cast());
        let u32x8 = _mm256_cvtepu8_epi32(_mm_shuffle_epi8(uv8, u_shuf));
        let v32x8 = _mm256_cvtepu8_epi32(_mm_shuffle_epi8(uv8, v_shuf));

        let c = _mm256_sub_epi32(y32, bias_16);
        let d = _mm256_sub_epi32(u32x8, bias_128);
        let e = _mm256_sub_epi32(v32x8, bias_128);

        let r32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(coeff_298, c),
                    _mm256_mullo_epi32(coeff_409, e),
                ),
                rounding,
            ),
            8,
        );

        let g32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_mullo_epi32(coeff_298, c),
                        _mm256_mullo_epi32(coeff_n100, d),
                    ),
                    _mm256_mullo_epi32(coeff_n208, e),
                ),
                rounding,
            ),
            8,
        );

        let b32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(coeff_298, c),
                    _mm256_mullo_epi32(coeff_516, d),
                ),
                rounding,
            ),
            8,
        );

        // ── Pack + interleave: split into two 4-pixel halves ──────
        let r_lo = _mm256_castsi256_si128(r32);
        let r_hi = _mm256_extracti128_si256(r32, 1);
        let g_lo = _mm256_castsi256_si128(g32);
        let g_hi = _mm256_extracti128_si256(g32, 1);
        let b_lo = _mm256_castsi256_si128(b32);
        let b_hi = _mm256_extracti128_si256(b32, 1);

        // Alpha channel is already 0xFF: `_mm_set1_epi8(-1)` in the
        // `ba` interleave places 0xFF in every alpha byte position.

        // Pixels 0–3
        let r16 = _mm_packs_epi32(r_lo, zero);
        let g16 = _mm_packs_epi32(g_lo, zero);
        let b16 = _mm_packs_epi32(b_lo, zero);
        let r8 = _mm_packus_epi16(r16, zero);
        let g8 = _mm_packus_epi16(g16, zero);
        let b8 = _mm_packus_epi16(b16, zero);

        let rg = _mm_unpacklo_epi8(r8, g8);
        let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
        let rgba = _mm_unpacklo_epi16(rg, ba);
        _mm_storeu_si128(rgba_out.as_mut_ptr().add(col * 4).cast(), rgba);

        // Pixels 4–7
        let r16 = _mm_packs_epi32(r_hi, zero);
        let g16 = _mm_packs_epi32(g_hi, zero);
        let b16 = _mm_packs_epi32(b_hi, zero);
        let r8 = _mm_packus_epi16(r16, zero);
        let g8 = _mm_packus_epi16(g16, zero);
        let b8 = _mm_packus_epi16(b16, zero);

        let rg = _mm_unpacklo_epi8(r8, g8);
        let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
        let rgba = _mm_unpacklo_epi16(rg, ba);
        _mm_storeu_si128(rgba_out.as_mut_ptr().add((col + 4) * 4).cast(), rgba);

        col += 8;
    }
    simd_width
}

// ── I420 → RGBA8 AVX2 (8 pixels / iter) ─────────────────────────────────────

/// Convert up to `width` I420 pixels from one row to RGBA8 using AVX2.
///
/// Processes 8 pixels per iteration (256-bit registers) — double the
/// throughput of the SSE4.1 variant.  Same BT.601 math as the NV12 AVX2
/// variant, but reads U and V from separate planes instead of interleaved.
///
/// Returns the number of pixels converted (always a multiple of 8).
#[target_feature(enable = "avx2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn i420_to_rgba8_row_avx2(
    y_row: &[u8],
    u_row: &[u8],
    v_row: &[u8],
    rgba_out: &mut [u8],
    width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm256_add_epi32, _mm256_castsi256_si128, _mm256_cvtepu8_epi32, _mm256_extracti128_si256,
        _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_set_epi32, _mm256_srai_epi32,
        _mm256_sub_epi32, _mm_loadl_epi64, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi8,
        _mm_setzero_si128, _mm_storeu_si128, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
    };

    let simd_width = width & !7; // round down to multiple of 8
    if simd_width == 0 {
        return 0;
    }

    let coeff_298 = _mm256_set1_epi32(298);
    let coeff_409 = _mm256_set1_epi32(409);
    let coeff_n100 = _mm256_set1_epi32(-100);
    let coeff_n208 = _mm256_set1_epi32(-208);
    let coeff_516 = _mm256_set1_epi32(516);
    let bias_16 = _mm256_set1_epi32(16);
    let bias_128 = _mm256_set1_epi32(128);
    let rounding = _mm256_set1_epi32(128);
    let zero = _mm_setzero_si128();

    let mut col = 0usize;
    while col < simd_width {
        // Load 8 luma samples.
        let y8 = _mm_loadl_epi64(y_row.as_ptr().add(col).cast());
        let y32 = _mm256_cvtepu8_epi32(y8);

        // Load 4 U and 4 V chroma samples via scalar reads, duplicating each
        // to pair with 2 luma pixels.  We avoid `_mm_loadl_epi64` here because
        // the chroma planes may have only 4 bytes remaining at the last
        // iteration, and `_mm_loadl_epi64` always reads 8 bytes.
        let chroma_col = col / 2;
        let u0 = i32::from(u_row[chroma_col]);
        let u1 = i32::from(u_row[chroma_col + 1]);
        let u2 = i32::from(u_row[chroma_col + 2]);
        let u3 = i32::from(u_row[chroma_col + 3]);
        let u32x8 = _mm256_set_epi32(u3, u3, u2, u2, u1, u1, u0, u0);
        let v0 = i32::from(v_row[chroma_col]);
        let v1 = i32::from(v_row[chroma_col + 1]);
        let v2 = i32::from(v_row[chroma_col + 2]);
        let v3 = i32::from(v_row[chroma_col + 3]);
        let v32x8 = _mm256_set_epi32(v3, v3, v2, v2, v1, v1, v0, v0);

        let c = _mm256_sub_epi32(y32, bias_16);
        let d = _mm256_sub_epi32(u32x8, bias_128);
        let e = _mm256_sub_epi32(v32x8, bias_128);

        let r32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(coeff_298, c),
                    _mm256_mullo_epi32(coeff_409, e),
                ),
                rounding,
            ),
            8,
        );

        let g32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_mullo_epi32(coeff_298, c),
                        _mm256_mullo_epi32(coeff_n100, d),
                    ),
                    _mm256_mullo_epi32(coeff_n208, e),
                ),
                rounding,
            ),
            8,
        );

        let b32 = _mm256_srai_epi32(
            _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(coeff_298, c),
                    _mm256_mullo_epi32(coeff_516, d),
                ),
                rounding,
            ),
            8,
        );

        // ── Pack + interleave: split into two 4-pixel halves ──────
        let r_lo = _mm256_castsi256_si128(r32);
        let r_hi = _mm256_extracti128_si256(r32, 1);
        let g_lo = _mm256_castsi256_si128(g32);
        let g_hi = _mm256_extracti128_si256(g32, 1);
        let b_lo = _mm256_castsi256_si128(b32);
        let b_hi = _mm256_extracti128_si256(b32, 1);

        // Alpha channel is already 0xFF: `_mm_set1_epi8(-1)` in the
        // `ba` interleave places 0xFF in every alpha byte position.

        // Pixels 0–3
        let r16 = _mm_packs_epi32(r_lo, zero);
        let g16 = _mm_packs_epi32(g_lo, zero);
        let b16 = _mm_packs_epi32(b_lo, zero);
        let r8 = _mm_packus_epi16(r16, zero);
        let g8 = _mm_packus_epi16(g16, zero);
        let b8 = _mm_packus_epi16(b16, zero);

        let rg = _mm_unpacklo_epi8(r8, g8);
        let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
        let rgba = _mm_unpacklo_epi16(rg, ba);
        _mm_storeu_si128(rgba_out.as_mut_ptr().add(col * 4).cast(), rgba);

        // Pixels 4–7
        let r16 = _mm_packs_epi32(r_hi, zero);
        let g16 = _mm_packs_epi32(g_hi, zero);
        let b16 = _mm_packs_epi32(b_hi, zero);
        let r8 = _mm_packus_epi16(r16, zero);
        let g8 = _mm_packus_epi16(g16, zero);
        let b8 = _mm_packus_epi16(b16, zero);

        let rg = _mm_unpacklo_epi8(r8, g8);
        let ba = _mm_unpacklo_epi8(b8, _mm_set1_epi8(-1));
        let rgba = _mm_unpacklo_epi16(rg, ba);
        _mm_storeu_si128(rgba_out.as_mut_ptr().add((col + 4) * 4).cast(), rgba);

        col += 8;
    }
    simd_width
}

// ── RGBA8 → Y-plane SSE2/SSE4.1 (macro-generated) ──────────────────────────

/// Generate an RGBA8 → Y-plane row conversion function for a given SIMD tier.
macro_rules! impl_rgba8_to_y_row {
    ($name:ident, $feature:literal, $mul32:expr) => {
        #[doc = concat!("Convert one row of RGBA8 pixels to Y values using ", $feature, ".")]
        ///
        /// Returns the number of pixels converted (multiple of 4).
        #[target_feature(enable = $feature)]
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        pub(super) unsafe fn $name(rgba_row: &[u8], y_out: &mut [u8], width: usize) -> usize {
            use std::arch::x86_64::{
                _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_packs_epi32, _mm_packus_epi16,
                _mm_set1_epi32, _mm_setzero_si128, _mm_srai_epi32, _mm_srli_epi32, _mm_storeu_si32,
            };

            let simd_width = width & !3;
            if simd_width == 0 {
                return 0;
            }

            let coeff_66 = _mm_set1_epi32(66);
            let coeff_129 = _mm_set1_epi32(129);
            let coeff_25 = _mm_set1_epi32(25);
            let rounding = _mm_set1_epi32(128);
            let bias_16 = _mm_set1_epi32(16);
            let zero = _mm_setzero_si128();
            let channel_mask = _mm_set1_epi32(0xFF);

            let mul32 = $mul32;

            let mut col = 0usize;
            while col < simd_width {
                let src_ptr = rgba_row.as_ptr().add(col * 4);
                let px = _mm_loadu_si128(src_ptr.cast());

                let r = _mm_and_si128(px, channel_mask);
                let g = _mm_and_si128(_mm_srli_epi32(px, 8), channel_mask);
                let b = _mm_and_si128(_mm_srli_epi32(px, 16), channel_mask);

                let y32 = _mm_add_epi32(
                    _mm_srai_epi32(
                        _mm_add_epi32(
                            _mm_add_epi32(
                                _mm_add_epi32(mul32(coeff_66, r), mul32(coeff_129, g)),
                                mul32(coeff_25, b),
                            ),
                            rounding,
                        ),
                        8,
                    ),
                    bias_16,
                );

                let y16 = _mm_packs_epi32(y32, zero);
                let y8 = _mm_packus_epi16(y16, zero);
                _mm_storeu_si32(y_out.as_mut_ptr().add(col).cast(), y8);

                col += 4;
            }
            simd_width
        }
    };
}

impl_rgba8_to_y_row!(rgba8_to_y_row_sse2, "sse2", |a, b| mul32_sse2(a, b));
impl_rgba8_to_y_row!(rgba8_to_y_row_sse41, "sse4.1", |a, b| {
    std::arch::x86_64::_mm_mullo_epi32(a, b)
});

// ── RGBA8 → Y-plane AVX2 (8 pixels / iter) ─────────────────────────────────

/// Convert one row of RGBA8 pixels to Y values using AVX2.
///
/// Processes 8 pixels per iteration (256-bit registers) — double the
/// throughput of the SSE4.1 variant.
///
/// Returns the number of pixels converted (multiple of 8).
#[target_feature(enable = "avx2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) unsafe fn rgba8_to_y_row_avx2(rgba_row: &[u8], y_out: &mut [u8], width: usize) -> usize {
    use std::arch::x86_64::{
        _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128, _mm256_extracti128_si256,
        _mm256_loadu_si256, _mm256_mullo_epi32, _mm256_packs_epi32, _mm256_packus_epi16,
        _mm256_set1_epi32, _mm256_setzero_si256, _mm256_srai_epi32, _mm256_srli_epi32,
        _mm_storel_epi64, _mm_unpacklo_epi32,
    };

    let simd_width = width & !7;
    if simd_width == 0 {
        return 0;
    }

    let coeff_66 = _mm256_set1_epi32(66);
    let coeff_129 = _mm256_set1_epi32(129);
    let coeff_25 = _mm256_set1_epi32(25);
    let rounding = _mm256_set1_epi32(128);
    let bias_16 = _mm256_set1_epi32(16);
    let zero = _mm256_setzero_si256();
    let channel_mask = _mm256_set1_epi32(0xFF);

    let mut col = 0usize;
    while col < simd_width {
        let src_ptr = rgba_row.as_ptr().add(col * 4);
        let px = _mm256_loadu_si256(src_ptr.cast());

        let r = _mm256_and_si256(px, channel_mask);
        let g = _mm256_and_si256(_mm256_srli_epi32(px, 8), channel_mask);
        let b = _mm256_and_si256(_mm256_srli_epi32(px, 16), channel_mask);

        let y32 = _mm256_add_epi32(
            _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(
                        _mm256_add_epi32(
                            _mm256_mullo_epi32(coeff_66, r),
                            _mm256_mullo_epi32(coeff_129, g),
                        ),
                        _mm256_mullo_epi32(coeff_25, b),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_16,
        );

        let y16 = _mm256_packs_epi32(y32, zero);
        let y8 = _mm256_packus_epi16(y16, zero);
        let lo = _mm256_castsi256_si128(y8);
        let hi = _mm256_extracti128_si256(y8, 1);
        let combined = _mm_unpacklo_epi32(lo, hi);
        _mm_storel_epi64(y_out.as_mut_ptr().add(col).cast(), combined);

        col += 8;
    }
    simd_width
}

// ── RGBA8 → I420 chroma row (SSE2: 4 chroma samples / iter) ────────────────

/// Convert one pair of RGBA8 rows to U and V chroma samples using SSE2.
///
/// Returns the number of chroma samples converted (multiple of 4).
#[target_feature(enable = "sse2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn rgba8_to_chroma_row_sse2(
    rgba_row0: &[u8],
    rgba_row1: &[u8],
    u_out: &mut [u8],
    v_out: &mut [u8],
    chroma_width: usize,
    luma_width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm_add_epi16, _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi16,
        _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32, _mm_set_epi16,
        _mm_setzero_si128, _mm_srai_epi16, _mm_srli_epi32, _mm_storeu_si32,
    };

    let simd_width = chroma_width & !3;
    if simd_width == 0 || luma_width < 8 {
        return 0;
    }

    let coeff_cb_r = _mm_set1_epi16(-38);
    let coeff_cb_g = _mm_set1_epi16(-74);
    let coeff_cb_b = _mm_set1_epi16(112);
    let coeff_cr_r = _mm_set1_epi16(112);
    let coeff_cr_g = _mm_set1_epi16(-94);
    let coeff_cr_b = _mm_set1_epi16(-18);
    let rounding = _mm_set1_epi16(128);
    let bias_128 = _mm_set1_epi16(128);
    let zero = _mm_setzero_si128();
    let channel_mask = _mm_set1_epi32(0xFF);

    let mut ccol = 0usize;
    while ccol < simd_width {
        let luma_col = ccol * 2;
        if luma_col + 8 > luma_width {
            break;
        }

        let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
        let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
        let px0_lo = _mm_loadu_si128(ptr0.cast());
        let px0_hi = _mm_loadu_si128(ptr0.add(16).cast());
        let px1_lo = _mm_loadu_si128(ptr1.cast());
        let px1_hi = _mm_loadu_si128(ptr1.add(16).cast());

        // 2×2 average for R channel.
        let r0_lo = _mm_and_si128(px0_lo, channel_mask);
        let r0_hi = _mm_and_si128(px0_hi, channel_mask);
        let r1_lo = _mm_and_si128(px1_lo, channel_mask);
        let r1_hi = _mm_and_si128(px1_hi, channel_mask);
        let r_v_lo = _mm_add_epi32(r0_lo, r1_lo);
        let r_v_hi = _mm_add_epi32(r0_hi, r1_hi);
        let r_v = _mm_packs_epi32(r_v_lo, r_v_hi);
        let r_even = _mm_and_si128(r_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let r_odd = _mm_srli_epi32(r_v, 16);
        let r_sum = _mm_add_epi16(r_even, r_odd);
        let r_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(r_sum, zero), _mm_set1_epi16(2)), 2);

        // G channel.
        let g0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 8), channel_mask);
        let g0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 8), channel_mask);
        let g1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 8), channel_mask);
        let g1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 8), channel_mask);
        let g_v_lo = _mm_add_epi32(g0_lo, g1_lo);
        let g_v_hi = _mm_add_epi32(g0_hi, g1_hi);
        let g_v = _mm_packs_epi32(g_v_lo, g_v_hi);
        let g_even = _mm_and_si128(g_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let g_odd = _mm_srli_epi32(g_v, 16);
        let g_sum = _mm_add_epi16(g_even, g_odd);
        let g_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(g_sum, zero), _mm_set1_epi16(2)), 2);

        // B channel.
        let b0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 16), channel_mask);
        let b0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 16), channel_mask);
        let b1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 16), channel_mask);
        let b1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 16), channel_mask);
        let b_v_lo = _mm_add_epi32(b0_lo, b1_lo);
        let b_v_hi = _mm_add_epi32(b0_hi, b1_hi);
        let b_v = _mm_packs_epi32(b_v_lo, b_v_hi);
        let b_even = _mm_and_si128(b_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let b_odd = _mm_srli_epi32(b_v, 16);
        let b_sum = _mm_add_epi16(b_even, b_odd);
        let b_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(b_sum, zero), _mm_set1_epi16(2)), 2);

        // Cb / Cr coefficient multiplies.
        let cb_result = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_add_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_mullo_epi16(coeff_cb_r, r_avg),
                            _mm_mullo_epi16(coeff_cb_g, g_avg),
                        ),
                        _mm_mullo_epi16(coeff_cb_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cr_result = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_add_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_mullo_epi16(coeff_cr_r, r_avg),
                            _mm_mullo_epi16(coeff_cr_g, g_avg),
                        ),
                        _mm_mullo_epi16(coeff_cr_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cb_packed = _mm_packus_epi16(cb_result, zero);
        let cr_packed = _mm_packus_epi16(cr_result, zero);

        _mm_storeu_si32(u_out.as_mut_ptr().add(ccol).cast(), cb_packed);
        _mm_storeu_si32(v_out.as_mut_ptr().add(ccol).cast(), cr_packed);

        ccol += 4;
    }
    ccol
}

// ── RGBA8 → NV12 chroma row (SSE2: 4 interleaved UV pairs / iter) ──────────

/// Convert one pair of RGBA8 rows to interleaved `[U, V, U, V, …]`
/// chroma samples for NV12 output, using SSE2.
///
/// Returns the number of chroma *pairs* converted (multiple of 4).
#[target_feature(enable = "sse2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn rgba8_to_chroma_row_nv12_sse2(
    rgba_row0: &[u8],
    rgba_row1: &[u8],
    uv_out: &mut [u8],
    chroma_width: usize,
    luma_width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm_add_epi16, _mm_add_epi32, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi16,
        _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32, _mm_set_epi16,
        _mm_setzero_si128, _mm_srai_epi16, _mm_srli_epi32, _mm_storel_epi64, _mm_unpacklo_epi8,
    };

    let simd_width = chroma_width & !3;
    if simd_width == 0 || luma_width < 8 {
        return 0;
    }

    let coeff_cb_r = _mm_set1_epi16(-38);
    let coeff_cb_g = _mm_set1_epi16(-74);
    let coeff_cb_b = _mm_set1_epi16(112);
    let coeff_cr_r = _mm_set1_epi16(112);
    let coeff_cr_g = _mm_set1_epi16(-94);
    let coeff_cr_b = _mm_set1_epi16(-18);
    let rounding = _mm_set1_epi16(128);
    let bias_128 = _mm_set1_epi16(128);
    let zero = _mm_setzero_si128();
    let channel_mask = _mm_set1_epi32(0xFF);

    let mut ccol = 0usize;
    while ccol < simd_width {
        let luma_col = ccol * 2;
        if luma_col + 8 > luma_width {
            break;
        }

        let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
        let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
        let px0_lo = _mm_loadu_si128(ptr0.cast());
        let px0_hi = _mm_loadu_si128(ptr0.add(16).cast());
        let px1_lo = _mm_loadu_si128(ptr1.cast());
        let px1_hi = _mm_loadu_si128(ptr1.add(16).cast());

        // 2×2 average for R channel.
        let r0_lo = _mm_and_si128(px0_lo, channel_mask);
        let r0_hi = _mm_and_si128(px0_hi, channel_mask);
        let r1_lo = _mm_and_si128(px1_lo, channel_mask);
        let r1_hi = _mm_and_si128(px1_hi, channel_mask);
        let r_v_lo = _mm_add_epi32(r0_lo, r1_lo);
        let r_v_hi = _mm_add_epi32(r0_hi, r1_hi);
        let r_v = _mm_packs_epi32(r_v_lo, r_v_hi);
        let r_even = _mm_and_si128(r_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let r_odd = _mm_srli_epi32(r_v, 16);
        let r_sum = _mm_add_epi16(r_even, r_odd);
        let r_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(r_sum, zero), _mm_set1_epi16(2)), 2);

        // G channel.
        let g0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 8), channel_mask);
        let g0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 8), channel_mask);
        let g1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 8), channel_mask);
        let g1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 8), channel_mask);
        let g_v_lo = _mm_add_epi32(g0_lo, g1_lo);
        let g_v_hi = _mm_add_epi32(g0_hi, g1_hi);
        let g_v = _mm_packs_epi32(g_v_lo, g_v_hi);
        let g_even = _mm_and_si128(g_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let g_odd = _mm_srli_epi32(g_v, 16);
        let g_sum = _mm_add_epi16(g_even, g_odd);
        let g_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(g_sum, zero), _mm_set1_epi16(2)), 2);

        // B channel.
        let b0_lo = _mm_and_si128(_mm_srli_epi32(px0_lo, 16), channel_mask);
        let b0_hi = _mm_and_si128(_mm_srli_epi32(px0_hi, 16), channel_mask);
        let b1_lo = _mm_and_si128(_mm_srli_epi32(px1_lo, 16), channel_mask);
        let b1_hi = _mm_and_si128(_mm_srli_epi32(px1_hi, 16), channel_mask);
        let b_v_lo = _mm_add_epi32(b0_lo, b1_lo);
        let b_v_hi = _mm_add_epi32(b0_hi, b1_hi);
        let b_v = _mm_packs_epi32(b_v_lo, b_v_hi);
        let b_even = _mm_and_si128(b_v, _mm_set_epi16(0, -1, 0, -1, 0, -1, 0, -1));
        let b_odd = _mm_srli_epi32(b_v, 16);
        let b_sum = _mm_add_epi16(b_even, b_odd);
        let b_avg =
            _mm_srai_epi16(_mm_add_epi16(_mm_packs_epi32(b_sum, zero), _mm_set1_epi16(2)), 2);

        // Cb / Cr coefficient multiplies.
        let cb_result = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_add_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_mullo_epi16(coeff_cb_r, r_avg),
                            _mm_mullo_epi16(coeff_cb_g, g_avg),
                        ),
                        _mm_mullo_epi16(coeff_cb_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cr_result = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_add_epi16(
                    _mm_add_epi16(
                        _mm_add_epi16(
                            _mm_mullo_epi16(coeff_cr_r, r_avg),
                            _mm_mullo_epi16(coeff_cr_g, g_avg),
                        ),
                        _mm_mullo_epi16(coeff_cr_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        // Pack to u8 and interleave: [U0,V0,U1,V1,U2,V2,U3,V3].
        let cb_packed = _mm_packus_epi16(cb_result, zero);
        let cr_packed = _mm_packus_epi16(cr_result, zero);
        let interleaved = _mm_unpacklo_epi8(cb_packed, cr_packed);

        _mm_storel_epi64(uv_out.as_mut_ptr().add(ccol * 2).cast(), interleaved);

        ccol += 4;
    }
    ccol
}

// ── RGBA8 → NV12 chroma row (AVX2: 8 interleaved UV pairs / iter) ──────────

/// Convert one pair of RGBA8 rows to interleaved `[U, V, U, V, …]`
/// chroma samples for NV12 output, using AVX2.
///
/// Processes 8 chroma pairs (16 luma pixels) per iteration.
///
/// Returns the number of chroma *pairs* converted (multiple of 8).
#[target_feature(enable = "avx2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn rgba8_to_chroma_row_nv12_avx2(
    rgba_row0: &[u8],
    rgba_row1: &[u8],
    uv_out: &mut [u8],
    chroma_width: usize,
    luma_width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm256_add_epi16, _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128,
        _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_mullo_epi16, _mm256_packs_epi32,
        _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_set1_epi16, _mm256_set1_epi32,
        _mm256_set_epi16, _mm256_setzero_si256, _mm256_srai_epi16, _mm256_srli_epi32,
        _mm_storeu_si128, _mm_unpacklo_epi32, _mm_unpacklo_epi8,
    };

    let simd_width = chroma_width & !7;
    if simd_width == 0 || luma_width < 16 {
        return 0;
    }

    let coeff_cb_r = _mm256_set1_epi16(-38);
    let coeff_cb_g = _mm256_set1_epi16(-74);
    let coeff_cb_b = _mm256_set1_epi16(112);
    let coeff_cr_r = _mm256_set1_epi16(112);
    let coeff_cr_g = _mm256_set1_epi16(-94);
    let coeff_cr_b = _mm256_set1_epi16(-18);
    let rounding = _mm256_set1_epi16(128);
    let bias_128 = _mm256_set1_epi16(128);
    let zero = _mm256_setzero_si256();
    let channel_mask = _mm256_set1_epi32(0xFF);
    let even_mask = _mm256_set_epi16(0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1);

    let mut ccol = 0usize;
    while ccol < simd_width {
        let luma_col = ccol * 2;
        if luma_col + 16 > luma_width {
            break;
        }

        let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
        let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
        let px0_a = _mm256_loadu_si256(ptr0.cast());
        let px0_b = _mm256_loadu_si256(ptr0.add(32).cast());
        let px1_a = _mm256_loadu_si256(ptr1.cast());
        let px1_b = _mm256_loadu_si256(ptr1.add(32).cast());

        // 2×2 average for R channel.
        let r0_a = _mm256_and_si256(px0_a, channel_mask);
        let r0_b = _mm256_and_si256(px0_b, channel_mask);
        let r1_a = _mm256_and_si256(px1_a, channel_mask);
        let r1_b = _mm256_and_si256(px1_b, channel_mask);
        let r_v_a = _mm256_add_epi32(r0_a, r1_a);
        let r_v_b = _mm256_add_epi32(r0_b, r1_b);
        let r_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(r_v_a, r_v_b), 0xD8);
        let r_even = _mm256_and_si256(r_v, even_mask);
        let r_odd = _mm256_srli_epi32(r_v, 16);
        let r_sum = _mm256_add_epi16(r_even, r_odd);
        let r_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(r_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // G channel.
        let g0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 8), channel_mask);
        let g0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 8), channel_mask);
        let g1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 8), channel_mask);
        let g1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 8), channel_mask);
        let g_v_a = _mm256_add_epi32(g0_a, g1_a);
        let g_v_b = _mm256_add_epi32(g0_b, g1_b);
        let g_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(g_v_a, g_v_b), 0xD8);
        let g_even = _mm256_and_si256(g_v, even_mask);
        let g_odd = _mm256_srli_epi32(g_v, 16);
        let g_sum = _mm256_add_epi16(g_even, g_odd);
        let g_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(g_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // B channel.
        let b0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 16), channel_mask);
        let b0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 16), channel_mask);
        let b1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 16), channel_mask);
        let b1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 16), channel_mask);
        let b_v_a = _mm256_add_epi32(b0_a, b1_a);
        let b_v_b = _mm256_add_epi32(b0_b, b1_b);
        let b_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(b_v_a, b_v_b), 0xD8);
        let b_even = _mm256_and_si256(b_v, even_mask);
        let b_odd = _mm256_srli_epi32(b_v, 16);
        let b_sum = _mm256_add_epi16(b_even, b_odd);
        let b_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(b_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // Cb / Cr coefficient multiplies.
        let cb_result = _mm256_add_epi16(
            _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_mullo_epi16(coeff_cb_r, r_avg),
                            _mm256_mullo_epi16(coeff_cb_g, g_avg),
                        ),
                        _mm256_mullo_epi16(coeff_cb_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cr_result = _mm256_add_epi16(
            _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_mullo_epi16(coeff_cr_r, r_avg),
                            _mm256_mullo_epi16(coeff_cr_g, g_avg),
                        ),
                        _mm256_mullo_epi16(coeff_cr_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cb_packed = _mm256_packus_epi16(cb_result, zero);
        let cr_packed = _mm256_packus_epi16(cr_result, zero);
        let cb_lo = _mm256_castsi256_si128(cb_packed);
        let cb_hi = _mm256_extracti128_si256(cb_packed, 1);
        let cr_lo = _mm256_castsi256_si128(cr_packed);
        let cr_hi = _mm256_extracti128_si256(cr_packed, 1);
        let cb8 = _mm_unpacklo_epi32(cb_lo, cb_hi);
        let cr8 = _mm_unpacklo_epi32(cr_lo, cr_hi);
        let interleaved = _mm_unpacklo_epi8(cb8, cr8);

        _mm_storeu_si128(uv_out.as_mut_ptr().add(ccol * 2).cast(), interleaved);

        ccol += 8;
    }
    ccol
}

// ── RGBA8 → I420 chroma row (AVX2: 8 chroma samples / iter) ────────────────

/// Convert one pair of RGBA8 rows to U and V chroma samples using AVX2.
///
/// Processes 8 chroma samples (16 luma pixels) per iteration.
///
/// Returns the number of chroma samples converted (multiple of 8).
#[target_feature(enable = "avx2")]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::similar_names)]
pub(super) unsafe fn rgba8_to_chroma_row_avx2(
    rgba_row0: &[u8],
    rgba_row1: &[u8],
    u_out: &mut [u8],
    v_out: &mut [u8],
    chroma_width: usize,
    luma_width: usize,
) -> usize {
    use std::arch::x86_64::{
        _mm256_add_epi16, _mm256_add_epi32, _mm256_and_si256, _mm256_castsi256_si128,
        _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_mullo_epi16, _mm256_packs_epi32,
        _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_set1_epi16, _mm256_set1_epi32,
        _mm256_set_epi16, _mm256_setzero_si256, _mm256_srai_epi16, _mm256_srli_epi32,
        _mm_storel_epi64, _mm_unpacklo_epi32,
    };

    let simd_width = chroma_width & !7;
    if simd_width == 0 || luma_width < 16 {
        return 0;
    }

    let coeff_cb_r = _mm256_set1_epi16(-38);
    let coeff_cb_g = _mm256_set1_epi16(-74);
    let coeff_cb_b = _mm256_set1_epi16(112);
    let coeff_cr_r = _mm256_set1_epi16(112);
    let coeff_cr_g = _mm256_set1_epi16(-94);
    let coeff_cr_b = _mm256_set1_epi16(-18);
    let rounding = _mm256_set1_epi16(128);
    let bias_128 = _mm256_set1_epi16(128);
    let zero = _mm256_setzero_si256();
    let channel_mask = _mm256_set1_epi32(0xFF);
    let even_mask = _mm256_set_epi16(0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1);

    let mut ccol = 0usize;
    while ccol < simd_width {
        let luma_col = ccol * 2;
        if luma_col + 16 > luma_width {
            break;
        }

        let ptr0 = rgba_row0.as_ptr().add(luma_col * 4);
        let ptr1 = rgba_row1.as_ptr().add(luma_col * 4);
        let px0_a = _mm256_loadu_si256(ptr0.cast());
        let px0_b = _mm256_loadu_si256(ptr0.add(32).cast());
        let px1_a = _mm256_loadu_si256(ptr1.cast());
        let px1_b = _mm256_loadu_si256(ptr1.add(32).cast());

        // 2×2 average for R channel.
        let r0_a = _mm256_and_si256(px0_a, channel_mask);
        let r0_b = _mm256_and_si256(px0_b, channel_mask);
        let r1_a = _mm256_and_si256(px1_a, channel_mask);
        let r1_b = _mm256_and_si256(px1_b, channel_mask);
        let r_v_a = _mm256_add_epi32(r0_a, r1_a);
        let r_v_b = _mm256_add_epi32(r0_b, r1_b);
        let r_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(r_v_a, r_v_b), 0xD8);
        let r_even = _mm256_and_si256(r_v, even_mask);
        let r_odd = _mm256_srli_epi32(r_v, 16);
        let r_sum = _mm256_add_epi16(r_even, r_odd);
        let r_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(r_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // G channel.
        let g0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 8), channel_mask);
        let g0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 8), channel_mask);
        let g1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 8), channel_mask);
        let g1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 8), channel_mask);
        let g_v_a = _mm256_add_epi32(g0_a, g1_a);
        let g_v_b = _mm256_add_epi32(g0_b, g1_b);
        let g_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(g_v_a, g_v_b), 0xD8);
        let g_even = _mm256_and_si256(g_v, even_mask);
        let g_odd = _mm256_srli_epi32(g_v, 16);
        let g_sum = _mm256_add_epi16(g_even, g_odd);
        let g_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(g_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // B channel.
        let b0_a = _mm256_and_si256(_mm256_srli_epi32(px0_a, 16), channel_mask);
        let b0_b = _mm256_and_si256(_mm256_srli_epi32(px0_b, 16), channel_mask);
        let b1_a = _mm256_and_si256(_mm256_srli_epi32(px1_a, 16), channel_mask);
        let b1_b = _mm256_and_si256(_mm256_srli_epi32(px1_b, 16), channel_mask);
        let b_v_a = _mm256_add_epi32(b0_a, b1_a);
        let b_v_b = _mm256_add_epi32(b0_b, b1_b);
        let b_v = _mm256_permute4x64_epi64(_mm256_packs_epi32(b_v_a, b_v_b), 0xD8);
        let b_even = _mm256_and_si256(b_v, even_mask);
        let b_odd = _mm256_srli_epi32(b_v, 16);
        let b_sum = _mm256_add_epi16(b_even, b_odd);
        let b_avg = _mm256_srai_epi16(
            _mm256_add_epi16(_mm256_packs_epi32(b_sum, zero), _mm256_set1_epi16(2)),
            2,
        );

        // Cb / Cr coefficient multiplies.
        let cb_result = _mm256_add_epi16(
            _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_mullo_epi16(coeff_cb_r, r_avg),
                            _mm256_mullo_epi16(coeff_cb_g, g_avg),
                        ),
                        _mm256_mullo_epi16(coeff_cb_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cr_result = _mm256_add_epi16(
            _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(
                            _mm256_mullo_epi16(coeff_cr_r, r_avg),
                            _mm256_mullo_epi16(coeff_cr_g, g_avg),
                        ),
                        _mm256_mullo_epi16(coeff_cr_b, b_avg),
                    ),
                    rounding,
                ),
                8,
            ),
            bias_128,
        );

        let cb_packed = _mm256_packus_epi16(cb_result, zero);
        let cr_packed = _mm256_packus_epi16(cr_result, zero);
        let cb_lo = _mm256_castsi256_si128(cb_packed);
        let cb_hi = _mm256_extracti128_si256(cb_packed, 1);
        let cr_lo = _mm256_castsi256_si128(cr_packed);
        let cr_hi = _mm256_extracti128_si256(cr_packed, 1);
        let cb8 = _mm_unpacklo_epi32(cb_lo, cb_hi);
        let cr8 = _mm_unpacklo_epi32(cr_lo, cr_hi);

        _mm_storel_epi64(u_out.as_mut_ptr().add(ccol).cast(), cb8);
        _mm_storel_epi64(v_out.as_mut_ptr().add(ccol).cast(), cr8);

        ccol += 8;
    }
    ccol
}
