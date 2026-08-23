// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#![allow(clippy::expect_used)] // Panicking on errors is fine in a benchmark binary.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]

//! Pixel-format conversion microbenchmark using [criterion].
//!
//! Exercises the following conversions across multiple resolutions:
//!
//! - RGBA8 → NV12
//! - RGBA8 → I420
//! - NV12 → RGBA8
//! - I420 → RGBA8
//! - NV12 → I420
//! - I420 → NV12
//!
//! ## Usage
//!
//! ```bash
//! cargo bench -p streamkit-engine --bench pixel_convert
//! ```

mod bench_utils;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use streamkit_nodes::video::pixel_ops::{
    i420_to_nv12_buf, i420_to_rgba8_buf, nv12_to_i420_buf, nv12_to_rgba8_buf, rgba8_to_i420_buf,
    rgba8_to_nv12_buf,
};

use bench_utils::{generate_i420_frame, generate_nv12_frame, generate_rgba_frame, RESOLUTIONS};

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
        ConversionScenario {
            label: "nv12-to-i420",
            input: generate_nv12_frame(width, height),
            output_size: i420_size,
            convert_fn: nv12_to_i420_buf,
        },
        ConversionScenario {
            label: "i420-to-nv12",
            input: generate_i420_frame(width, height),
            output_size: nv12_size,
            convert_fn: i420_to_nv12_buf,
        },
    ]
}

// ── Criterion benchmarks ────────────────────────────────────────────────────

fn bench_pixel_convert(c: &mut Criterion) {
    for &(w, h) in RESOLUTIONS {
        let mut group = c.benchmark_group(format!("pixel_convert/{w}x{h}"));
        group.throughput(Throughput::Elements(1));

        let scenarios = build_scenarios(w, h);

        for scenario in &scenarios {
            // Warm-cache: same input buffer every iteration.
            group.bench_function(scenario.label, |b| {
                let mut output = vec![0u8; scenario.output_size];
                // Warm up: prime rayon thread pool.
                (scenario.convert_fn)(&scenario.input, w, h, &mut output);

                b.iter(|| {
                    (scenario.convert_fn)(&scenario.input, w, h, &mut output);
                });
            });
        }

        group.finish();
    }
}

criterion_group!(benches, bench_pixel_convert);
criterion_main!(benches);
