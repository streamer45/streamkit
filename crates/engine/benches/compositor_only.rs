// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::disallowed_macros)] // Bench binary intentionally uses eprintln!/println! for output.
#![allow(clippy::expect_used)] // Panicking on errors is fine in a benchmark binary.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]

//! Compositor-only microbenchmark — measures `composite_frame` in isolation
//! (no VP9 encode, no mux, no async runtime overhead).
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
//! Quick run (default 200 frames @ 1280×720):
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench compositor_only
//! ```
//!
//! Custom parameters:
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench compositor_only -- --frames 500 --width 1920 --height 1080
//! ```

use std::sync::Arc;
use std::time::Instant;

use streamkit_core::frame_pool::PooledVideoData;
use streamkit_core::types::PixelFormat;
use streamkit_core::VideoFramePool;

// Re-use the compositor kernel and pixel_ops directly.
use streamkit_nodes::video::compositor::config::Rect;
use streamkit_nodes::video::compositor::kernel::{composite_frame, ConversionCache, LayerSnapshot};
use streamkit_nodes::video::compositor::overlay::DecodedOverlay;
use streamkit_nodes::video::pixel_ops::{rgba8_to_i420_buf, rgba8_to_nv12_buf};

// ── Default benchmark parameters ────────────────────────────────────────────

const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;
const DEFAULT_FRAME_COUNT: u32 = 200;

// ── Arg parser ──────────────────────────────────────────────────────────────

struct BenchArgs {
    width: u32,
    height: u32,
    frame_count: u32,
    iterations: u32,
    /// Optional filter: only run scenarios whose label contains this substring.
    filter: Option<String>,
}

impl BenchArgs {
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut cfg = Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            frame_count: DEFAULT_FRAME_COUNT,
            iterations: 3,
            filter: None,
        };
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--width" | "-w" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.width = v.parse().unwrap_or(cfg.width);
                    }
                },
                "--height" | "-h" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.height = v.parse().unwrap_or(cfg.height);
                    }
                },
                "--frames" | "-n" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.frame_count = v.parse().unwrap_or(cfg.frame_count);
                    }
                },
                "--iterations" | "-i" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.iterations = v.parse().unwrap_or(cfg.iterations);
                    }
                },
                "--filter" | "-f" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.filter = Some(v.clone());
                    }
                },
                _ => {},
            }
            i += 1;
        }
        cfg
    }
}

// ── Frame generators ────────────────────────────────────────────────────────

/// Generate an RGBA8 color-bar frame (opaque, all alpha = 255).
#[allow(clippy::many_single_char_names)]
fn generate_rgba_frame(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 4];
    // Simple vertical gradient bars for visual distinctness.
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
fn generate_i420_frame(width: u32, height: u32) -> Vec<u8> {
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
fn generate_nv12_frame(width: u32, height: u32) -> Vec<u8> {
    let rgba = generate_rgba_frame(width, height);
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let nv12_size = w * h + chroma_w * 2 * chroma_h;
    let mut nv12 = vec![0u8; nv12_size];
    streamkit_nodes::video::compositor::pixel_ops::rgba8_to_nv12_buf(
        &rgba, width, height, &mut nv12,
    );
    nv12
}

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

// ── Compositing harness ─────────────────────────────────────────────────────

/// Call the real `composite_frame` kernel for `frame_count` iterations,
/// returning per-frame timing statistics.  This exercises all kernel
/// optimizations: conversion cache, skip-canvas-clear, identity-scale
/// fast-path, precomputed x-map, SSE2 blend, etc.
///
/// Uses a real `VideoFramePool` to match production behaviour (pooled buffer
/// reuse instead of per-frame heap allocation).
fn bench_composite(
    _label: &str,
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    image_overlays: &[Arc<DecodedOverlay>],
    text_overlays: &[Arc<DecodedOverlay>],
    frame_count: u32,
) -> BenchResult {
    let mut conversion_cache = ConversionCache::new();
    let pool = VideoFramePool::video_default();

    let start = Instant::now();

    for _ in 0..frame_count {
        let _result = composite_frame(
            canvas_w,
            canvas_h,
            layers,
            image_overlays,
            text_overlays,
            Some(&pool),
            &mut conversion_cache,
        );
    }

    let elapsed = start.elapsed();
    BenchResult { total_secs: elapsed.as_secs_f64(), frame_count }
}

/// Benchmark RGBA8 → NV12 output conversion in isolation.
///
/// Mirrors the production VP9 encoder path (`vp9.rs:1131`) where `composite_frame`
/// output feeds directly into `rgba8_to_nv12_buf`.  Pre-composites a single frame,
/// then times repeated NV12 conversions from the same RGBA buffer.
fn bench_rgba_to_nv12(canvas_w: u32, canvas_h: u32, frame_count: u32) -> BenchResult {
    let w = canvas_w as usize;
    let h = canvas_h as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);

    // Pre-generate a realistic RGBA canvas (colorbar pattern, all opaque).
    let rgba = generate_rgba_frame(canvas_w, canvas_h);
    let nv12_size = w * h + chroma_w * 2 * chroma_h;
    let mut nv12 = vec![0u8; nv12_size];

    let start = Instant::now();

    for _ in 0..frame_count {
        rgba8_to_nv12_buf(&rgba, canvas_w, canvas_h, &mut nv12);
    }

    let elapsed = start.elapsed();
    BenchResult { total_secs: elapsed.as_secs_f64(), frame_count }
}

struct BenchResult {
    total_secs: f64,
    frame_count: u32,
}

impl BenchResult {
    fn fps(&self) -> f64 {
        f64::from(self.frame_count) / self.total_secs
    }

    fn ms_per_frame(&self) -> f64 {
        self.total_secs * 1000.0 / f64::from(self.frame_count)
    }
}

// ── Scenario definitions ────────────────────────────────────────────────────

struct Scenario {
    label: String,
    layers: Vec<Option<LayerSnapshot>>,
    image_overlays: Vec<Arc<DecodedOverlay>>,
    text_overlays: Vec<Arc<DecodedOverlay>>,
}

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
    });

    // Image overlay: a corner logo watermark.
    let logo_w = canvas_w / 6;
    let logo_h = canvas_h / 8;
    let image_overlay = Arc::new(DecodedOverlay {
        rgba_data: generate_image_overlay(logo_w, logo_h),
        width: logo_w,
        height: logo_h,
        rect: Rect { x: 20, y: 20, width: logo_w, height: logo_h },
        opacity: 0.8,
        rotation_degrees: 0.0,
        z_index: 11,
        mirror_horizontal: false,
        mirror_vertical: false,
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
        // 2 layers RGBA, static (same Arc — for future cache-hit measurement)
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

// ── Main ────────────────────────────────────────────────────────────────────

fn main() {
    let args = BenchArgs::parse();

    let resolutions: &[(u32, u32)] = if args.width == DEFAULT_WIDTH && args.height == DEFAULT_HEIGHT
    {
        // Default: run at multiple resolutions.
        &[(640, 480), (1280, 720), (1920, 1080)]
    } else {
        // Custom: run at the specified resolution only.
        // (Leak to get 'static — acceptable in a short-lived bench binary.)
        let res = Box::leak(Box::new([(args.width, args.height)]));
        res
    };

    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║         Compositor-Only Microbenchmark                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  Resolutions : {:<41}║",
        resolutions.iter().map(|(w, h)| format!("{w}×{h}")).collect::<Vec<_>>().join(", ")
    );
    eprintln!("║  Frames      : {:<41}║", args.frame_count);
    eprintln!("║  Iterations  : {:<41}║", args.iterations);
    if let Some(ref f) = args.filter {
        eprintln!("║  Filter      : {f:<41}║");
    }
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!();

    let mut json_results: Vec<serde_json::Value> = Vec::new();

    for &(w, h) in resolutions {
        eprintln!("── {w}×{h} ──────────────────────────────────────────────");

        let scenarios = build_scenarios(w, h);

        for scenario in &scenarios {
            if let Some(ref filter) = args.filter {
                if !scenario.label.contains(filter.as_str()) {
                    continue;
                }
            }

            let mut iter_results = Vec::with_capacity(args.iterations as usize);

            for iter in 1..=args.iterations {
                let result = bench_composite(
                    &scenario.label,
                    w,
                    h,
                    &scenario.layers,
                    &scenario.image_overlays,
                    &scenario.text_overlays,
                    args.frame_count,
                );
                eprintln!(
                    "  {:<28} iter {iter}/{}: {:>8.1} fps  ({:.2} ms/frame)",
                    scenario.label,
                    args.iterations,
                    result.fps(),
                    result.ms_per_frame(),
                );
                iter_results.push(result);
            }

            // Summary for this scenario.
            let fps_values: Vec<f64> = iter_results.iter().map(BenchResult::fps).collect();
            let ms_values: Vec<f64> = iter_results.iter().map(BenchResult::ms_per_frame).collect();
            let mean_fps = fps_values.iter().sum::<f64>() / fps_values.len() as f64;
            let mean_ms = ms_values.iter().sum::<f64>() / ms_values.len() as f64;
            let min_ms = ms_values.iter().copied().fold(f64::INFINITY, f64::min);
            let max_ms = ms_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);

            eprintln!(
                "  {:<28} avg: {:>8.1} fps  ({:.2} ms/frame, min={:.2}, max={:.2})",
                "", mean_fps, mean_ms, min_ms, max_ms,
            );

            json_results.push(serde_json::json!({
                "benchmark": "compositor_only",
                "scenario": scenario.label,
                "width": w,
                "height": h,
                "frame_count": args.frame_count,
                "iterations": args.iterations,
                "mean_fps": mean_fps,
                "mean_ms_per_frame": mean_ms,
                "min_ms_per_frame": min_ms,
                "max_ms_per_frame": max_ms,
            }));
        }

        // ── Standalone conversion benchmarks ──────────────────────────
        let conversion_label = "rgba-to-nv12-output";
        if args.filter.as_ref().is_none_or(|f| conversion_label.contains(f.as_str())) {
            let mut iter_results = Vec::with_capacity(args.iterations as usize);
            for iter in 1..=args.iterations {
                let result = bench_rgba_to_nv12(w, h, args.frame_count);
                eprintln!(
                    "  {:<28} iter {iter}/{}: {:>8.1} fps  ({:.2} ms/frame)",
                    conversion_label,
                    args.iterations,
                    result.fps(),
                    result.ms_per_frame(),
                );
                iter_results.push(result);
            }
            let fps_values: Vec<f64> = iter_results.iter().map(BenchResult::fps).collect();
            let ms_values: Vec<f64> = iter_results.iter().map(BenchResult::ms_per_frame).collect();
            let mean_fps = fps_values.iter().sum::<f64>() / fps_values.len() as f64;
            let mean_ms = ms_values.iter().sum::<f64>() / ms_values.len() as f64;
            let min_ms = ms_values.iter().copied().fold(f64::INFINITY, f64::min);
            let max_ms = ms_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            eprintln!(
                "  {:<28} avg: {:>8.1} fps  ({:.2} ms/frame, min={:.2}, max={:.2})",
                "", mean_fps, mean_ms, min_ms, max_ms,
            );
            json_results.push(serde_json::json!({
                "benchmark": "compositor_only",
                "scenario": conversion_label,
                "width": w,
                "height": h,
                "frame_count": args.frame_count,
                "iterations": args.iterations,
                "mean_fps": mean_fps,
                "mean_ms_per_frame": mean_ms,
                "min_ms_per_frame": min_ms,
                "max_ms_per_frame": max_ms,
            }));
        }

        eprintln!();
    }

    // Machine-readable JSON output.
    println!("{}", serde_json::to_string_pretty(&json_results).expect("JSON serialization"));
}
