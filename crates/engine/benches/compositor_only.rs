// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::expect_used)] // Panicking on errors is fine in a benchmark binary.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]

//! Compositor-only microbenchmark using [criterion].
//!
//! Exercises the following scenarios across multiple resolutions:
//!
//! - 1 layer RGBA (baseline)
//! - 2 layers RGBA (PiP)
//! - 4 layers RGBA
//! - 2 layers mixed I420 + RGBA (measures YUV→RGBA conversion overhead)
//! - 2 layers mixed NV12 + RGBA
//! - 2 layers RGBA with rotation
//! - 2 layers RGBA, static (same data each frame — for future cache-hit measurement)
//! - 1 layer RGBA + text overlay (lower-third banner)
//! - 1 layer RGBA + image overlay (logo watermark)
//! - 2 layers PiP + both overlays (realistic broadcast layout)
//! - I420 bg + PiP + both overlays (realistic codec→compositor pipeline)
//!
//! ## Usage
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench compositor_only
//! ```

mod bench_utils;

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use streamkit_core::frame_pool::PooledVideoData;
use streamkit_core::types::PixelFormat;
use streamkit_core::VideoFramePool;

use streamkit_nodes::video::compositor::config::Rect;
use streamkit_nodes::video::compositor::kernel::{composite_frame, ConversionCache, LayerSnapshot};
use streamkit_nodes::video::compositor::overlay::DecodedOverlay;
use streamkit_nodes::video::pixel_ops::rgba8_to_nv12_buf;

use bench_utils::{generate_i420_frame, generate_nv12_frame, generate_rgba_frame, RESOLUTIONS};

// ── Overlay generators (specific to compositor benchmarks) ──────────────────

/// Generate a semi-transparent RGBA overlay simulating rendered text.
///
/// Produces a strip with alternating opaque "glyph" blocks and transparent
/// gaps, similar to real rasterised text bitmaps.
fn generate_text_overlay(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let off = (row * w + col) * 4;
            // Simulate glyph blocks: 60% of columns are "ink", rest transparent.
            let in_glyph = (col * 5 / w).is_multiple_of(2);
            if in_glyph {
                data[off] = 255; // white text
                data[off + 1] = 255;
                data[off + 2] = 255;
                data[off + 3] = 220; // slightly translucent
            }
            // else: stays rgba(0,0,0,0) — fully transparent gap
        }
    }
    data
}

/// Generate a semi-transparent RGBA overlay simulating an image/logo.
///
/// Produces a filled rectangle with partial alpha, exercising the alpha-blend
/// code path in the compositor.
fn generate_image_overlay(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let off = (row * w + col) * 4;
            // Gradient alpha from top to bottom for realistic compositing.
            let alpha = (row * 200 / h + 55).min(255) as u8;
            data[off] = 60;
            data[off + 1] = 120;
            data[off + 2] = 200;
            data[off + 3] = alpha;
        }
    }
    data
}

// ── Layer / scenario helpers ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
fn make_layer(
    data: Vec<u8>,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    rect: Option<Rect>,
    opacity: f32,
    z_index: i32,
    rotation_degrees: f32,
) -> Option<LayerSnapshot> {
    Some(LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(data)),
        width,
        height,
        pixel_format,
        rect,
        opacity,
        z_index,
        rotation_degrees,
        mirror_horizontal: false,
        mirror_vertical: false,
    })
}

struct Scenario {
    label: String,
    layers: Vec<Option<LayerSnapshot>>,
    image_overlays: Vec<Arc<DecodedOverlay>>,
    text_overlays: Vec<Arc<DecodedOverlay>>,
}

#[allow(clippy::too_many_lines)]
fn build_scenarios(canvas_w: u32, canvas_h: u32) -> Vec<Scenario> {
    let pip_w = canvas_w / 3;
    let pip_h = canvas_h / 3;
    let pip_x = (canvas_w - pip_w - 20).cast_signed();
    let pip_y = (canvas_h - pip_h - 20).cast_signed();

    // ── Overlay data (reused across scenarios) ──────────────────────
    // Text overlay: a bottom-third banner (typical lower-third title).
    let text_ov_w = canvas_w * 2 / 3;
    let text_ov_h = canvas_h / 8;
    let text_overlay = Arc::new(DecodedOverlay {
        id: "bench-text".to_string(),
        rgba_data: generate_text_overlay(text_ov_w, text_ov_h),
        width: text_ov_w,
        height: text_ov_h,
        rect: Rect {
            x: ((canvas_w - text_ov_w) / 2).cast_signed(),
            y: (canvas_h - text_ov_h - 40).cast_signed(),
            width: text_ov_w,
            height: text_ov_h,
        },
        opacity: 0.95,
        rotation_degrees: 0.0,
        z_index: 10,
        mirror_horizontal: false,
        mirror_vertical: false,
        measured_text_width: None,
        measured_text_height: None,
    });

    // Image overlay: a corner logo watermark.
    let logo_w = canvas_w / 6;
    let logo_h = canvas_h / 8;
    let image_overlay = Arc::new(DecodedOverlay {
        id: "bench-image".to_string(),
        rgba_data: generate_image_overlay(logo_w, logo_h),
        width: logo_w,
        height: logo_h,
        rect: Rect { x: 20, y: 20, width: logo_w, height: logo_h },
        opacity: 0.8,
        rotation_degrees: 0.0,
        z_index: 11,
        mirror_horizontal: false,
        mirror_vertical: false,
        measured_text_width: None,
        measured_text_height: None,
    });

    vec![
        // 1 layer RGBA — baseline
        Scenario {
            label: "1-layer-rgba".to_string(),
            layers: vec![make_layer(
                generate_rgba_frame(canvas_w, canvas_h),
                canvas_w,
                canvas_h,
                PixelFormat::Rgba8,
                None,
                1.0,
                0,
                0.0,
            )],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 2 layers RGBA (PiP)
        Scenario {
            label: "2-layer-rgba-pip".to_string(),
            layers: vec![
                make_layer(
                    generate_rgba_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::Rgba8,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
            ],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 4 layers RGBA
        Scenario {
            label: "4-layer-rgba".to_string(),
            layers: vec![
                make_layer(
                    generate_rgba_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::Rgba8,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: 20, y: 20, width: pip_w, height: pip_h }),
                    0.8,
                    2,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: 20, y: pip_y, width: pip_w, height: pip_h }),
                    0.7,
                    3,
                    0.0,
                ),
            ],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 2 layers: I420 bg + RGBA PiP (measures conversion overhead)
        Scenario {
            label: "2-layer-i420+rgba".to_string(),
            layers: vec![
                make_layer(
                    generate_i420_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::I420,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
            ],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 2 layers: NV12 bg + RGBA PiP
        Scenario {
            label: "2-layer-nv12+rgba".to_string(),
            layers: vec![
                make_layer(
                    generate_nv12_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::Nv12,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
            ],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 2 layers RGBA with rotation on PiP
        Scenario {
            label: "2-layer-rgba-rotated".to_string(),
            layers: vec![
                make_layer(
                    generate_rgba_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::Rgba8,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    15.0, // 15° rotation
                ),
            ],
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // 2 layers RGBA, static (same Arc — for cache-hit measurement)
        Scenario {
            label: "2-layer-rgba-static".to_string(),
            layers: {
                let bg =
                    Arc::new(PooledVideoData::from_vec(generate_rgba_frame(canvas_w, canvas_h)));
                let pip = Arc::new(PooledVideoData::from_vec(generate_rgba_frame(pip_w, pip_h)));
                vec![
                    Some(LayerSnapshot {
                        data: bg,
                        width: canvas_w,
                        height: canvas_h,
                        pixel_format: PixelFormat::Rgba8,
                        rect: None,
                        opacity: 1.0,
                        z_index: 0,
                        rotation_degrees: 0.0,
                        mirror_horizontal: false,
                        mirror_vertical: false,
                    }),
                    Some(LayerSnapshot {
                        data: pip,
                        width: pip_w,
                        height: pip_h,
                        pixel_format: PixelFormat::Rgba8,
                        rect: Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                        opacity: 0.9,
                        z_index: 1,
                        rotation_degrees: 0.0,
                        mirror_horizontal: false,
                        mirror_vertical: false,
                    }),
                ]
            },
            image_overlays: Vec::new(),
            text_overlays: Vec::new(),
        },
        // ── Overlay scenarios ──────────────────────────────────────────
        // 1 layer RGBA + text overlay (lower-third banner)
        Scenario {
            label: "1-layer+text-overlay".to_string(),
            layers: vec![make_layer(
                generate_rgba_frame(canvas_w, canvas_h),
                canvas_w,
                canvas_h,
                PixelFormat::Rgba8,
                None,
                1.0,
                0,
                0.0,
            )],
            image_overlays: Vec::new(),
            text_overlays: vec![Arc::clone(&text_overlay)],
        },
        // 1 layer RGBA + image overlay (logo watermark)
        Scenario {
            label: "1-layer+img-overlay".to_string(),
            layers: vec![make_layer(
                generate_rgba_frame(canvas_w, canvas_h),
                canvas_w,
                canvas_h,
                PixelFormat::Rgba8,
                None,
                1.0,
                0,
                0.0,
            )],
            image_overlays: vec![Arc::clone(&image_overlay)],
            text_overlays: Vec::new(),
        },
        // 2 layers PiP + both overlays (realistic broadcast layout)
        Scenario {
            label: "2-layer-pip+overlays".to_string(),
            layers: vec![
                make_layer(
                    generate_rgba_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::Rgba8,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
            ],
            image_overlays: vec![Arc::clone(&image_overlay)],
            text_overlays: vec![Arc::clone(&text_overlay)],
        },
        // I420 bg + PiP + both overlays (realistic codec→compositor pipeline)
        Scenario {
            label: "i420+pip+overlays".to_string(),
            layers: vec![
                make_layer(
                    generate_i420_frame(canvas_w, canvas_h),
                    canvas_w,
                    canvas_h,
                    PixelFormat::I420,
                    None,
                    1.0,
                    0,
                    0.0,
                ),
                make_layer(
                    generate_rgba_frame(pip_w, pip_h),
                    pip_w,
                    pip_h,
                    PixelFormat::Rgba8,
                    Some(Rect { x: pip_x, y: pip_y, width: pip_w, height: pip_h }),
                    0.9,
                    1,
                    0.0,
                ),
            ],
            image_overlays: vec![Arc::clone(&image_overlay)],
            text_overlays: vec![Arc::clone(&text_overlay)],
        },
    ]
}

// ── Criterion benchmarks ────────────────────────────────────────────────────

fn bench_compositor(c: &mut Criterion) {
    for &(w, h) in RESOLUTIONS {
        let mut group = c.benchmark_group(format!("compositor/{w}x{h}"));
        group.throughput(Throughput::Elements(1));

        let scenarios = build_scenarios(w, h);

        for scenario in &scenarios {
            group.bench_function(&scenario.label, |b| {
                let pool = VideoFramePool::video_default();
                let mut cache = ConversionCache::new();
                // Warm up: prime rayon thread pool and conversion cache.
                let _ = composite_frame(
                    w,
                    h,
                    &scenario.layers,
                    &scenario.image_overlays,
                    &scenario.text_overlays,
                    Some(&pool),
                    &mut cache,
                );

                b.iter(|| {
                    composite_frame(
                        w,
                        h,
                        &scenario.layers,
                        &scenario.image_overlays,
                        &scenario.text_overlays,
                        Some(&pool),
                        &mut cache,
                    )
                });
            });
        }

        // Standalone RGBA→NV12 conversion (mirrors the VP9 encoder output path).
        group.bench_function("rgba-to-nv12-output", |b| {
            let rgba = generate_rgba_frame(w, h);
            let nv12_size =
                (w as usize * h as usize) + (w as usize).div_ceil(2) * 2 * (h as usize).div_ceil(2);
            let mut nv12 = vec![0u8; nv12_size];

            b.iter(|| {
                rgba8_to_nv12_buf(&rgba, w, h, &mut nv12);
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_compositor);
criterion_main!(benches);
