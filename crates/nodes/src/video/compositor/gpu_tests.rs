// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! GPU compositor tests.
//!
//! All tests are gated behind `#[cfg(feature = "gpu")]` so they only
//! compile and run when a GPU is available.  Run with:
//!
//! ```bash
//! cargo test -p streamkit-nodes --features gpu -- gpu
//! ```
//!
//! All GPU tests share a single `GpuContext` behind a `Mutex` to avoid
//! creating 26+ simultaneous wgpu devices.  The guard is intentionally
//! held for each test's full duration to serialise GPU access.
#![allow(clippy::significant_drop_tightening)]

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use streamkit_core::frame_pool::PooledVideoData;
use streamkit_core::types::PixelFormat;

use super::config::{CropShape, Rect};
use super::gpu::{self, GpuContext, GpuMode, GpuPathState};
use super::kernel::LayerSnapshot;
use super::overlay::DecodedOverlay;
use super::overlay::OverlaySourceKind;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Create a solid-colour RGBA8 buffer.
fn solid_rgba(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
    let pixel = [r, g, b, a];
    pixel.iter().copied().cycle().take((width as usize) * (height as usize) * 4).collect()
}

/// Create a simple LayerSnapshot from RGBA8 data.
fn make_layer(data: Vec<u8>, width: u32, height: u32, rect: Option<Rect>) -> LayerSnapshot {
    LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(data)),
        width,
        height,
        pixel_format: PixelFormat::Rgba8,
        rect,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: CropShape::Rect,
    }
}

/// Create a LayerSnapshot with specific properties.
#[allow(clippy::too_many_arguments)]
fn make_layer_with_props(
    data: Vec<u8>,
    width: u32,
    height: u32,
    rect: Option<Rect>,
    opacity: f32,
    rotation_degrees: f32,
    z_index: i32,
    mirror_h: bool,
    mirror_v: bool,
    crop_zoom: f32,
    crop_shape: CropShape,
) -> LayerSnapshot {
    LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(data)),
        width,
        height,
        pixel_format: PixelFormat::Rgba8,
        rect,
        opacity,
        z_index,
        rotation_degrees,
        mirror_horizontal: mirror_h,
        mirror_vertical: mirror_v,
        crop_zoom,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape,
    }
}

/// Create a solid-colour I420 buffer.
///
/// Uses ceiling division for chroma dimensions to match
/// `VideoLayout::packed` and the GPU upload path.
#[allow(clippy::many_single_char_names)]
fn solid_i420(width: u32, height: u32, y: u8, u: u8, v: u8) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut buf = vec![0u8; w * h + 2 * cw * ch];
    // Y plane
    buf[..w * h].fill(y);
    // U plane
    buf[w * h..w * h + cw * ch].fill(u);
    // V plane
    buf[w * h + cw * ch..].fill(v);
    buf
}

/// Shared GPU context for all tests.
///
/// Creating a separate `wgpu::Device` per test (26+) overwhelms the Vulkan
/// driver on CI machines, causing `device.poll(wait_indefinitely)` to hang
/// indefinitely.  Sharing one context behind a `Mutex` serialises the GPU
/// work (which hits a single physical GPU anyway) while letting non-GPU
/// tests run in parallel.
static SHARED_GPU: LazyLock<Mutex<GpuContext>> = LazyLock::new(|| {
    Mutex::new(GpuContext::try_init().expect(
        "GPU not available — skipping test. \
         Run on a machine with a GPU to execute GPU compositor tests.",
    ))
});

/// Lock the shared GPU context; panics if no GPU is available.
fn require_gpu() -> MutexGuard<'static, GpuContext> {
    SHARED_GPU.lock().expect("GPU mutex poisoned")
}

/// Average pixel value in the central region of an RGBA8 buffer.
/// Returns (R, G, B, A) averages.
fn avg_centre_pixel(data: &[u8], width: u32, height: u32) -> (f32, f32, f32, f32) {
    let w = width as usize;
    let h = height as usize;
    let cx = w / 4;
    let cy = h / 4;
    let cw = w / 2;
    let ch = h / 2;
    let mut sum = [0f64; 4];
    let mut count = 0u64;
    for y in cy..cy + ch {
        for x in cx..cx + cw {
            let off = (y * w + x) * 4;
            for c in 0..4 {
                sum[c] += f64::from(data[off + c]);
            }
            count += 1;
        }
    }
    let count_f = count as f64;
    (
        (sum[0] / count_f) as f32,
        (sum[1] / count_f) as f32,
        (sum[2] / count_f) as f32,
        (sum[3] / count_f) as f32,
    )
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn gpu_context_init() {
    let _ctx = require_gpu();
}

#[test]
fn gpu_mode_from_config() {
    assert_eq!(GpuMode::from_config(None), GpuMode::Auto);
    assert_eq!(GpuMode::from_config(Some("auto")), GpuMode::Auto);
    assert_eq!(GpuMode::from_config(Some("gpu")), GpuMode::ForceGpu);
    assert_eq!(GpuMode::from_config(Some("GPU")), GpuMode::ForceGpu);
    assert_eq!(GpuMode::from_config(Some("cpu")), GpuMode::ForceCpu);
    assert_eq!(GpuMode::from_config(Some("CPU")), GpuMode::ForceCpu);
    assert_eq!(GpuMode::from_config(Some("anything")), GpuMode::Auto);
}

#[test]
fn gpu_single_opaque_layer_rgba() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Solid red layer covering the full canvas.
    let data = solid_rgba(canvas_w, canvas_h, 255, 0, 0, 255);
    let layer = make_layer(data, canvas_w, canvas_h, None);
    let layers = vec![Some(layer)];

    let (result, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();
    assert_eq!(buf.len(), (canvas_w as usize) * (canvas_h as usize) * 4);

    // Check the centre pixel is red.
    let (r, g, b, a) = avg_centre_pixel(buf, canvas_w, canvas_h);
    assert!(r > 240.0, "Expected red > 240, got {r}");
    assert!(g < 15.0, "Expected green < 15, got {g}");
    assert!(b < 15.0, "Expected blue < 15, got {b}");
    assert!(a > 240.0, "Expected alpha > 240, got {a}");
}

#[test]
fn gpu_two_layer_pip() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Background: solid blue, full canvas.
    let bg = make_layer(solid_rgba(canvas_w, canvas_h, 0, 0, 255, 255), canvas_w, canvas_h, None);

    // Foreground: solid red, small PiP in top-left quadrant.
    let pip_w = canvas_w / 2;
    let pip_h = canvas_h / 2;
    let fg = make_layer_with_props(
        solid_rgba(pip_w, pip_h, 255, 0, 0, 255),
        pip_w,
        pip_h,
        Some(Rect { x: 0, y: 0, width: pip_w, height: pip_h }),
        1.0,
        0.0,
        1, // higher z_index → drawn on top
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(bg), Some(fg)];
    let (result, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();

    // Bottom-right quadrant should be blue (background only).
    let br_x = (canvas_w as usize) * 3 / 4;
    let br_y = (canvas_h as usize) * 3 / 4;
    let off = (br_y * canvas_w as usize + br_x) * 4;
    assert!(buf[off] < 15, "BR should be blue, R={}", buf[off]);
    assert!(buf[off + 2] > 240, "BR should be blue, B={}", buf[off + 2]);
}

#[test]
fn gpu_layer_opacity() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Background: solid blue.
    let bg = make_layer(solid_rgba(canvas_w, canvas_h, 0, 0, 255, 255), canvas_w, canvas_h, None);

    // Foreground: solid red at 50% opacity, full canvas.
    let fg = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 255, 0, 0, 255),
        canvas_w,
        canvas_h,
        None,
        0.5,
        0.0,
        1,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(bg), Some(fg)];
    let (result, _fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    let buf = result.as_slice();
    let (r, _g, b, _a) = avg_centre_pixel(buf, canvas_w, canvas_h);

    // At 50% opacity, red over blue should give roughly (127, 0, 127).
    assert!(r > 100.0 && r < 160.0, "Expected blended red ~127, got {r}");
    assert!(b > 100.0 && b < 160.0, "Expected blended blue ~127, got {b}");
}

#[test]
fn gpu_layer_rotation() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // A small coloured layer, rotated 45°.
    let layer = make_layer_with_props(
        solid_rgba(160, 120, 0, 255, 0, 255),
        160,
        120,
        Some(Rect { x: 80, y: 60, width: 160, height: 120 }),
        1.0,
        45.0,
        0,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(layer)];
    let (result, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();
    assert_eq!(buf.len(), (canvas_w as usize) * (canvas_h as usize) * 4);

    // The centre of the canvas should have the green layer visible
    // (rotated but still covering the centre).
    let mid = ((canvas_h as usize / 2) * canvas_w as usize + canvas_w as usize / 2) * 4;
    assert!(buf[mid + 1] > 200, "Centre should be green after 45° rotation, G={}", buf[mid + 1]);
}

/// Regression test: GPU rotation direction must match the CPU path.
///
/// Positive `rotation_degrees` means clockwise in screen-space (Y-down),
/// consistent with the CPU compositor.  Before the fix the GPU used
/// the raw angle in NDC (Y-up), which produced counter-clockwise
/// rotation — i.e. the visual result was inverted.
///
/// Uses a left-red / right-green split image rotated 90° CW on a
/// square canvas.  After a 90° clockwise rotation the left (red) half
/// should move to the top and the right (green) half to the bottom.
#[test]
fn gpu_rotation_direction_matches_cpu() {
    let mut ctx = require_gpu();
    let size = 64_u32;

    // Build a left-red / right-green split image.
    let mut data = vec![0u8; (size as usize) * (size as usize) * 4];
    for y in 0..size as usize {
        for x in 0..size as usize {
            let off = (y * size as usize + x) * 4;
            if x < size as usize / 2 {
                data[off] = 255; // R
            } else {
                data[off + 1] = 255; // G
            }
            data[off + 3] = 255; // A
        }
    }

    let layer = make_layer_with_props(
        data,
        size,
        size,
        None,
        1.0,
        90.0, // 90° clockwise
        0,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(layer)];
    let (result, _) = ctx.composite_frame_gpu(size, size, &layers, &[], &[], None, None);
    let buf = result.as_slice();

    // After 90° CW rotation:
    //   - original left (red)  → top
    //   - original right (green) → bottom
    // Sample a point in the top-centre (should be red).
    let top_x = size as usize / 2;
    let top_y = size as usize / 4;
    let top_off = (top_y * size as usize + top_x) * 4;
    assert!(
        buf[top_off] > 200 && buf[top_off + 1] < 55,
        "Top-centre should be red after 90° CW rotation: R={}, G={}",
        buf[top_off],
        buf[top_off + 1],
    );

    // Sample a point in the bottom-centre (should be green).
    let bot_x = size as usize / 2;
    let bot_y = size as usize * 3 / 4;
    let bot_off = (bot_y * size as usize + bot_x) * 4;
    assert!(
        buf[bot_off + 1] > 200 && buf[bot_off] < 55,
        "Bottom-centre should be green after 90° CW rotation: R={}, G={}",
        buf[bot_off],
        buf[bot_off + 1],
    );
}

#[test]
fn gpu_circle_crop() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 320;

    // A solid green layer with circle crop.
    let layer = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 0, 255, 0, 255),
        canvas_w,
        canvas_h,
        None,
        1.0,
        0.0,
        0,
        false,
        false,
        1.0,
        CropShape::Circle,
    );

    let layers = vec![Some(layer)];
    let (result, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    let buf = result.as_slice();

    // Centre should be green (inside the circle).
    let mid = ((canvas_h as usize / 2) * canvas_w as usize + canvas_w as usize / 2) * 4;
    assert!(buf[mid + 1] > 200, "Centre should be green (inside circle), G={}", buf[mid + 1]);

    // Corner should be transparent/black (outside the circle).
    let corner = 4; // pixel (1, 0)
    assert!(
        buf[corner + 3] < 30,
        "Corner should be transparent (outside circle), A={}",
        buf[corner + 3]
    );
}

/// Regression test: circle crop on a non-square layer must produce a true
/// circle (inscribed in the shorter dimension), not an ellipse.
///
/// Before the fix, the GPU shader used `length(quad_uv - centre)` in UV
/// space where both axes span [0,1] regardless of aspect ratio.  On a
/// wide rect (e.g. 400×200), this produced a circle in UV space but an
/// ellipse on screen.  The fix scales the distance by the aspect ratio
/// so the circle is inscribed in min(width, height), matching the CPU path.
#[test]
fn gpu_circle_crop_nonsquare() {
    let mut ctx = require_gpu();
    let canvas_w = 400;
    let canvas_h = 200;

    // Solid green layer covering the full non-square canvas with circle crop.
    let layer = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 0, 255, 0, 255),
        canvas_w,
        canvas_h,
        None,
        1.0,
        0.0,
        0,
        false,
        false,
        1.0,
        CropShape::Circle,
    );

    let layers = vec![Some(layer)];
    let (result, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    let buf = result.as_slice();

    // Centre should be green (inside the circle).
    let mid = ((canvas_h as usize / 2) * canvas_w as usize + canvas_w as usize / 2) * 4;
    assert!(buf[mid + 1] > 200, "Centre should be green (inside circle), G={}", buf[mid + 1]);

    // The circle is inscribed in the shorter dimension (height = 200),
    // so it has radius 100px.  The horizontal centre is at x=200.
    // A point at x = 200 + 100 + 10 = 310, y = 100 (mid-height) is
    // outside the circle horizontally.  If the bug were present (ellipse
    // in screen space), this point would be inside because the ellipse
    // extends to x = 200 ± 200.
    let outside_x = 310_usize; // 10px beyond circle edge horizontally
    let outside_y = canvas_h as usize / 2;
    let off = (outside_y * canvas_w as usize + outside_x) * 4;
    assert!(
        buf[off + 3] < 30,
        "Point beyond circle radius horizontally should be transparent, A={}",
        buf[off + 3]
    );

    // A point at x = 200, y = 100 - 90 = 10 (near top, inside circle)
    // should be green.
    let inside_x = canvas_w as usize / 2;
    let inside_y = 10_usize;
    let off2 = (inside_y * canvas_w as usize + inside_x) * 4;
    assert!(
        buf[off2 + 1] > 200,
        "Point near top at horizontal centre should be inside circle, G={}",
        buf[off2 + 1]
    );
}

#[test]
fn gpu_empty_scene() {
    let mut ctx = require_gpu();
    let canvas_w = 64;
    let canvas_h = 64;

    // No layers, no overlays.
    let layers: Vec<Option<LayerSnapshot>> = Vec::new();
    let (result, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();
    // Canvas should be all transparent (cleared).
    assert!(buf.iter().all(|&b| b == 0), "Empty canvas should be all zeros");
}

#[test]
fn gpu_overlay_compositing() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Background layer: solid blue.
    let bg = make_layer(solid_rgba(canvas_w, canvas_h, 0, 0, 255, 255), canvas_w, canvas_h, None);

    // Image overlay: solid yellow in the centre.
    let ov_w = 80;
    let ov_h = 60;
    let overlay = Arc::new(DecodedOverlay {
        id: "test-overlay".to_string(),
        rgba_data: Arc::from(solid_rgba(ov_w, ov_h, 255, 255, 0, 255)),
        width: ov_w,
        height: ov_h,
        rect: Rect {
            x: (canvas_w - ov_w).cast_signed() / 2,
            y: (canvas_h - ov_h).cast_signed() / 2,
            width: ov_w,
            height: ov_h,
        },
        opacity: 1.0,
        z_index: 10,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        measured_text_width: None,
        measured_text_height: None,
        source_kind: OverlaySourceKind::Raster,
    });

    let layers = vec![Some(bg)];
    let (result, _) =
        ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[overlay], &[], None, None);

    let buf = result.as_slice();

    // Centre should be yellow (overlay on top of blue background).
    let (r, g, b, _a) = avg_centre_pixel(buf, canvas_w, canvas_h);
    // The overlay only covers a small region; avg_centre_pixel samples
    // the centre 50% area which includes both overlay and background.
    // The very centre pixel should be yellow.
    let mid = ((canvas_h as usize / 2) * canvas_w as usize + canvas_w as usize / 2) * 4;
    assert!(buf[mid] > 240, "Centre R should be bright (yellow), got {}", buf[mid]);
    assert!(buf[mid + 1] > 240, "Centre G should be bright (yellow), got {}", buf[mid + 1]);
    // Suppress unused var warning.
    let _ = (r, g, b);
}

#[test]
fn gpu_i420_layer() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Solid green in YUV: Y≈149, U≈43, V≈21 (BT.601).
    let data = solid_i420(canvas_w, canvas_h, 149, 43, 21);
    let layer = LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(data)),
        width: canvas_w,
        height: canvas_h,
        pixel_format: PixelFormat::I420,
        rect: None,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: CropShape::Rect,
    };

    let layers = vec![Some(layer)];
    let (result, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();
    let (r, g, b, _a) = avg_centre_pixel(buf, canvas_w, canvas_h);

    // Should be approximately green.
    assert!(g > r && g > b, "I420 green layer should produce green output: R={r}, G={g}, B={b}");
    assert!(g > 100.0, "Green channel should be strong, got {g}");
}

#[test]
fn gpu_output_nv12_conversion() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    let data = solid_rgba(canvas_w, canvas_h, 255, 0, 0, 255);
    let layer = make_layer(data, canvas_w, canvas_h, None);
    let layers = vec![Some(layer)];

    let (result, fmt) = ctx.composite_frame_gpu(
        canvas_w,
        canvas_h,
        &layers,
        &[],
        &[],
        None,
        Some(PixelFormat::Nv12),
    );

    assert_eq!(fmt, PixelFormat::Nv12);
    let buf = result.as_slice();
    let w = canvas_w as usize;
    let h = canvas_h as usize;
    let expected_y_size = w * h;
    let expected_uv_size = (w / 2) * (h / 2) * 2;
    assert_eq!(
        buf.len(),
        expected_y_size + expected_uv_size,
        "NV12 buffer size mismatch: got {}, expected {}",
        buf.len(),
        expected_y_size + expected_uv_size,
    );

    // Y plane should have non-zero values (red → Y ≈ 82 in BT.601).
    let y_avg: f32 =
        buf[..expected_y_size].iter().map(|&b| f32::from(b)).sum::<f32>() / expected_y_size as f32;
    assert!(y_avg > 50.0 && y_avg < 120.0, "Y average for red should be ~82, got {y_avg}");
}

#[test]
fn gpu_output_i420_conversion() {
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    let data = solid_rgba(canvas_w, canvas_h, 0, 255, 0, 255);
    let layer = make_layer(data, canvas_w, canvas_h, None);
    let layers = vec![Some(layer)];

    let (result, fmt) = ctx.composite_frame_gpu(
        canvas_w,
        canvas_h,
        &layers,
        &[],
        &[],
        None,
        Some(PixelFormat::I420),
    );

    assert_eq!(fmt, PixelFormat::I420);
    let buf = result.as_slice();
    let w = canvas_w as usize;
    let h = canvas_h as usize;
    let expected_size = w * h + 2 * (w / 2) * (h / 2);
    assert_eq!(buf.len(), expected_size, "I420 buffer size mismatch");

    // Y plane should have non-zero values (green → Y ≈ 145 in BT.601).
    let y_avg: f32 = buf[..w * h].iter().map(|&b| f32::from(b)).sum::<f32>() / (w * h) as f32;
    assert!(y_avg > 100.0 && y_avg < 180.0, "Y average for green should be ~145, got {y_avg}");
}

#[test]
fn gpu_canvas_resize() {
    let mut ctx = require_gpu();

    // Composite at one size.
    let data1 = solid_rgba(320, 240, 255, 0, 0, 255);
    let layer1 = make_layer(data1, 320, 240, None);
    let (result1, _) = ctx.composite_frame_gpu(320, 240, &[Some(layer1)], &[], &[], None, None);
    assert_eq!(result1.as_slice().len(), 320 * 240 * 4);

    // Composite at a different size — should reallocate canvas.
    let data2 = solid_rgba(640, 480, 0, 255, 0, 255);
    let layer2 = make_layer(data2, 640, 480, None);
    let (result2, _) = ctx.composite_frame_gpu(640, 480, &[Some(layer2)], &[], &[], None, None);
    assert_eq!(result2.as_slice().len(), 640 * 480 * 4);
}

#[test]
fn gpu_z_ordering() {
    let mut ctx = require_gpu();
    let canvas_w = 64;
    let canvas_h = 64;

    // Layer 0 (z=0): solid red.
    let layer0 = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 255, 0, 0, 255),
        canvas_w,
        canvas_h,
        None,
        1.0,
        0.0,
        0,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    // Layer 1 (z=1): solid green — should be on top.
    let layer1 = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 0, 255, 0, 255),
        canvas_w,
        canvas_h,
        None,
        1.0,
        0.0,
        1,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(layer0), Some(layer1)];
    let (result, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    let buf = result.as_slice();
    let (r, g, _b, _a) = avg_centre_pixel(buf, canvas_w, canvas_h);
    assert!(g > r, "Green (z=1) should be on top of red (z=0): R={r}, G={g}");
    assert!(g > 240.0, "Expected solid green on top, got G={g}");
}

/// Regression test: I420 layer with odd height.
///
/// Before the fix (2ff012f), the YUV→RGBA shader used floor division
/// (`height / 2`) for the V-plane offset, but the Rust packing uses
/// `div_ceil(2)`.  For odd heights the shader read U data as V data,
/// producing wrong chroma.
#[test]
fn gpu_i420_odd_height() {
    let mut ctx = require_gpu();
    // Odd height: 321×241.
    let w = 321_u32;
    let h = 241_u32;

    // Solid green in YUV: Y≈149, U≈43, V≈21 (BT.601).
    let data = solid_i420(w, h, 149, 43, 21);
    let layer = LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(data)),
        width: w,
        height: h,
        pixel_format: PixelFormat::I420,
        rect: None,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: CropShape::Rect,
    };

    let layers = vec![Some(layer)];
    let (result, fmt) = ctx.composite_frame_gpu(w, h, &layers, &[], &[], None, None);

    assert_eq!(fmt, PixelFormat::Rgba8);
    let buf = result.as_slice();
    let (r, g, b, _a) = avg_centre_pixel(buf, w, h);

    // Should be approximately green — if V-plane offset is wrong,
    // chroma will be wildly off.
    assert!(g > r && g > b, "Odd-height I420 should produce green: R={r}, G={g}, B={b}");
    assert!(g > 100.0, "Green channel should be strong for odd-height I420, got {g}");
}

/// Regression test: circle crop combined with crop_zoom > 1.0.
///
/// Before the fix (b74cfa7), the circle distance was computed in
/// source-remapped UV space.  With crop_zoom > 1.0 the UV range is
/// compressed, making the circle mask too large and failing to clip
/// corner pixels.
#[test]
fn gpu_circle_crop_with_zoom() {
    let mut ctx = require_gpu();
    let canvas_w = 320_u32;
    let canvas_h = 320_u32;

    // Solid green layer with circle crop AND 2× crop zoom.
    let layer = make_layer_with_props(
        solid_rgba(canvas_w, canvas_h, 0, 255, 0, 255),
        canvas_w,
        canvas_h,
        None,
        1.0,
        0.0,
        0,
        false,
        false,
        2.0, // crop_zoom > 1.0
        CropShape::Circle,
    );

    let layers = vec![Some(layer)];
    let (result, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);
    let buf = result.as_slice();

    // Centre should be green (inside the circle).
    let mid = ((canvas_h as usize / 2) * canvas_w as usize + canvas_w as usize / 2) * 4;
    assert!(buf[mid + 1] > 200, "Centre should be green with circle+zoom, G={}", buf[mid + 1],);

    // Corner should be transparent (outside the circle).  Before the
    // fix, the compressed UV range made the circle too large and the
    // corner would be green instead of transparent.
    let corner = 4; // pixel (1, 0)
    assert!(
        buf[corner + 3] < 30,
        "Corner should be transparent with circle+zoom, A={}",
        buf[corner + 3],
    );
}

#[test]
fn gpu_should_use_gpu_heuristic() {
    use super::gpu::should_use_gpu;

    // Single small layer, RGBA output → CPU.
    let small_layer = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    assert!(
        !should_use_gpu(320, 240, &[Some(small_layer)], &[], &[], None),
        "Single small layer should prefer CPU"
    );

    // Two layers → GPU.
    let l1 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let l2 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    assert!(
        should_use_gpu(320, 240, &[Some(l1), Some(l2)], &[], &[], None),
        "Two layers should prefer GPU"
    );

    // 1080p single layer → GPU (high pixel count).
    let big_layer = make_layer(solid_rgba(1920, 1080, 0, 0, 0, 255), 1920, 1080, None);
    assert!(
        should_use_gpu(1920, 1080, &[Some(big_layer)], &[], &[], None),
        "1080p should prefer GPU"
    );

    // Single layer with rotation → GPU (effects).
    let rotated = make_layer_with_props(
        solid_rgba(320, 240, 0, 0, 0, 255),
        320,
        240,
        None,
        1.0,
        45.0,
        0,
        false,
        false,
        1.0,
        CropShape::Rect,
    );
    assert!(
        should_use_gpu(320, 240, &[Some(rotated)], &[], &[], None),
        "Rotated layer should prefer GPU"
    );

    // Single small layer with NV12 output → GPU (YUV conversion offload).
    let nv12_layer = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    assert!(
        should_use_gpu(320, 240, &[Some(nv12_layer)], &[], &[], Some(PixelFormat::Nv12)),
        "NV12 output should prefer GPU even for single small layer"
    );

    // Single small layer with I420 output → GPU (YUV conversion offload).
    let i420_layer = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    assert!(
        should_use_gpu(320, 240, &[Some(i420_layer)], &[], &[], Some(PixelFormat::I420)),
        "I420 output should prefer GPU even for single small layer"
    );

    // Single small layer with RGBA8 output → CPU (no conversion needed).
    let rgba_layer = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    assert!(
        !should_use_gpu(320, 240, &[Some(rgba_layer)], &[], &[], Some(PixelFormat::Rgba8)),
        "RGBA8 output should prefer CPU for single small layer"
    );
}

// ── Phase 2 tests ───────────────────────────────────────────────────────────

#[test]
fn gpu_multi_frame_pooling() {
    // Composite 10 frames consecutively with the same scene.
    // Pool reuse should produce identical output to a single-frame baseline.
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    let layer_data = solid_rgba(canvas_w, canvas_h, 200, 100, 50, 255);
    let layers = vec![Some(make_layer(layer_data.clone(), canvas_w, canvas_h, None))];

    // Single-frame baseline.
    let (baseline, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);
    let baseline_avg = avg_centre_pixel(baseline.as_slice(), canvas_w, canvas_h);

    // Composite 9 more frames and verify each matches the baseline.
    for frame in 1..10 {
        let (output, _) =
            ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);
        let avg = avg_centre_pixel(output.as_slice(), canvas_w, canvas_h);
        assert!(
            (avg.0 - baseline_avg.0).abs() < 2.0
                && (avg.1 - baseline_avg.1).abs() < 2.0
                && (avg.2 - baseline_avg.2).abs() < 2.0,
            "Frame {frame} output diverged from baseline: {avg:?} vs {baseline_avg:?}"
        );
    }
}

#[test]
fn gpu_pool_dimension_change() {
    // Composite at 320×240, then 640×480, then back to 320×240.
    // Pool should handle dimension changes correctly.
    let mut ctx = require_gpu();

    // First size: 320×240.
    let small_data = solid_rgba(320, 240, 255, 0, 0, 255);
    let small_layers = vec![Some(make_layer(small_data, 320, 240, None))];
    let (out1, _) = ctx.composite_frame_gpu(320, 240, &small_layers, &[], &[], None, None);
    let avg1 = avg_centre_pixel(out1.as_slice(), 320, 240);

    // Second size: 640×480.
    let big_data = solid_rgba(640, 480, 0, 255, 0, 255);
    let big_layers = vec![Some(make_layer(big_data, 640, 480, None))];
    let (out2, _) = ctx.composite_frame_gpu(640, 480, &big_layers, &[], &[], None, None);
    let avg2 = avg_centre_pixel(out2.as_slice(), 640, 480);

    // Third: back to 320×240 with original colour.
    let small_data2 = solid_rgba(320, 240, 255, 0, 0, 255);
    let small_layers2 = vec![Some(make_layer(small_data2, 320, 240, None))];
    let (out3, _) = ctx.composite_frame_gpu(320, 240, &small_layers2, &[], &[], None, None);
    let avg3 = avg_centre_pixel(out3.as_slice(), 320, 240);

    // Verify colours are correct at each stage.
    assert!(avg1.0 > 200.0, "First frame should be red: {avg1:?}");
    assert!(avg2.1 > 200.0, "Second frame should be green: {avg2:?}");
    assert!(avg3.0 > 200.0, "Third frame should be red again: {avg3:?}");

    // First and third frames should match closely (same scene).
    assert!(
        (avg1.0 - avg3.0).abs() < 2.0 && (avg1.1 - avg3.1).abs() < 2.0,
        "Return to original size should produce same output: {avg1:?} vs {avg3:?}"
    );
}

#[test]
fn gpu_yuv_batch_correctness() {
    // Multi-YUV-layer scene: verify batched YUV→RGBA submission
    // produces correct output (no corruption from shared encoder).
    let mut ctx = require_gpu();
    let canvas_w = 320;
    let canvas_h = 240;

    // Two I420 layers with different colours, composited together.
    let yuv_data1 = solid_i420(canvas_w, canvas_h, 235, 128, 128); // white-ish
    let yuv_data2 = solid_i420(canvas_w, canvas_h, 16, 128, 128); // black-ish

    let layer1 = LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(yuv_data1)),
        width: canvas_w,
        height: canvas_h,
        pixel_format: PixelFormat::I420,
        rect: None,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: CropShape::Rect,
    };
    let layer2 = LayerSnapshot {
        data: Arc::new(PooledVideoData::from_vec(yuv_data2)),
        width: canvas_w,
        height: canvas_h,
        pixel_format: PixelFormat::I420,
        rect: None,
        opacity: 1.0,
        z_index: 1,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: CropShape::Rect,
    };

    let layers = vec![Some(layer1), Some(layer2)];
    let (output, fmt) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);

    // Output should be valid RGBA8.
    assert_eq!(fmt, PixelFormat::Rgba8);
    assert_eq!(output.as_slice().len(), (canvas_w as usize) * (canvas_h as usize) * 4);

    // The top layer (z=1) is black-ish — centre pixels should be dark.
    let avg = avg_centre_pixel(output.as_slice(), canvas_w, canvas_h);
    assert!(
        avg.0 < 40.0 && avg.1 < 40.0 && avg.2 < 40.0,
        "Top YUV layer should be dark, got {avg:?}"
    );
}

#[test]
fn gpu_hysteresis_stability() {
    // Unit test: verify should_use_gpu_with_state requires N consecutive
    // frames voting opposite before flipping.

    // Seed with CPU (false) to test the flip-to-GPU path.
    let mut state = GpuPathState::new_seeded(false);

    // Build a scene that votes GPU.
    let l1 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let l2 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let gpu_scene: Vec<Option<LayerSnapshot>> = vec![Some(l1), Some(l2)];

    // First 4 frames voting GPU should NOT flip (hysteresis = 5).
    for _ in 0..4 {
        let result =
            gpu::should_use_gpu_with_state(&mut state, 320, 240, &gpu_scene, &[], &[], None);
        assert!(!result, "Should stay on CPU during hysteresis window");
    }

    // 5th consecutive frame should flip to GPU.
    let result = gpu::should_use_gpu_with_state(&mut state, 320, 240, &gpu_scene, &[], &[], None);
    assert!(result, "Should flip to GPU after 5 consecutive votes");

    // Now on GPU. Build a scene that votes CPU.
    let cpu_layer = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let cpu_scene: Vec<Option<LayerSnapshot>> = vec![Some(cpu_layer)];

    // Interleave: vote CPU, then vote GPU — should reset the counter.
    for _ in 0..3 {
        gpu::should_use_gpu_with_state(&mut state, 320, 240, &cpu_scene, &[], &[], None);
    }
    // Interrupt with a GPU vote (re-add two layers).
    let l3 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let l4 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let gpu_scene2: Vec<Option<LayerSnapshot>> = vec![Some(l3), Some(l4)];
    let result = gpu::should_use_gpu_with_state(&mut state, 320, 240, &gpu_scene2, &[], &[], None);
    assert!(result, "Interruption should reset counter; stay on GPU");
}

#[test]
fn gpu_hysteresis_seeded_skips_warmup() {
    // Verify that seeding with `true` avoids the warm-up period:
    // on the very first frame the GPU path is used immediately.
    let mut state = GpuPathState::new_seeded(true);

    let l1 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let l2 = make_layer(solid_rgba(320, 240, 0, 0, 0, 255), 320, 240, None);
    let gpu_scene: Vec<Option<LayerSnapshot>> = vec![Some(l1), Some(l2)];

    // First frame should immediately use GPU — no warm-up.
    let result = gpu::should_use_gpu_with_state(&mut state, 320, 240, &gpu_scene, &[], &[], None);
    assert!(result, "Seeded state should use GPU on the very first frame");
}

#[test]
fn gpu_runtime_mode_switch() {
    // Verify GpuMode::from_u8 roundtrips correctly for all variants.
    assert_eq!(GpuMode::from_u8(GpuMode::Auto as u8), GpuMode::Auto);
    assert_eq!(GpuMode::from_u8(GpuMode::ForceGpu as u8), GpuMode::ForceGpu);
    assert_eq!(GpuMode::from_u8(GpuMode::ForceCpu as u8), GpuMode::ForceCpu);

    // Unknown values should map to Auto.
    assert_eq!(GpuMode::from_u8(3), GpuMode::Auto);
    assert_eq!(GpuMode::from_u8(255), GpuMode::Auto);

    // Verify the repr(u8) values are stable.
    assert_eq!(GpuMode::Auto as u8, 0);
    assert_eq!(GpuMode::ForceGpu as u8, 1);
    assert_eq!(GpuMode::ForceCpu as u8, 2);
}

/// Regression test: GPU rotation on a non-square canvas must match the CPU
/// path.
///
/// Before the fix, `build_layer_uniforms` applied rotation in NDC space
/// (where both axes are normalised to [-1,1]).  On a non-square canvas
/// (e.g. 400×200) the differing NDC-to-pixel ratios per axis caused the
/// rotation to skew the layer — producing a diamond-like shape instead of
/// a properly rotated rectangle.
///
/// The fix rotates in pixel space (using cross-axis canvas dimensions in
/// the off-diagonal sin terms) so the shape is preserved regardless of
/// canvas aspect ratio.
///
/// This test uses a left-red / right-green split image on a 400×200
/// canvas, rotated 90° CW.  After rotation the top half should be red
/// (original left) and the bottom half green (original right) — the same
/// result the CPU path produces.  The non-square canvas (2:1) is the key
/// ingredient that exposed the original bug.
#[test]
fn gpu_rotation_nonsquare_canvas_matches_cpu() {
    let mut ctx = require_gpu();
    let canvas_w = 400_u32;
    let canvas_h = 200_u32;

    // Build a left-red / right-green split image covering the full canvas.
    let mut data = vec![0u8; (canvas_w as usize) * (canvas_h as usize) * 4];
    for y in 0..canvas_h as usize {
        for x in 0..canvas_w as usize {
            let off = (y * canvas_w as usize + x) * 4;
            if x < canvas_w as usize / 2 {
                data[off] = 255; // R
            } else {
                data[off + 1] = 255; // G
            }
            data[off + 3] = 255; // A
        }
    }

    let layer = make_layer_with_props(
        data,
        canvas_w,
        canvas_h,
        None,
        1.0,
        90.0, // 90° clockwise
        0,
        false,
        false,
        1.0,
        CropShape::Rect,
    );

    let layers = vec![Some(layer)];
    let (result, _) = ctx.composite_frame_gpu(canvas_w, canvas_h, &layers, &[], &[], None, None);
    let buf = result.as_slice();

    // After 90° CW rotation on a non-square canvas:
    //   - original left (red)   → top
    //   - original right (green) → bottom
    //
    // Sample the vertical centre column at 25% and 75% height.
    let mid_x = canvas_w as usize / 2;

    let top_y = canvas_h as usize / 4;
    let top_off = (top_y * canvas_w as usize + mid_x) * 4;
    assert!(
        buf[top_off] > 200 && buf[top_off + 1] < 55,
        "Top-centre should be red after 90° CW on non-square canvas: R={}, G={}",
        buf[top_off],
        buf[top_off + 1],
    );

    let bot_y = canvas_h as usize * 3 / 4;
    let bot_off = (bot_y * canvas_w as usize + mid_x) * 4;
    assert!(
        buf[bot_off + 1] > 200 && buf[bot_off] < 55,
        "Bottom-centre should be green after 90° CW on non-square canvas: R={}, G={}",
        buf[bot_off],
        buf[bot_off + 1],
    );
}
