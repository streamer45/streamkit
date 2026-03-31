// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::disallowed_macros)]
#![allow(clippy::expect_used)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(unsafe_code)]
//! Standalone SVT-AV1 microbenchmark — isolates encoder performance from the
//! StreamKit pipeline, compositor, and WebM muxer.
//!
//! Generates synthetic I420 frames and feeds them directly to the SVT-AV1
//! encoder via FFI, measuring per-frame encode time, flush time, and overall
//! throughput.
//!
//! ## Usage
//!
//! ```bash
//! cargo bench -p streamkit-nodes --features svt_av1 --bench svt_av1_standalone
//! cargo bench -p streamkit-nodes --features svt_av1 --bench svt_av1_standalone -- --width 640 --height 480 --preset 12
//! cargo bench -p streamkit-nodes --features svt_av1 --bench svt_av1_standalone -- --flat  # flat gray input
//! ```

use std::ffi::CString;
use std::time::Instant;

use streamkit_nodes::video::svt_av1_ffi::{
    self, EbBufferHeaderType, EbComponentType, EbSvtAv1EncConfiguration, EbSvtIOFormat,
    EB_BUFFERFLAG_EOS, EB_ERROR_NONE, EB_NO_ERROR_EMPTY_QUEUE,
};

const DEFAULT_WIDTH: usize = 640;
const DEFAULT_HEIGHT: usize = 480;
const DEFAULT_FPS: usize = 30;
const DEFAULT_FRAME_COUNT: usize = 90;
const DEFAULT_PRESET: u32 = 12;
const DEFAULT_CRF: u32 = 35;
const DEFAULT_THREADS: u32 = 0;

struct Args {
    width: usize,
    height: usize,
    fps: usize,
    frame_count: usize,
    preset: u32,
    crf: u32,
    threads: u32,
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
            preset: DEFAULT_PRESET,
            crf: DEFAULT_CRF,
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
                "--preset" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.preset = v.parse().unwrap_or(cfg.preset);
                    }
                },
                "--crf" => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        cfg.crf = v.parse().unwrap_or(cfg.crf);
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

/// Generate a synthetic I420 frame into the provided plane buffers.
///
/// When `flat` is true, produces uniform gray (Y=128, U=128, V=128).
/// Otherwise produces a horizontal gradient with per-frame offset.
fn generate_i420_frame(
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    width: usize,
    height: usize,
    frame_idx: usize,
    flat: bool,
) {
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);

    for row in 0..height {
        for col in 0..width {
            y_plane[row * width + col] =
                if flat { 128 } else { ((col + frame_idx * 3) % 256) as u8 };
        }
    }

    for row in 0..chroma_h {
        for col in 0..chroma_w {
            u_plane[row * chroma_w + col] = 128;
            v_plane[row * chroma_w + col] = 128;
        }
    }
}

/// Helper to set a config parameter via the string API.
fn set_param(config: &mut EbSvtAv1EncConfiguration, name: &str, value: &str) {
    let c_name = CString::new(name).expect("invalid param name");
    let c_value = CString::new(value).expect("invalid param value");
    let ret = unsafe {
        svt_av1_ffi::svt_av1_enc_parse_parameter(config, c_name.as_ptr(), c_value.as_ptr())
    };
    assert!(ret == EB_ERROR_NONE, "svt_av1_enc_parse_parameter({name}={value}) failed: {ret:#X}");
}

struct IterResult {
    total_elapsed: std::time::Duration,
    encode_elapsed: std::time::Duration,
    flush_elapsed: std::time::Duration,
    total_bytes: usize,
    total_packets: usize,
}

/// Wrapper to send `*mut EbComponentType` across thread boundaries.
///
/// # Safety
///
/// SVT-AV1 explicitly supports concurrent `send_picture` / `get_packet`
/// calls from different threads — the library provides internal
/// synchronisation via FIFOs and semaphores.
struct SendableHandle(*mut EbComponentType);
unsafe impl Send for SendableHandle {}

/// Drain all remaining encoded packets from the encoder on a dedicated
/// thread.  Returns `(total_bytes, packet_count)`.
///
/// Uses `pic_send_done = 0` (non-blocking) and polls until the EOS
/// sentinel packet is received.  This must run on a **separate thread**
/// from the one calling `send_picture` — SVT-AV1's internal pipeline
/// can stall if `send_picture` and `get_packet` share a thread, because
/// `send_picture` blocks when the input FIFO is full while `get_packet`
/// is needed to drain output buffers and free pipeline slots.
fn receive_thread(handle: SendableHandle) -> (usize, usize) {
    let handle = handle.0;
    let mut total_bytes: usize = 0;
    let mut packet_count: usize = 0;

    loop {
        let mut out_buf: *mut EbBufferHeaderType = std::ptr::null_mut();
        let ret = unsafe { svt_av1_ffi::svt_av1_enc_get_packet(handle, &raw mut out_buf, 0) };

        if ret == EB_NO_ERROR_EMPTY_QUEUE {
            // Encoder worker threads are still processing — yield briefly.
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        assert!(ret == EB_ERROR_NONE, "svt_av1_enc_get_packet failed: {ret:#X}");

        let (bytes, is_eos) = unsafe {
            let buf = &*out_buf;
            let b = if buf.p_buffer.is_null() { 0 } else { buf.n_filled_len as usize };
            let eos = (buf.flags & EB_BUFFERFLAG_EOS) != 0;
            (b, eos)
        };
        total_bytes += bytes;
        packet_count += 1;

        unsafe { svt_av1_ffi::svt_av1_enc_release_out_buffer(&raw mut out_buf) };

        if is_eos {
            break;
        }
    }

    (total_bytes, packet_count)
}

fn run_once(args: &Args) -> IterResult {
    let mut enc_config = EbSvtAv1EncConfiguration::zeroed();
    let mut handle: *mut EbComponentType = std::ptr::null_mut();

    // Init handle (SVT-AV1 4.x API: no p_app_data parameter).
    let ret = unsafe { svt_av1_ffi::svt_av1_enc_init_handle(&raw mut handle, &raw mut enc_config) };
    assert!(ret == EB_ERROR_NONE, "svt_av1_enc_init_handle failed: {ret:#X}");

    // Configure.
    set_param(&mut enc_config, "preset", &args.preset.to_string());
    set_param(&mut enc_config, "width", &args.width.to_string());
    set_param(&mut enc_config, "height", &args.height.to_string());
    set_param(&mut enc_config, "fps-num", &args.fps.to_string());
    set_param(&mut enc_config, "fps-denom", "1");
    set_param(&mut enc_config, "input-depth", "8");
    set_param(&mut enc_config, "color-format", "1"); // YUV420
    set_param(&mut enc_config, "rc", "0");
    set_param(&mut enc_config, "crf", &args.crf.to_string());
    set_param(&mut enc_config, "aq-mode", "2");
    set_param(&mut enc_config, "lp", &args.threads.to_string());
    set_param(&mut enc_config, "pred-struct", if args.low_latency { "1" } else { "2" });
    set_param(&mut enc_config, "keyint", "149"); // ~5s at 30fps

    let ret = unsafe { svt_av1_ffi::svt_av1_enc_set_parameter(handle, &raw mut enc_config) };
    assert!(ret == EB_ERROR_NONE, "svt_av1_enc_set_parameter failed: {ret:#X}");

    let ret = unsafe { svt_av1_ffi::svt_av1_enc_init(handle) };
    assert!(ret == EB_ERROR_NONE, "svt_av1_enc_init failed: {ret:#X}");

    // Allocate plane buffers.
    let y_size = args.width * args.height;
    let chroma_w = args.width.div_ceil(2);
    let chroma_h = args.height.div_ceil(2);
    let uv_size = chroma_w * chroma_h;

    let mut y_plane = vec![0u8; y_size];
    let mut u_plane = vec![0u8; uv_size];
    let mut v_plane = vec![0u8; uv_size];

    let total_start = Instant::now();

    // Spawn a dedicated receive thread — SVT-AV1 requires send_picture and
    // get_packet to run on separate threads.  The internal pipeline stalls
    // if both happen on the same thread because send_picture blocks when the
    // input FIFO is full, but slots only free up when get_packet drains
    // output buffers.
    let recv_handle = SendableHandle(handle);
    let recv_thread = std::thread::spawn(move || receive_thread(recv_handle));

    // Phase 1: encode frames (send to encoder).
    let encode_start = Instant::now();

    for i in 0..args.frame_count {
        generate_i420_frame(
            &mut y_plane,
            &mut u_plane,
            &mut v_plane,
            args.width,
            args.height,
            i,
            args.flat,
        );

        let mut io_format = EbSvtIOFormat {
            luma: y_plane.as_mut_ptr(),
            cb: u_plane.as_mut_ptr(),
            cr: v_plane.as_mut_ptr(),
            y_stride: args.width as u32,
            cb_stride: chroma_w as u32,
            cr_stride: chroma_w as u32,
        };

        let frame_size = (y_size + uv_size * 2) as u32;

        let mut buf_header = EbBufferHeaderType {
            size: std::mem::size_of::<EbBufferHeaderType>() as u32,
            p_buffer: std::ptr::from_mut(&mut io_format).cast::<u8>(),
            n_filled_len: frame_size,
            n_alloc_len: frame_size,
            p_app_private: std::ptr::null_mut(),
            wrapper_ptr: std::ptr::null_mut(),
            n_tick_count: 0,
            dts: 0,
            pts: i as i64,
            temporal_layer_index: 0,
            qp: 0,
            avg_qp: 0,
            pic_type: 0,
            luma_sse: 0,
            cr_sse: 0,
            cb_sse: 0,
            flags: 0,
            luma_ssim: 0.0,
            cr_ssim: 0.0,
            cb_ssim: 0.0,
            metadata: std::ptr::null_mut(),
        };

        let ret = unsafe { svt_av1_ffi::svt_av1_enc_send_picture(handle, &raw mut buf_header) };
        assert!(ret == EB_ERROR_NONE, "svt_av1_enc_send_picture failed: {ret:#X}");
    }
    let encode_elapsed = encode_start.elapsed();

    // Phase 2: flush (send EOS).
    let flush_start = Instant::now();
    let mut eos_header = EbBufferHeaderType {
        size: std::mem::size_of::<EbBufferHeaderType>() as u32,
        p_buffer: std::ptr::null_mut(),
        n_filled_len: 0,
        n_alloc_len: 0,
        p_app_private: std::ptr::null_mut(),
        wrapper_ptr: std::ptr::null_mut(),
        n_tick_count: 0,
        dts: 0,
        pts: 0,
        temporal_layer_index: 0,
        qp: 0,
        avg_qp: 0,
        pic_type: 0,
        luma_sse: 0,
        cr_sse: 0,
        cb_sse: 0,
        flags: EB_BUFFERFLAG_EOS,
        luma_ssim: 0.0,
        cr_ssim: 0.0,
        cb_ssim: 0.0,
        metadata: std::ptr::null_mut(),
    };

    let ret = unsafe { svt_av1_ffi::svt_av1_enc_send_picture(handle, &raw mut eos_header) };
    assert!(ret == EB_ERROR_NONE, "svt_av1_enc_send_picture (EOS) failed: {ret:#X}");

    // Wait for the receive thread to drain all packets (including EOS).
    let (total_bytes, total_packets) = recv_thread.join().expect("receive thread panicked");
    let flush_elapsed = flush_start.elapsed();
    let total_elapsed = total_start.elapsed();

    // Cleanup.
    unsafe {
        svt_av1_ffi::svt_av1_enc_deinit(handle);
        svt_av1_ffi::svt_av1_enc_deinit_handle(handle);
    }

    IterResult { total_elapsed, encode_elapsed, flush_elapsed, total_bytes, total_packets }
}

fn main() {
    let args = Args::parse();

    eprintln!();
    eprintln!("  Standalone SVT-AV1 Microbenchmark");
    eprintln!("  ─────────────────────────────────");
    eprintln!("  Resolution  : {}x{}", args.width, args.height);
    eprintln!("  Target FPS  : {}", args.fps);
    eprintln!("  Frames      : {}", args.frame_count);
    eprintln!("  Iterations  : {}", args.iterations);
    eprintln!("  Preset      : {}", args.preset);
    eprintln!("  CRF         : {}", args.crf);
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
             pkts={}  output={} bytes",
            args.iterations,
            r.total_elapsed.as_secs_f64(),
            fps,
            r.encode_elapsed.as_secs_f64(),
            encode_fps,
            r.flush_elapsed.as_secs_f64(),
            r.total_packets,
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
    let avg_pkts = all_results.iter().map(|r| r.total_packets).sum::<usize>() as f64 / n;
    let avg_bytes = all_results.iter().map(|r| r.total_bytes).sum::<usize>() as f64 / n;

    eprintln!("── Summary ({} iterations) ──────────────────────────────", args.iterations);
    eprintln!(
        "  total        : {mean_total:.3}s  ({mean_fps:.1} fps,  {mean_frame_ms:.2} ms/frame)"
    );
    eprintln!("  encode phase : {mean_encode:.3}s  ({mean_encode_fps:.1} fps)");
    eprintln!("  flush phase  : {mean_flush:.3}s");
    eprintln!("  flush/total  : {:.1}%", mean_flush / mean_total * 100.0);
    eprintln!("  packets      : {avg_pkts:.0}");
    eprintln!(
        "  output size  : {avg_bytes:.0} bytes ({:.0} bytes/frame)",
        avg_bytes / args.frame_count as f64
    );
    eprintln!();

    // JSON output
    let json = serde_json::json!({
        "benchmark": "svt_av1_standalone",
        "width": args.width,
        "height": args.height,
        "fps": args.fps,
        "frame_count": args.frame_count,
        "preset": args.preset,
        "crf": args.crf,
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
        "avg_packets": avg_pkts,
        "avg_bytes": avg_bytes,
    });
    println!("{json}");
}
