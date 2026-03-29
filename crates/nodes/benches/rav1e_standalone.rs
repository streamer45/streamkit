// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::disallowed_macros)]
#![allow(clippy::expect_used)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
//! Standalone rav1e microbenchmark — isolates encoder performance from the
//! StreamKit pipeline, compositor, and WebM muxer.
//!
//! Generates synthetic I420 frames and feeds them directly to rav1e,
//! measuring per-frame encode time, flush time, and overall throughput.
//!
//! ## Usage
//!
//! ```bash
//! cargo bench -p streamkit-nodes --features av1 --bench rav1e_standalone
//! cargo bench -p streamkit-nodes --features av1 --bench rav1e_standalone -- --width 640 --height 480 --speed 10
//! cargo bench -p streamkit-nodes --features av1 --bench rav1e_standalone -- --flat  # flat gray input for comparison
//! ```

use rav1e::prelude::*;
use std::time::Instant;

const DEFAULT_WIDTH: usize = 640;
const DEFAULT_HEIGHT: usize = 480;
const DEFAULT_FPS: usize = 30;
const DEFAULT_FRAME_COUNT: usize = 90;
const DEFAULT_SPEED: u8 = 10;
const DEFAULT_QUANTIZER: usize = 80;
const DEFAULT_THREADS: usize = 0;

struct Args {
    width: usize,
    height: usize,
    fps: usize,
    frame_count: usize,
    speed: u8,
    quantizer: usize,
    threads: usize,
    iterations: usize,
    low_latency: bool,
    flat: bool,
}

impl Args {
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut cfg = Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            fps: DEFAULT_FPS,
            frame_count: DEFAULT_FRAME_COUNT,
            speed: DEFAULT_SPEED,
            quantizer: DEFAULT_QUANTIZER,
            threads: DEFAULT_THREADS,
            iterations: 3,
            low_latency: true,
            flat: false,
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
                "--speed" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.speed = v.parse().unwrap_or(cfg.speed);
                    }
                },
                "--quantizer" | "-q" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.quantizer = v.parse().unwrap_or(cfg.quantizer);
                    }
                },
                "--threads" | "-t" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.threads = v.parse().unwrap_or(cfg.threads);
                    }
                },
                "--iterations" | "-i" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.iterations = v.parse().unwrap_or(cfg.iterations);
                    }
                },
                "--no-low-latency" => {
                    cfg.low_latency = false;
                },
                "--flat" => {
                    cfg.flat = true;
                },
                _ => {},
            }
            i += 1;
        }
        cfg
    }
}

/// Generate a synthetic I420 frame.
///
/// When `flat` is true, produces a uniform gray frame (Y=128, U=128, V=128)
/// matching ffmpeg's `color=c=0x808080` filter for apples-to-apples comparison.
/// When false, produces a horizontal gradient with per-frame offset.
fn generate_i420_frame(
    ctx: &Context<u8>,
    width: usize,
    height: usize,
    frame_idx: usize,
    flat: bool,
) -> Frame<u8> {
    let mut frame = ctx.new_frame();
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);

    // Y plane
    let y_stride = frame.planes[0].cfg.stride;
    let y_data = frame.planes[0].data_origin_mut();
    for row in 0..height {
        for col in 0..width {
            y_data[row * y_stride + col] =
                if flat { 128 } else { ((col + frame_idx * 3) % 256) as u8 };
        }
    }

    // U plane
    let u_stride = frame.planes[1].cfg.stride;
    let u_data = frame.planes[1].data_origin_mut();
    for row in 0..chroma_h {
        for col in 0..chroma_w {
            u_data[row * u_stride + col] = 128;
        }
    }

    // V plane
    let v_stride = frame.planes[2].cfg.stride;
    let v_data = frame.planes[2].data_origin_mut();
    for row in 0..chroma_h {
        for col in 0..chroma_w {
            v_data[row * v_stride + col] = 128;
        }
    }

    frame
}

struct IterResult {
    total_elapsed: std::time::Duration,
    encode_elapsed: std::time::Duration,
    flush_elapsed: std::time::Duration,
    total_bytes: usize,
    packets_from_encode: usize,
    packets_from_flush: usize,
}

fn run_once(args: &Args) -> IterResult {
    let enc_cfg = EncoderConfig {
        width: args.width,
        height: args.height,
        bit_depth: 8,
        chroma_sampling: ChromaSampling::Cs420,
        chroma_sample_position: ChromaSamplePosition::Unknown,
        time_base: Rational { num: 1, den: args.fps as u64 },
        low_latency: args.low_latency,
        min_key_frame_interval: 0,
        max_key_frame_interval: 150,
        bitrate: 0,
        quantizer: args.quantizer,
        speed_settings: SpeedSettings::from_preset(args.speed),
        ..Default::default()
    };

    let rav1e_cfg = Config::default().with_encoder_config(enc_cfg).with_threads(args.threads);

    let mut ctx: Context<u8> = rav1e_cfg.new_context().expect("Failed to create rav1e context");

    let total_start = Instant::now();

    // Phase 1: encode frames
    let encode_start = Instant::now();
    let mut total_bytes: usize = 0;
    let mut packets_from_encode: usize = 0;

    for i in 0..args.frame_count {
        let frame = generate_i420_frame(&ctx, args.width, args.height, i, args.flat);
        ctx.send_frame(frame).expect("send_frame failed");

        // Drain available packets
        loop {
            match ctx.receive_packet() {
                Ok(pkt) => {
                    total_bytes += pkt.data.len();
                    packets_from_encode += 1;
                },
                Err(EncoderStatus::NeedMoreData | EncoderStatus::Encoded) => break,
                Err(e) => panic!("receive_packet failed: {e}"),
            }
        }
    }
    let encode_elapsed = encode_start.elapsed();

    // Phase 2: flush
    let flush_start = Instant::now();
    ctx.flush();
    let mut packets_from_flush: usize = 0;
    loop {
        match ctx.receive_packet() {
            Ok(pkt) => {
                total_bytes += pkt.data.len();
                packets_from_flush += 1;
            },
            Err(
                EncoderStatus::LimitReached | EncoderStatus::NeedMoreData | EncoderStatus::Encoded,
            ) => break,
            Err(e) => panic!("flush receive_packet failed: {e}"),
        }
    }
    let flush_elapsed = flush_start.elapsed();
    let total_elapsed = total_start.elapsed();

    IterResult {
        total_elapsed,
        encode_elapsed,
        flush_elapsed,
        total_bytes,
        packets_from_encode,
        packets_from_flush,
    }
}

fn main() {
    let args = Args::parse();

    eprintln!();
    eprintln!("  Standalone rav1e Microbenchmark");
    eprintln!("  ──────────────────────────────");
    eprintln!("  Resolution  : {}x{}", args.width, args.height);
    eprintln!("  Target FPS  : {}", args.fps);
    eprintln!("  Frames      : {}", args.frame_count);
    eprintln!("  Iterations  : {}", args.iterations);
    eprintln!("  Speed       : {}", args.speed);
    eprintln!("  Quantizer   : {}", args.quantizer);
    eprintln!("  Threads     : {} (0=auto)", args.threads);
    eprintln!("  Low latency : {}", args.low_latency);
    eprintln!("  Content     : {}", if args.flat { "flat gray" } else { "gradient" });
    eprintln!();

    let mut all_results = Vec::with_capacity(args.iterations);

    for iter in 1..=args.iterations {
        let r = run_once(&args);

        let fps = args.frame_count as f64 / r.total_elapsed.as_secs_f64();
        let encode_fps = args.frame_count as f64 / r.encode_elapsed.as_secs_f64();

        eprintln!(
            "  iter {iter}/{}: total={:.3}s ({:.1} fps)  encode={:.3}s ({:.1} fps)  flush={:.3}s  \
             pkts={}/{}  output={} bytes",
            args.iterations,
            r.total_elapsed.as_secs_f64(),
            fps,
            r.encode_elapsed.as_secs_f64(),
            encode_fps,
            r.flush_elapsed.as_secs_f64(),
            r.packets_from_encode,
            r.packets_from_flush,
            r.total_bytes,
        );

        all_results.push(r);
    }

    // Summary
    eprintln!();
    let n = all_results.len() as f64;
    let mean_total = all_results.iter().map(|r| r.total_elapsed.as_secs_f64()).sum::<f64>() / n;
    let mean_encode = all_results.iter().map(|r| r.encode_elapsed.as_secs_f64()).sum::<f64>() / n;
    let mean_flush = all_results.iter().map(|r| r.flush_elapsed.as_secs_f64()).sum::<f64>() / n;
    let mean_fps = args.frame_count as f64 / mean_total;
    let mean_encode_fps = args.frame_count as f64 / mean_encode;
    let mean_frame_ms = mean_total * 1000.0 / args.frame_count as f64;
    let avg_pkts_encode =
        all_results.iter().map(|r| r.packets_from_encode).sum::<usize>() as f64 / n;
    let avg_pkts_flush = all_results.iter().map(|r| r.packets_from_flush).sum::<usize>() as f64 / n;
    let avg_bytes = all_results.iter().map(|r| r.total_bytes).sum::<usize>() as f64 / n;

    eprintln!("── Summary ({} iterations) ──────────────────────────────", args.iterations);
    eprintln!(
        "  total        : {mean_total:.3}s  ({mean_fps:.1} fps,  {mean_frame_ms:.2} ms/frame)"
    );
    eprintln!("  encode phase : {mean_encode:.3}s  ({mean_encode_fps:.1} fps)");
    eprintln!("  flush phase  : {mean_flush:.3}s");
    eprintln!("  flush/total  : {:.1}%", mean_flush / mean_total * 100.0);
    eprintln!("  pkts encode  : {avg_pkts_encode:.0}");
    eprintln!("  pkts flush   : {avg_pkts_flush:.0}");
    eprintln!(
        "  output size  : {avg_bytes:.0} bytes ({:.0} bytes/frame)",
        avg_bytes / args.frame_count as f64
    );
    eprintln!();

    // JSON output
    let json = serde_json::json!({
        "benchmark": "rav1e_standalone",
        "width": args.width,
        "height": args.height,
        "fps": args.fps,
        "frame_count": args.frame_count,
        "speed": args.speed,
        "quantizer": args.quantizer,
        "threads": args.threads,
        "low_latency": args.low_latency,
        "iterations": args.iterations,
        "mean_total_secs": mean_total,
        "mean_encode_secs": mean_encode,
        "mean_flush_secs": mean_flush,
        "mean_fps": mean_fps,
        "mean_encode_fps": mean_encode_fps,
        "mean_frame_ms": mean_frame_ms,
        "flush_pct": mean_flush / mean_total * 100.0,
        "avg_pkts_encode": avg_pkts_encode,
        "avg_pkts_flush": avg_pkts_flush,
        "avg_bytes": avg_bytes,
    });
    println!("{json}");
}
