// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::disallowed_macros)] // Bench binary intentionally uses eprintln!/println! for output.
#![allow(clippy::expect_used)] // Panicking on errors is fine in a benchmark binary.
//! Benchmark for the compositing oneshot pipeline.
//!
//! Runs the same graph as `samples/pipelines/oneshot/video_compositor_demo.yml`:
//!
//!   colorbars_bg (I420) ──┐
//!                         ├─► compositor ──► vp9_encoder ──► http_output
//!   colorbars_pip (RGBA8) ┘
//!
//! The benchmark drives the pipeline through [`Engine::run_oneshot_pipeline`]
//! and reports wall-clock time, throughput (frames/s), and total output bytes.
//!
//! ## Usage
//!
//! Quick run (default 90 frames @ 640×480):
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench compositor_pipeline
//! ```
//!
//! Custom frame count / resolution for profiling:
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench compositor_pipeline -- --frames 300 --width 1280 --height 720
//! ```
//!
//! Attach a profiler (e.g. `perf`, `samply`, `cargo flamegraph`):
//!
//! ```bash
//! cargo build --release -p streamkit-engine --bench compositor_pipeline
//! samply record target/release/deps/compositor_pipeline-* -- --frames 300
//! ```

use std::time::Instant;
use streamkit_engine::Engine;

/// Default benchmark parameters (matches the sample pipeline).
const DEFAULT_WIDTH: u32 = 640;
const DEFAULT_HEIGHT: u32 = 480;
const DEFAULT_FPS: u32 = 30;
const DEFAULT_FRAME_COUNT: u32 = 90;

/// Simple arg parser — not worth pulling in clap for a bench binary.
struct BenchArgs {
    width: u32,
    height: u32,
    fps: u32,
    frame_count: u32,
    iterations: u32,
}

impl BenchArgs {
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut cfg = Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            fps: DEFAULT_FPS,
            frame_count: DEFAULT_FRAME_COUNT,
            iterations: 3,
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
                "--fps" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.fps = v.parse().unwrap_or(cfg.fps);
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
                _ => {}, // ignore unknown (cargo bench passes extra flags)
            }
            i += 1;
        }
        cfg
    }
}

/// Build the compositor demo pipeline definition programmatically.
///
/// Mirrors `samples/pipelines/oneshot/video_compositor_demo.yml` but with
/// configurable resolution and frame count.
fn build_pipeline(width: u32, height: u32, fps: u32, frame_count: u32) -> streamkit_api::Pipeline {
    use streamkit_api::{Connection, EngineMode, Node, Pipeline};

    let mut nodes = indexmap::IndexMap::new();

    // --- colorbars_bg (I420, full-size) ---
    nodes.insert(
        "colorbars_bg".to_string(),
        Node {
            kind: "video::colorbars".to_string(),
            params: Some(serde_json::json!({
                "width": width,
                "height": height,
                "fps": fps,
                "frame_count": frame_count,
            })),
            state: None,
        },
    );

    // --- colorbars_pip (RGBA8, half-size PiP) ---
    nodes.insert(
        "colorbars_pip".to_string(),
        Node {
            kind: "video::colorbars".to_string(),
            params: Some(serde_json::json!({
                "width": width / 2,
                "height": height / 2,
                "fps": fps,
                "frame_count": frame_count,
                "pixel_format": "rgba8",
            })),
            state: None,
        },
    );

    // --- compositor ---
    nodes.insert(
        "compositor".to_string(),
        Node {
            kind: "video::compositor".to_string(),
            params: Some(serde_json::json!({
                "width": width,
                "height": height,
                "num_inputs": 2,
            })),
            state: None,
        },
    );

    // --- VP9 encoder ---
    nodes.insert(
        "vp9_encoder".to_string(),
        Node { kind: "video::vp9::encoder".to_string(), params: None, state: None },
    );

    // --- WebM muxer (converts encoded video to binary bytes) ---
    nodes.insert(
        "webm_muxer".to_string(),
        Node {
            kind: "containers::webm::muxer".to_string(),
            params: Some(serde_json::json!({
                "video_width": width,
                "video_height": height,
                "streaming_mode": "live",
            })),
            state: None,
        },
    );

    // --- http_output (bytes sink) ---
    nodes.insert(
        "http_output".to_string(),
        Node { kind: "streamkit::http_output".to_string(), params: None, state: None },
    );

    let connections = vec![
        Connection {
            from_node: "colorbars_bg".to_string(),
            from_pin: "out".to_string(),
            to_node: "compositor".to_string(),
            to_pin: "in_0".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
        Connection {
            from_node: "colorbars_pip".to_string(),
            from_pin: "out".to_string(),
            to_node: "compositor".to_string(),
            to_pin: "in_1".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
        Connection {
            from_node: "compositor".to_string(),
            from_pin: "out".to_string(),
            to_node: "vp9_encoder".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
        Connection {
            from_node: "vp9_encoder".to_string(),
            from_pin: "out".to_string(),
            to_node: "webm_muxer".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
        Connection {
            from_node: "webm_muxer".to_string(),
            from_pin: "out".to_string(),
            to_node: "http_output".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
    ];

    Pipeline {
        name: Some("Compositor Benchmark".to_string()),
        description: Some(format!("Benchmark: {width}×{height} @ {fps} fps, {frame_count} frames")),
        mode: EngineMode::OneShot,
        nodes,
        connections,
    }
}

/// Run one iteration of the benchmark pipeline and return (elapsed, output_bytes).
async fn run_once(
    engine: &Engine,
    width: u32,
    height: u32,
    fps: u32,
    frame_count: u32,
) -> (std::time::Duration, usize) {
    let definition = build_pipeline(width, height, fps, frame_count);

    let start = Instant::now();

    let result = engine
        .run_oneshot_pipeline::<futures::stream::Empty<Result<bytes::Bytes, std::io::Error>>, std::io::Error>(
            definition,
            vec![], // no HTTP inputs — generator mode
            None,   // default config
            None,   // no cancellation
        )
        .await
        .expect("Pipeline should start successfully");

    // Drain all output bytes.
    let mut total_bytes: usize = 0;
    let mut data_stream = result.data_stream;
    while let Some(chunk) = data_stream.recv().await {
        total_bytes += chunk.len();
    }

    let elapsed = start.elapsed();
    (elapsed, total_bytes)
}

fn main() {
    // Initialise a minimal tracing subscriber so nodes don't panic on log calls.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args = BenchArgs::parse();

    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║         Compositor Pipeline Benchmark                   ║");
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  Resolution : {}×{:<36}║", args.width, format!("{}", args.height));
    eprintln!("║  FPS        : {:<42}║", args.fps);
    eprintln!("║  Frames     : {:<42}║", args.frame_count);
    eprintln!("║  Iterations : {:<42}║", args.iterations);
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime");

    let engine = Engine::without_plugins();

    let mut durations = Vec::with_capacity(args.iterations as usize);
    let mut output_bytes_all = Vec::with_capacity(args.iterations as usize);

    for iter in 1..=args.iterations {
        let (elapsed, output_bytes) =
            rt.block_on(run_once(&engine, args.width, args.height, args.fps, args.frame_count));

        let fps_actual = f64::from(args.frame_count) / elapsed.as_secs_f64();

        eprintln!(
            "  iter {iter}/{}: {:.3}s  ({:.1} fps)  output={} bytes",
            args.iterations,
            elapsed.as_secs_f64(),
            fps_actual,
            output_bytes,
        );

        durations.push(elapsed);
        output_bytes_all.push(output_bytes);
    }

    // --- Summary ---
    eprintln!();
    let total_secs: Vec<f64> = durations.iter().map(std::time::Duration::as_secs_f64).collect();
    #[allow(clippy::cast_precision_loss)]
    let mean = total_secs.iter().sum::<f64>() / total_secs.len() as f64;
    let min = total_secs.iter().copied().fold(f64::INFINITY, f64::min);
    let max = total_secs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let stddev = if total_secs.len() > 1 {
        #[allow(clippy::cast_precision_loss)]
        let variance = total_secs.iter().map(|t| (t - mean).powi(2)).sum::<f64>()
            / (total_secs.len() - 1) as f64;
        variance.sqrt()
    } else {
        0.0
    };

    let mean_fps = f64::from(args.frame_count) / mean;
    let mean_frame_ms = mean * 1000.0 / f64::from(args.frame_count);
    let avg_output = output_bytes_all.iter().sum::<usize>() / output_bytes_all.len();

    eprintln!("── Summary ({} iterations) ──────────────────────────────", args.iterations);
    eprintln!("  wall-clock   : {mean:.3}s  (min={min:.3}s  max={max:.3}s  σ={stddev:.4}s)");
    eprintln!("  throughput   : {mean_fps:.1} fps");
    eprintln!("  per-frame    : {mean_frame_ms:.2} ms/frame");
    eprintln!("  output size  : {avg_output} bytes (avg)");
    eprintln!();

    // Also print a machine-readable JSON line for CI / automated collection.
    let json = serde_json::json!({
        "benchmark": "compositor_pipeline",
        "width": args.width,
        "height": args.height,
        "fps": args.fps,
        "frame_count": args.frame_count,
        "iterations": args.iterations,
        "mean_secs": mean,
        "min_secs": min,
        "max_secs": max,
        "stddev_secs": stddev,
        "mean_fps": mean_fps,
        "mean_frame_ms": mean_frame_ms,
        "avg_output_bytes": avg_output,
    });
    println!("{json}");
}
