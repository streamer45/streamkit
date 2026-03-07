// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::disallowed_macros)] // Bench binary intentionally uses eprintln!/println! for output.
#![allow(clippy::expect_used)] // Panicking on errors is fine in a benchmark binary.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]

//! Pixel-format conversion microbenchmark — measures raw conversion throughput
//! for the `video::pixel_convert` node's supported conversion paths in isolation
//! (no async runtime, no channel overhead).
//!
//! Exercises the following conversions across multiple resolutions:
//!
//! - RGBA8 → NV12
//! - RGBA8 → I420
//! - NV12 → RGBA8
//! - I420 → RGBA8
//!
//! ## Usage
//!
//! Quick run (default 200 frames @ 1280×720):
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench pixel_convert
//! ```
//!
//! Custom parameters:
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench pixel_convert -- --frames 300 --width 1920 --height 1080
//! ```

use std::time::Instant;

use streamkit_nodes::video::compositor::pixel_ops::{
    i420_to_rgba8_buf, nv12_to_rgba8_buf, rgba8_to_i420_buf, rgba8_to_nv12_buf,
};

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

/// Generate an NV12 frame by converting an RGBA frame.
fn generate_nv12_frame(width: u32, height: u32) -> Vec<u8> {
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

// ── Benchmark harness ───────────────────────────────────────────────────────

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

/// Benchmark a conversion function by running it `frame_count` times on a
/// pre-allocated input/output buffer pair.
fn bench_conversion(
    input: &[u8],
    output: &mut [u8],
    width: u32,
    height: u32,
    frame_count: u32,
    convert_fn: fn(&[u8], u32, u32, &mut [u8]),
) -> BenchResult {
    // Warm-up: run once to prime caches / JIT / rayon thread pool.
    convert_fn(input, width, height, output);

    let start = Instant::now();
    for _ in 0..frame_count {
        convert_fn(input, width, height, output);
    }
    let elapsed = start.elapsed();

    BenchResult { total_secs: elapsed.as_secs_f64(), frame_count }
}

// ── Conversion scenarios ────────────────────────────────────────────────────

struct ConversionScenario {
    label: &'static str,
    input: Vec<u8>,
    output_size: usize,
    convert_fn: fn(&[u8], u32, u32, &mut [u8]),
}

fn build_scenarios(width: u32, height: u32) -> Vec<ConversionScenario> {
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let rgba_size = w * h * 4;
    let nv12_size = w * h + chroma_w * 2 * chroma_h;
    let i420_size = w * h + 2 * chroma_w * chroma_h;

    vec![
        ConversionScenario {
            label: "rgba8-to-nv12",
            input: generate_rgba_frame(width, height),
            output_size: nv12_size,
            convert_fn: rgba8_to_nv12_buf,
        },
        ConversionScenario {
            label: "rgba8-to-i420",
            input: generate_rgba_frame(width, height),
            output_size: i420_size,
            convert_fn: rgba8_to_i420_buf,
        },
        ConversionScenario {
            label: "nv12-to-rgba8",
            input: generate_nv12_frame(width, height),
            output_size: rgba_size,
            convert_fn: nv12_to_rgba8_buf,
        },
        ConversionScenario {
            label: "i420-to-rgba8",
            input: generate_i420_frame(width, height),
            output_size: rgba_size,
            convert_fn: i420_to_rgba8_buf,
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
        let res = Box::leak(Box::new([(args.width, args.height)]));
        res
    };

    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║         Pixel Convert Microbenchmark                    ║");
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
                let mut output = vec![0u8; scenario.output_size];
                let result = bench_conversion(
                    &scenario.input,
                    &mut output,
                    w,
                    h,
                    args.frame_count,
                    scenario.convert_fn,
                );
                eprintln!(
                    "  {:<28} iter {iter}/{}: {:>8.1} fps  ({:.3} ms/frame)",
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
                "  {:<28} avg: {:>8.1} fps  ({:.3} ms/frame, min={:.3}, max={:.3})",
                "", mean_fps, mean_ms, min_ms, max_ms,
            );

            json_results.push(serde_json::json!({
                "benchmark": "pixel_convert",
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
