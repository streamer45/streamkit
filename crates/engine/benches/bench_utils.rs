// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared utilities for benchmark binaries.

#![allow(dead_code)] // Not every bench uses every helper.

use streamkit_nodes::video::pixel_ops::{rgba8_to_i420_buf, rgba8_to_nv12_buf};

/// Standard resolutions used across benchmark suites.
pub const RESOLUTIONS: &[(u32, u32)] = &[(640, 480), (1280, 720), (1920, 1080)];

/// Generate an RGBA8 color-bar frame (opaque, all alpha = 255).
#[allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn generate_rgba_frame(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 4];
    let bar_colors: &[(u8, u8, u8)] = &[
        (191, 191, 191), // white
        (191, 191, 0),   // yellow
        (0, 191, 191),   // cyan
        (0, 191, 0),     // green
        (191, 0, 191),   // magenta
        (191, 0, 0),     // red
        (0, 0, 191),     // blue
    ];
    for row in 0..h {
        for col in 0..w {
            let bar_idx = col * bar_colors.len() / w;
            let (r, g, b) = bar_colors[bar_idx];
            let off = (row * w + col) * 4;
            data[off] = r;
            data[off + 1] = g;
            data[off + 2] = b;
            data[off + 3] = 255;
        }
    }
    data
}

/// Generate an I420 frame by converting an RGBA frame.
pub fn generate_i420_frame(width: u32, height: u32) -> Vec<u8> {
    let rgba = generate_rgba_frame(width, height);
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let i420_size = w * h + 2 * chroma_w * chroma_h;
    let mut i420 = vec![0u8; i420_size];
    rgba8_to_i420_buf(&rgba, width, height, &mut i420);
    i420
}

/// Generate an NV12 frame by converting an RGBA frame.
pub fn generate_nv12_frame(width: u32, height: u32) -> Vec<u8> {
    let rgba = generate_rgba_frame(width, height);
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let nv12_size = w * h + chroma_w * 2 * chroma_h;
    let mut nv12 = vec![0u8; nv12_size];
    rgba8_to_nv12_buf(&rgba, width, height, &mut nv12);
    nv12
}
