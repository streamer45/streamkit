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

// Re-use the compositor kernel and pixel_ops directly.
use streamkit_nodes::video::compositor::config::Rect;
use streamkit_nodes::video::compositor::pixel_ops::rgba8_to_i420;

/// Inline copy of `LayerSnapshot` to avoid depending on the private `kernel` module.
/// Must stay in sync with `kernel::LayerSnapshot`.
struct LayerSnapshot {
    data: Arc<PooledVideoData>,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    rect: Option<Rect>,
    opacity: f32,
    z_index: i32,
    rotation_degrees: f32,
}

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
    rgba8_to_i420(&rgba, width, height)
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

// ── Compositing harness ─────────────────────────────────────────────────────

/// Directly call the compositing kernel for `frame_count` iterations,
/// returning per-frame timing statistics.
fn bench_composite(
    _label: &str,
    canvas_w: u32,
    canvas_h: u32,
    layers: &[Option<LayerSnapshot>],
    frame_count: u32,
) -> BenchResult {
    // Re-create the kernel's compositing logic inline since `composite_frame`
    // is pub(crate). We call the public pixel_ops functions directly.
    let total_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;
    let mut canvas = vec![0u8; total_bytes];
    let mut i420_scratch: Vec<u8> = Vec::new();

    let start = Instant::now();

    for _ in 0..frame_count {
        // Zero the canvas.
        canvas.fill(0);

        // Blit each layer.
        for layer in layers.iter().flatten() {
            let dst_rect = layer.rect.clone().unwrap_or(Rect {
                x: 0,
                y: 0,
                width: canvas_w,
                height: canvas_h,
            });

            let src_data: &[u8] = match layer.pixel_format {
                PixelFormat::Rgba8 => layer.data.as_slice(),
                PixelFormat::I420 => {
                    let needed = layer.width as usize * layer.height as usize * 4;
                    if i420_scratch.len() < needed {
                        i420_scratch.resize(needed, 0);
                    }
                    streamkit_nodes::video::compositor::pixel_ops::i420_to_rgba8_buf(
                        layer.data.as_slice(),
                        layer.width,
                        layer.height,
                        &mut i420_scratch,
                    );
                    &i420_scratch[..needed]
                },
                PixelFormat::Nv12 => {
                    let needed = layer.width as usize * layer.height as usize * 4;
                    if i420_scratch.len() < needed {
                        i420_scratch.resize(needed, 0);
                    }
                    streamkit_nodes::video::compositor::pixel_ops::nv12_to_rgba8_buf(
                        layer.data.as_slice(),
                        layer.width,
                        layer.height,
                        &mut i420_scratch,
                    );
                    &i420_scratch[..needed]
                },
            };

            streamkit_nodes::video::compositor::pixel_ops::scale_blit_rgba_rotated(
                &mut canvas,
                canvas_w,
                canvas_h,
                src_data,
                layer.width,
                layer.height,
                &dst_rect,
                layer.opacity,
                layer.rotation_degrees,
            );
        }
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
}

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
    })
}

fn build_scenarios(canvas_w: u32, canvas_h: u32) -> Vec<Scenario> {
    let pip_w = canvas_w / 3;
    let pip_h = canvas_h / 3;
    let pip_x = (canvas_w - pip_w - 20) as i32;
    let pip_y = (canvas_h - pip_h - 20) as i32;

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
                    }),
                ]
            },
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
                let result =
                    bench_composite(&scenario.label, w, h, &scenario.layers, args.frame_count);
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
        eprintln!();
    }

    // Machine-readable JSON output.
    println!("{}", serde_json::to_string_pretty(&json_results).expect("JSON serialization"));
}
