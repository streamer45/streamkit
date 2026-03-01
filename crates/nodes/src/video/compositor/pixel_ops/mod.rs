// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Pixel-level operations for the video compositor.
//!
//! Contains RGBA8 blitting (with nearest-neighbor scaling), alpha blending,
//! overlay compositing, and I420 / NV12 ↔ RGBA8 colour-space conversion.
//!
//! All hot loops use row-level parallelism via `rayon` when the region is
//! large enough to amortise the thread-pool dispatch overhead.  Below the
//! threshold the same per-row closures run sequentially.
//!
//! # Module structure
//!
//! - [`blit`] — axis-aligned and rotated scale + blit operations.
//! - [`convert`] — colour-space conversion (I420, NV12 ↔ RGBA8).
//! - [`simd`] (x86-64 only) — SIMD kernels for both blitting and conversion.

mod blit;
mod convert;

#[cfg(target_arch = "x86_64")]
mod simd_x86_64;

/// Re-export the x86-64 SIMD module under a shorter name for internal use.
#[cfg(target_arch = "x86_64")]
use simd_x86_64 as simd;

// ── Shared constants and helpers ────────────────────────────────────────────

/// Minimum number of output rows before we dispatch to rayon.  Below this
/// threshold the per-row work is small enough that the rayon scheduling
/// overhead (work-stealing queue push/pop, thread wake-up) dominates.
/// 64 rows at 1280-wide RGBA8 ≈ 320 KiB — a reasonable crossover point
/// on modern x86-64 cores.
const RAYON_ROW_THRESHOLD: usize = 64;

/// Number of rows to bundle into a single rayon task once parallel mode is
/// entered.  Reduces work-stealing overhead from ~1 task/row to
/// ~rows/chunk tasks.
///
/// [`rayon_chunk_rows`] auto-tunes the chunk size based on workload:
/// wider or taller frames produce fewer, larger chunks, keeping
/// scheduling cost proportional to the actual parallelism available.
///
/// Formula: `max(8, total_rows / (num_cpus * 4))`, clamped to `[8, 64]`.
/// This keeps chunk counts proportional to hardware parallelism while
/// avoiding both excessive scheduling overhead (too many tiny chunks)
/// and poor load-balancing (too few large chunks).
///
/// The CPU count is cached in a `LazyLock` so we avoid a `sysconf` syscall
/// (~40 µs on Linux) on every call.
fn rayon_chunk_rows(total_rows: usize) -> usize {
    static CPUS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        std::thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(1)
    });
    let ideal = total_rows.div_ceil(*CPUS * 4);
    ideal.clamp(8, 64)
}

/// Fixed-point alpha blend: `(src * alpha + dst * (255 - alpha) + 128) / 255`
/// using the well-known `((x + (x >> 8)) >> 8)` fast approximation of `x / 255`.
#[allow(clippy::inline_always)]
#[inline(always)]
const fn blend_u8(src: u8, dst: u8, alpha: u16) -> u8 {
    let inv = 255 - alpha;
    let val = src as u16 * alpha + dst as u16 * inv + 128;
    ((val + (val >> 8)) >> 8) as u8
}

// ── Public API re-exports ───────────────────────────────────────────────────

pub use blit::{scale_blit_rgba, scale_blit_rgba_rotated};
pub use convert::{i420_to_rgba8_buf, nv12_to_rgba8_buf, rgba8_to_i420_buf, rgba8_to_nv12_buf};
