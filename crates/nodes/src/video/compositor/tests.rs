// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::test_utils::{
    assert_state_initializing, assert_state_running, assert_state_stopped,
    create_oneshot_test_context, create_test_context,
};
use crate::video::pixel_ops;
use crate::video::pixel_ops::{scale_blit_rgba, scale_blit_rgba_rotated, BlitRect};
use config::{LayerConfig, Rect};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Create a solid-colour RGBA8 VideoFrame.
fn make_rgba_frame(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> VideoFrame {
    let total = (width as usize) * (height as usize) * 4;
    let mut data = vec![0u8; total];
    for pixel in data.chunks_exact_mut(4) {
        pixel[0] = r;
        pixel[1] = g;
        pixel[2] = b;
        pixel[3] = a;
    }
    VideoFrame::new(width, height, PixelFormat::Rgba8, data).unwrap()
}

// ── Unit tests for compositing helpers ───────────────────────────────

#[test]
fn test_scale_blit_identity() {
    // 2x2 red source blitted onto a 4x4 canvas at (1,1) 2x2 rect.
    let src = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 128, 128, 255];
    let mut dst = vec![0u8; 4 * 4 * 4]; // 4x4 RGBA, all transparent black

    scale_blit_rgba(
        &mut dst,
        4,
        4,
        &src,
        2,
        2,
        &BlitRect { x: 1, y: 1, width: 2, height: 2 },
        1.0,
        false,
        false,
        false,
        None,
        false,
    );

    // Pixel at (1,1) should be red.
    let x = 1usize;
    let y = 1usize;
    let idx = (y * 4 + x) * 4;
    assert_eq!(dst[idx], 255);
    assert_eq!(dst[idx + 1], 0);
    assert_eq!(dst[idx + 2], 0);
    assert_eq!(dst[idx + 3], 255);

    // Pixel at (0,0) should remain transparent black.
    assert_eq!(dst[0], 0);
    assert_eq!(dst[3], 0);
}

#[test]
fn test_scale_blit_with_opacity() {
    // White source at 50% opacity over black background.
    let src = vec![255, 255, 255, 255]; // 1x1 white
    let mut dst = vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]; // 2x2 black

    scale_blit_rgba(
        &mut dst,
        2,
        2,
        &src,
        1,
        1,
        &BlitRect { x: 0, y: 0, width: 1, height: 1 },
        0.5,
        false,
        false,
        false,
        None,
        false,
    );

    // Pixel (0,0): white at 50% over opaque black -> ~128 grey.
    let r = dst[0];
    assert!(r > 120 && r < 135, "Expected ~128, got {r}");
}

#[test]
fn test_scale_blit_scaling() {
    // 1x1 red source scaled to 4x4 rect on an 8x8 canvas.
    let src = vec![255, 0, 0, 255];
    let mut dst = vec![0u8; 8 * 8 * 4];

    scale_blit_rgba(
        &mut dst,
        8,
        8,
        &src,
        1,
        1,
        &BlitRect { x: 2, y: 2, width: 4, height: 4 },
        1.0,
        false,
        false,
        false,
        None,
        false,
    );

    // All pixels in the 4x4 destination rect should be red.
    for y in 2..6u32 {
        for x in 2..6u32 {
            let idx = ((y * 8 + x) * 4) as usize;
            assert_eq!(dst[idx], 255, "Red at ({x},{y})");
            assert_eq!(dst[idx + 1], 0, "Green at ({x},{y})");
        }
    }
    // Outside should remain black.
    assert_eq!(dst[0], 0);
}

#[test]
fn test_rotated_blit_stretch_to_fill() {
    // A wide 4×2 red source blitted into a square 20×20 rect with 45°
    // rotation on a 40×40 canvas.
    //
    // The source is stretched to fill the 20×20 rect (no aspect-ratio
    // fit), then rotated 45°.  The centre of the rect (canvas pixel
    // 20,20) should be covered by red source pixels, while the rect
    // corner (10,10) — outside the rotated area — should remain
    // transparent.
    let src = [255u8, 0, 0, 255].repeat(4 * 2); // 4×2 solid red
    let mut dst = vec![0u8; 40 * 40 * 4];

    scale_blit_rgba_rotated(
        &mut dst,
        40,
        40,
        &src,
        4,
        2,
        &BlitRect { x: 10, y: 10, width: 20, height: 20 },
        1.0,
        45.0,
        false,
        false,
        false,
        None,
        false,
    );

    // The centre of the rect (canvas pixel 20,20) should be covered
    // by source content (red).
    let cx = 20usize;
    let cy = 20usize;
    let idx = (cy * 40 + cx) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert_eq!(dst[idx + 1], 0, "Centre G");
    assert_eq!(dst[idx + 2], 0, "Centre B");
    assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");

    // The rect corner (10,10) is outside the rotated content area
    // and should remain transparent.
    let corner_idx = (10usize * 40 + 10) * 4;
    assert_eq!(dst[corner_idx + 3], 0, "Rect corner should be transparent");
}

#[test]
fn test_composite_frame_empty_layers() {
    // No layers, no overlays -> transparent black canvas.
    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[], &[], &[], None, &mut cache);
    let buf = result.as_slice();
    assert_eq!(buf.len(), 4 * 4 * 4);
    assert!(buf.iter().all(|&b| b == 0));
}

#[test]
fn test_composite_frame_single_layer() {
    let data = make_rgba_frame(2, 2, 255, 0, 0, 255);
    let layer = LayerSnapshot {
        data: data.data,
        width: 2,
        height: 2,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Entire canvas should be red (scaled from 2x2 to 4x4).
    for pixel in buf.chunks_exact(4) {
        assert_eq!(pixel[0], 255, "Red channel");
        assert_eq!(pixel[1], 0, "Green channel");
        assert_eq!(pixel[2], 0, "Blue channel");
        assert_eq!(pixel[3], 255, "Alpha channel");
    }
}

#[test]
fn test_composite_frame_two_layers() {
    // Bottom: full-canvas red. Top: small green square at (1,1) 2x2.
    let red = make_rgba_frame(4, 4, 255, 0, 0, 255);
    let green = make_rgba_frame(2, 2, 0, 255, 0, 255);

    let layer0 = LayerSnapshot {
        data: red.data,
        width: 4,
        height: 4,
        pixel_format: PixelFormat::Rgba8,
        rect: None,
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Rect,
    };
    let layer1 = LayerSnapshot {
        data: green.data,
        width: 2,
        height: 2,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 1, y: 1, width: 2, height: 2 }),
        opacity: 1.0,
        z_index: 1,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer0), Some(layer1)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // (0,0) should be red.
    assert_eq!(buf[0], 255);
    assert_eq!(buf[1], 0);

    // (1,1) should be green (overwritten by top layer).
    let x = 1usize;
    let y = 1usize;
    let idx = (y * 4 + x) * 4;
    assert_eq!(buf[idx], 0);
    assert_eq!(buf[idx + 1], 255);
    assert_eq!(buf[idx + 2], 0);
}

#[test]
fn test_rasterize_text_overlay_produces_pixels() {
    let cfg = config::TextOverlayConfig {
        id: "test-text".to_string(),
        text: "Hi".to_string(),
        transform: config::OverlayTransform {
            rect: Rect { x: 0, y: 0, width: 64, height: 32 },
            opacity: 1.0,
            rotation_degrees: 0.0,
            z_index: 0,
            mirror_horizontal: false,
            mirror_vertical: false,
        },
        color: [255, 255, 0, 255],
        font_size: 24,
        font_name: None,
    };
    let overlay = rasterize_text_overlay(&cfg, 7680, 10_000);
    // Bitmap is sized to the measured text extent, not the config rect.
    assert!(overlay.width > 0, "rasterized width must be positive");
    assert!(overlay.height > 0, "rasterized height must be positive");
    // The rect in the returned overlay should match the bitmap dimensions.
    assert_eq!(overlay.rect.width, overlay.width);
    assert_eq!(overlay.rect.height, overlay.height);
    // Should have some non-zero pixels (text was drawn).
    assert!(overlay.rgba_data.iter().any(|&b| b > 0));
}

#[test]
fn test_fit_rect_preserving_aspect() {
    // 4:3 source into 16:9 bounds → pillarboxed (width-limited)
    let bounds = Rect { x: 100, y: 50, width: 426, height: 240 };
    let fitted = fit_rect_preserving_aspect(640, 480, &bounds);
    // Scale = min(426/640, 240/480) = min(0.666, 0.5) = 0.5
    // Fitted: 320×240, centred within 426×240
    assert_eq!(fitted.width, 320);
    assert_eq!(fitted.height, 240);
    assert_eq!(fitted.x, 100 + (426 - 320) / 2);
    assert_eq!(fitted.y, 50);

    // 16:9 source into 4:3 bounds → letterboxed (height-limited)
    let bounds = Rect { x: 0, y: 0, width: 400, height: 400 };
    let fitted = fit_rect_preserving_aspect(1280, 720, &bounds);
    // Scale = min(400/1280, 400/720) = min(0.3125, 0.555) = 0.3125
    // Fitted: 400×225, centred within 400×400
    assert_eq!(fitted.width, 400);
    assert_eq!(fitted.height, 225);
    assert_eq!(fitted.x, 0);
    assert_eq!(fitted.y, (400 - 225) / 2);

    // Exact match → no change
    let bounds = Rect { x: 10, y: 20, width: 640, height: 480 };
    let fitted = fit_rect_preserving_aspect(640, 480, &bounds);
    assert_eq!(fitted.width, 640);
    assert_eq!(fitted.height, 480);
    assert_eq!(fitted.x, 10);
    assert_eq!(fitted.y, 20);
}

#[test]
fn test_resolved_layer_source_dims() {
    // When a slot has a latest_frame, resolve_scene should populate source dims.
    let config = CompositorConfig::default();
    let (_, rx) = mpsc::channel(1);
    let slots = vec![InputSlot {
        name: "in_0".to_string(),
        rx,
        latest_frame: Some(make_rgba_frame(1920, 1080, 0, 0, 0, 255)),
        last_source_dims: Some((1920, 1080)),
        hint_tx: None,
    }];
    let image_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);
    let text_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);

    let scene = resolve_scene(&slots, &config, &image_overlays, &text_overlays);
    assert_eq!(scene.layout.layers.len(), 1);
    assert_eq!(scene.layout.layers[0].source_width, Some(1920));
    assert_eq!(scene.layout.layers[0].source_height, Some(1080));
}

#[test]
fn test_resolved_layer_source_dims_none_when_no_frame() {
    // When a slot has no latest_frame, source dims should be None.
    let config = CompositorConfig::default();
    let (_, rx) = mpsc::channel(1);
    let slots = vec![InputSlot {
        name: "in_0".to_string(),
        rx,
        latest_frame: None,
        last_source_dims: None,
        hint_tx: None,
    }];
    let image_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);
    let text_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);

    let scene = resolve_scene(&slots, &config, &image_overlays, &text_overlays);
    assert_eq!(scene.layout.layers.len(), 1);
    assert_eq!(scene.layout.layers[0].source_width, None);
    assert_eq!(scene.layout.layers[0].source_height, None);
}

#[test]
fn test_config_validate_ok() {
    let cfg = CompositorConfig::default();
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_ok());
}

#[test]
fn test_config_validate_zero_dimensions() {
    let cfg = CompositorConfig { width: 0, height: 720, ..Default::default() };
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_err());
}

#[test]
fn test_config_validate_bad_opacity() {
    let mut cfg = CompositorConfig::default();
    cfg.layers.insert("in_0".to_string(), LayerConfig { opacity: 1.5, ..Default::default() });
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_err());
}

// ── Integration test: node run() with mock context ──────────────────

#[tokio::test]
async fn test_compositor_node_run_main_only() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = CompositorConfig { width: 4, height: 4, ..Default::default() };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send a red frame.
    let frame = make_rgba_frame(2, 2, 255, 0, 0, 255);
    input_tx.send(Packet::Video(frame)).await.unwrap();

    // Give time for processing.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Close input.
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Expected at least 1 output frame");

    // Verify output is 4x4 RGBA.
    if let Packet::Video(ref out_frame) = output_packets[0] {
        assert_eq!(out_frame.width, 4);
        assert_eq!(out_frame.height, 4);
        assert_eq!(out_frame.pixel_format, PixelFormat::Rgba8);
        // Should be red (2x2 scaled to fill 4x4).
        assert_eq!(out_frame.data()[0], 255); // R
        assert_eq!(out_frame.data()[1], 0); // G
    } else {
        panic!("Expected video packet");
    }
}

#[tokio::test]
async fn test_compositor_node_generates_own_timestamps() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = CompositorConfig { width: 2, height: 2, ..Default::default() };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send a frame with an arbitrary input timestamp — in live mode
    // the compositor passes it through, but this test context is NOT
    // oneshot (it uses the default dynamic context), so the input
    // timestamp is propagated.
    let mut frame = make_rgba_frame(2, 2, 100, 100, 100, 255);
    frame.metadata = Some(PacketMetadata {
        timestamp_us: Some(42_000),
        duration_us: Some(33_333),
        sequence: Some(7),
        keyframe: Some(true),
    });
    input_tx.send(Packet::Video(frame)).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty());

    if let Packet::Video(ref out_frame) = output_packets[0] {
        let meta = out_frame.metadata.as_ref().expect("metadata should be present");
        // Live mode: highest-indexed input's timestamp is used.
        // Single input (in_0) has timestamp 42000.
        assert_eq!(meta.timestamp_us, Some(42_000));
        assert_eq!(meta.duration_us, Some(33_333));
        assert_eq!(meta.sequence, Some(0));
    } else {
        panic!("Expected video packet");
    }
}

#[test]
fn test_compositor_definition_pins() {
    let (inputs, outputs) = CompositorNode::definition_pins();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].name, "in");
    assert!(matches!(inputs[0].cardinality, PinCardinality::Dynamic { .. }));
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].name, "out");
}

#[test]
fn test_compositor_pool_usage() {
    use streamkit_core::frame_pool::FramePool;

    let canvas_w = 4u32;
    let canvas_h = 4u32;
    let total = (canvas_w as usize) * (canvas_h as usize) * 4; // 64 bytes

    let pool = FramePool::<u8>::preallocated(&[total], 2);
    assert_eq!(pool.stats().buckets[0].available, 2);

    let mut cache = ConversionCache::new();
    let result = composite_frame(canvas_w, canvas_h, &[], &[], &[], Some(&pool), &mut cache);
    assert_eq!(result.as_slice().len(), total);
    // One buffer was taken from the pool.
    assert_eq!(pool.stats().buckets[0].available, 1);

    // Drop returns to pool.
    drop(result);
    assert_eq!(pool.stats().buckets[0].available, 2);
}

// ── SIMD vs scalar equivalence tests ────────────────────────────────

/// Helper: scalar I420→RGBA8 conversion for a single pixel (reference).
#[allow(clippy::many_single_char_names)]
fn scalar_i420_to_rgba8(y: u8, u: u8, v: u8) -> [u8; 4] {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
    [r, g, b, 255]
}

/// Helper: scalar RGBA8→Y for a single pixel (reference).
fn scalar_rgba8_to_y(r: u8, g: u8, b: u8) -> u8 {
    let y = ((66 * i32::from(r) + 129 * i32::from(g) + 25 * i32::from(b) + 128) >> 8) + 16;
    y.clamp(0, 255) as u8
}

#[test]
fn test_i420_to_rgba8_simd_matches_scalar() {
    // Test a variety of YUV values, including edge cases that trigger
    // i16 overflow with the BT.601 coefficients.
    let test_cases: Vec<(u8, u8, u8)> = vec![
        (16, 128, 128),  // black
        (235, 128, 128), // white
        (81, 90, 240),   // pure red
        (145, 54, 34),   // pure green
        (41, 240, 110),  // pure blue
        (255, 128, 128), // max Y
        (0, 0, 0),       // min everything
        (255, 255, 255), // max everything
        (16, 0, 255),    // extreme chroma
        (235, 255, 0),   // extreme chroma
    ];

    let width = test_cases.len() as u32;
    // Build I420 buffer.
    let mut y_plane = Vec::new();
    let mut u_plane = Vec::new();
    let mut v_plane = Vec::new();
    for &(y, u, v) in &test_cases {
        y_plane.push(y);
        // Each chroma sample covers 2 luma pixels horizontally.
        if y_plane.len() % 2 == 1 {
            u_plane.push(u);
            v_plane.push(v);
        }
    }
    let chroma_w = (width as usize).div_ceil(2);
    // Pad if needed.
    while u_plane.len() < chroma_w {
        u_plane.push(128);
        v_plane.push(128);
    }

    let mut i420_data = Vec::new();
    i420_data.extend_from_slice(&y_plane);
    i420_data.extend_from_slice(&u_plane);
    i420_data.extend_from_slice(&v_plane);

    // Convert using the public function (which uses SIMD internally).
    let mut simd_out = vec![0u8; width as usize * 4];
    pixel_ops::i420_to_rgba8_buf(&i420_data, width, 1, &mut simd_out);

    // Compare with scalar reference.
    for (i, &(y, _u, _v)) in test_cases.iter().enumerate() {
        // For chroma, each sample covers 2 pixels, so use the chroma
        // value from the corresponding pair.
        let chroma_idx = i / 2;
        let actual_u = u_plane[chroma_idx];
        let actual_v = v_plane[chroma_idx];
        let expected = scalar_i420_to_rgba8(y, actual_u, actual_v);
        let got = &simd_out[i * 4..(i + 1) * 4];
        assert_eq!(
            got, &expected,
            "pixel {i}: Y={y} U={actual_u} V={actual_v} → expected {expected:?}, got {got:?}"
        );
    }
}

#[test]
fn test_rgba8_to_i420_simd_matches_scalar() {
    // Test RGBA→Y conversion with values that trigger i16 overflow
    // (129 * 255 = 32895 > i16::MAX).
    let test_pixels: Vec<(u8, u8, u8)> = vec![
        (0, 0, 0),       // black
        (255, 255, 255), // white
        (255, 0, 0),     // red
        (0, 255, 0),     // green
        (0, 0, 255),     // blue
        (128, 128, 128), // mid grey
        (0, 254, 0),     // just below overflow threshold
        (0, 255, 0),     // at overflow threshold
    ];

    let width = test_pixels.len() as u32;
    let mut rgba_data = Vec::with_capacity(width as usize * 4);
    for &(r, g, b) in &test_pixels {
        rgba_data.extend_from_slice(&[r, g, b, 255]);
    }

    // Convert using the public function (SIMD internally).
    let i420_size = width as usize + 2 * (width as usize).div_ceil(2);
    let mut i420_out = vec![0u8; i420_size];
    pixel_ops::rgba8_to_i420_buf(&rgba_data, width, 1, &mut i420_out);

    // Check Y plane matches scalar.
    for (i, &(r, g, b)) in test_pixels.iter().enumerate() {
        let expected_y = scalar_rgba8_to_y(r, g, b);
        let got_y = i420_out[i];
        assert_eq!(
            got_y, expected_y,
            "pixel {i}: R={r} G={g} B={b} → expected Y={expected_y}, got Y={got_y}"
        );
    }
}

#[test]
fn test_i420_rgba8_roundtrip_preserves_values() {
    // A full I420→RGBA8→I420 round-trip should produce values close
    // to the originals (within ±2 due to integer rounding).
    let width: u32 = 8;
    let height: u32 = 2;
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w.div_ceil(2);

    // Build a simple I420 test pattern.
    let mut i420_data = vec![0u8; w * h + 2 * chroma_w * (h / 2)];
    // Y plane: gradient.
    for (i, val) in i420_data[..w * h].iter_mut().enumerate() {
        *val = (16 + (i * 219 / (w * h))) as u8;
    }
    // U/V planes: mid-range.
    let u_offset = w * h;
    let v_offset = u_offset + chroma_w * (h / 2);
    for i in 0..chroma_w * (h / 2) {
        i420_data[u_offset + i] = 128;
        i420_data[v_offset + i] = 128;
    }

    // I420 → RGBA8 → I420
    let mut rgba = vec![0u8; w * h * 4];
    pixel_ops::i420_to_rgba8_buf(&i420_data, width, height, &mut rgba);
    let mut i420_roundtrip = vec![0u8; i420_data.len()];
    pixel_ops::rgba8_to_i420_buf(&rgba, width, height, &mut i420_roundtrip);

    // Y values should be close (within ±2 of originals due to rounding).
    for (idx, orig_val) in i420_data[..w * h].iter().enumerate() {
        let orig = i32::from(*orig_val);
        let rt = i32::from(i420_roundtrip[idx]);
        assert!(
            (orig - rt).abs() <= 2,
            "Y[{idx}]: original={orig}, roundtrip={rt}, diff={}",
            (orig - rt).abs()
        );
    }
}

/// Test that `scale_blit_rgba` with opacity < 1.0 writes all rows correctly
/// on a buffer wide enough to exercise the AVX2 blend path (32 pixels).
/// This verifies the AVX2 → SSE2 → scalar cascade in `blit_row_alpha`.
#[test]
fn test_scale_blit_opacity_all_rows_written() {
    let w = 32usize;
    let h = 32usize;
    // Fully opaque red source.
    let src: Vec<u8> = [200, 50, 30, 255].repeat(w * h);
    // All-black destination (simulates cleared canvas).
    let mut dst = vec![0u8; w * h * 4];

    scale_blit_rgba(
        &mut dst,
        w as u32,
        h as u32,
        &src,
        w as u32,
        h as u32,
        &BlitRect { x: 0, y: 0, width: w as u32, height: h as u32 },
        0.9,
        false,
        false,
        false,
        None,
        false,
    );

    // Every single row should have been written to (non-zero pixels).
    for row in 0..h {
        let row_start = row * w * 4;
        let row_slice = &dst[row_start..row_start + w * 4];
        let any_written = row_slice.iter().any(|&b| b != 0);
        assert!(any_written, "Row {row} was not written to (all zeros)");

        // Verify each pixel matches the expected scalar blend.
        // opacity_u16 = (0.9 * 255 + 0.5) as u16 = 230
        // sa_eff = (255 * 230 + 128) >> 8 = 229
        // Dst is black (0), so blended = src * sa_eff / 255.
        let opacity_u16: u16 = 230;
        let sa_eff = ((255u16 * opacity_u16 + 128) >> 8).min(255);
        let expected_r = {
            let blend = 200u16 * sa_eff + 128;
            ((blend + (blend >> 8)) >> 8) as u8
        };
        let expected_g = {
            let blend = 50u16 * sa_eff + 128;
            ((blend + (blend >> 8)) >> 8) as u8
        };
        let expected_b = {
            let blend = 30u16 * sa_eff + 128;
            ((blend + (blend >> 8)) >> 8) as u8
        };
        for col in 0..w {
            let idx = row_start + col * 4;
            let got_r = dst[idx];
            let got_g = dst[idx + 1];
            let got_b = dst[idx + 2];
            let got_a = dst[idx + 3];

            // Allow ±1 for rounding differences between SIMD and scalar paths.
            assert!(
                (i16::from(got_r) - i16::from(expected_r)).abs() <= 1,
                "Row {row}, Col {col}: R={got_r}, expected ~{expected_r}"
            );
            assert!(
                (i16::from(got_g) - i16::from(expected_g)).abs() <= 1,
                "Row {row}, Col {col}: G={got_g}, expected ~{expected_g}"
            );
            assert!(
                (i16::from(got_b) - i16::from(expected_b)).abs() <= 1,
                "Row {row}, Col {col}: B={got_b}, expected ~{expected_b}"
            );
            assert!(got_a > 200, "Row {row}, Col {col}: A={got_a}, expected >200");
        }
    }
}

/// Test I420→RGBA8 AVX2 kernel correctness with a multi-row buffer wide
/// enough to exercise the 8-pixel AVX2 path plus scalar remainder.
/// Verifies the OOB-safe scalar chroma reads produce identical output to
/// the scalar reference for every pixel.
#[test]
fn test_i420_to_rgba8_avx2_wide_multirow() {
    // 24 pixels wide = 3 AVX2 iterations (8px each) with 0 remainder.
    // 4 rows to exercise multi-row chroma subsampling.
    let width: u32 = 24;
    let height: u32 = 4;
    let w = width as usize;
    let h = height as usize;
    let chroma_w = w / 2;

    // Build a varied I420 test pattern.
    let mut i420_data = vec![0u8; w * h + 2 * chroma_w * (h / 2)];
    // Y plane: gradient across rows and columns.
    for row in 0..h {
        for col in 0..w {
            i420_data[row * w + col] = (16 + ((row * w + col) * 219) / (w * h)) as u8;
        }
    }
    // U/V planes: varying chroma values.
    let u_offset = w * h;
    let v_offset = u_offset + chroma_w * (h / 2);
    for i in 0..chroma_w * (h / 2) {
        i420_data[u_offset + i] = (64 + (i * 3) % 192) as u8;
        i420_data[v_offset + i] = (32 + (i * 7) % 224) as u8;
    }

    // Convert using the public function (dispatches to AVX2 on this machine).
    let mut simd_out = vec![0u8; w * h * 4];
    pixel_ops::i420_to_rgba8_buf(&i420_data, width, height, &mut simd_out);

    // Compare every pixel against the scalar reference.
    for row in 0..h {
        for col in 0..w {
            let luma = i420_data[row * w + col];
            let chroma_r = row / 2;
            let chroma_c = col / 2;
            let u_val = i420_data[u_offset + chroma_r * chroma_w + chroma_c];
            let v_val = i420_data[v_offset + chroma_r * chroma_w + chroma_c];
            let expected = scalar_i420_to_rgba8(luma, u_val, v_val);
            let got_idx = (row * w + col) * 4;
            let got = &simd_out[got_idx..got_idx + 4];
            assert_eq!(
                got, &expected,
                "row={row} col={col}: Y={luma} U={u_val} V={v_val} → expected {expected:?}, got {got:?}"
            );
        }
    }
}

/// Test that opacity < 1.0 through `composite_frame` produces correct
/// output with no black borders when source matches canvas dimensions.
#[test]
fn test_composite_frame_opacity_no_black_borders() {
    let w = 32u32;
    let h = 32u32;
    let frame = make_rgba_frame(w, h, 200, 100, 50, 255);

    let layer = LayerSnapshot {
        data: frame.data,
        width: w,
        height: h,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: w, height: h }),
        opacity: 0.8,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(w, h, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Every row should have non-zero content (no black borders).
    for row in 0..h as usize {
        let row_start = row * w as usize * 4;
        let row_end = row_start + w as usize * 4;
        let any_nonzero = buf[row_start..row_end].iter().any(|&b| b != 0);
        assert!(any_nonzero, "Row {row} is all zeros — black border detected");
    }
}

/// Full-pipeline test at real dimensions (640×480): compositor blit with
/// opacity < 1.0, then RGBA→NV12→RGBA roundtrip, checking for black bands.
/// This exercises the exact pipeline the VP9 encoder sees.
#[test]
#[allow(clippy::many_single_char_names)] // Standard image-processing shorthand (w, h, r, g, b, etc.)
fn test_full_pipeline_opacity_nv12_roundtrip_no_black_bands() {
    let w = 640u32;
    let h = 480u32;
    let wu = w as usize;
    let hu = h as usize;

    // Create a colorbars-like pattern: 7 vertical bars of different colors.
    let colors: [(u8, u8, u8); 7] = [
        (255, 255, 255), // white
        (255, 255, 0),   // yellow
        (0, 255, 255),   // cyan
        (0, 255, 0),     // green
        (255, 0, 255),   // magenta
        (255, 0, 0),     // red
        (0, 0, 255),     // blue
    ];
    let mut src_rgba = vec![0u8; wu * hu * 4];
    for row in 0..hu {
        for col in 0..wu {
            let bar_idx = (col * 7) / wu;
            let (r, g, b) = colors[bar_idx];
            let off = (row * wu + col) * 4;
            src_rgba[off] = r;
            src_rgba[off + 1] = g;
            src_rgba[off + 2] = b;
            src_rgba[off + 3] = 255;
        }
    }

    // Step 1: Blit onto canvas with opacity 0.9 (through scale_blit_rgba_rotated,
    // exactly as the compositor does).
    let mut canvas = vec![0u8; wu * hu * 4];
    pixel_ops::scale_blit_rgba_rotated(
        &mut canvas,
        w,
        h,
        &src_rgba,
        w,
        h,
        &BlitRect { x: 0, y: 0, width: w, height: h },
        0.9,
        0.0,
        false,
        false,
        false,
        None,
        false,
    );

    // Verify compositor output: every row should have non-zero pixels.
    for row in 0..hu {
        let row_start = row * wu * 4;
        let any_nonzero = canvas[row_start..row_start + wu * 4].iter().any(|&b| b != 0);
        assert!(any_nonzero, "Compositor output row {row} is all zeros (black band)");
    }

    // Step 2: Convert RGBA → NV12 (exactly as the VP9 encoder does).
    let chroma_w = wu.div_ceil(2);
    let chroma_h = hu.div_ceil(2);
    let nv12_size = wu * hu + chroma_w * 2 * chroma_h;
    let mut nv12 = vec![0u8; nv12_size];
    pixel_ops::rgba8_to_nv12_buf(&canvas, w, h, &mut nv12);

    // Verify Y plane: no rows should be all-zero (Y=0 is below black level).
    // With opacity 0.9 on colored bars, Y values should be well above 0.
    for row in 0..hu {
        let y_row = &nv12[row * wu..(row + 1) * wu];
        let max_y = *y_row.iter().max().unwrap();
        assert!(max_y > 16, "NV12 Y-plane row {row}: max Y={max_y}, expected >16 (not black)");
    }

    // Step 3: Convert NV12 → RGBA (simulates decoder display).
    let mut decoded_rgba = vec![0u8; wu * hu * 4];
    pixel_ops::nv12_to_rgba8_buf(&nv12, w, h, &mut decoded_rgba);

    // Verify decoded output: every row should have non-black pixels.
    for row in 0..hu {
        let row_start = row * wu * 4;
        let row_slice = &decoded_rgba[row_start..row_start + wu * 4];
        // Check that at least some pixels have R, G, or B > 10 (not near-black).
        let has_visible =
            row_slice.chunks_exact(4).any(|px| px[0] > 10 || px[1] > 10 || px[2] > 10);
        assert!(has_visible, "Decoded row {row} has no visible pixels (all near-black)");
    }
}

/// Regression test: a 4:3 source blitted onto a 16:9 canvas with opacity < 1.0
/// must cover the entire canvas (stretch-to-fill) with no black bars.
/// Previously the near-zero rotation fast path applied an aspect-ratio-preserving
/// fit that left letterbox gaps visible as black bands when opacity < 1.0.
#[test]
fn test_mismatched_aspect_ratio_opacity_no_black_bars() {
    let src_w = 640u32;
    let src_h = 480u32; // 4:3
    let canvas_w = 1280u32;
    let canvas_h = 720u32; // 16:9

    // Solid green source.
    let src = [0u8, 255, 0, 255].repeat((src_w * src_h) as usize);
    let mut canvas = vec![0u8; (canvas_w * canvas_h * 4) as usize];

    pixel_ops::scale_blit_rgba_rotated(
        &mut canvas,
        canvas_w,
        canvas_h,
        &src,
        src_w,
        src_h,
        &BlitRect { x: 0, y: 0, width: canvas_w, height: canvas_h },
        0.9,
        0.0, // no rotation — exercises the near-zero fast path
        false,
        false,
        false,
        None,
        false,
    );

    // Every row should have non-zero pixels (no black bars on left/right).
    for row in 0..canvas_h as usize {
        let row_start = row * canvas_w as usize * 4;
        let row_end = row_start + canvas_w as usize * 4;
        let any_nonzero = canvas[row_start..row_end].iter().any(|&b| b != 0);
        assert!(any_nonzero, "Row {row} is all zeros — black bar detected");
    }

    // Every column should have non-zero pixels (no black bars on top/bottom).
    for col in 0..canvas_w as usize {
        let any_nonzero = (0..canvas_h as usize).any(|row| {
            let idx = (row * canvas_w as usize + col) * 4;
            canvas[idx] != 0 || canvas[idx + 1] != 0 || canvas[idx + 2] != 0
        });
        assert!(any_nonzero, "Column {col} is all zeros — black bar detected");
    }
}

/// Regression test: a 4:3 source blitted into a non-square rect with 15°
/// rotation must cover the centre of the rect (stretch-to-fill, not
/// aspect-ratio fit).  Exercises the rotated path's per-axis inverse
/// scaling (`inv_scale_x` / `inv_scale_y`).
#[test]
fn test_rotated_blit_mismatched_aspect_ratio_covers_centre() {
    // 4×2 red source into a 40×20 rect (2:1 aspect mismatch) at 15° on
    // a 60×40 canvas.  The centre of the rect (canvas pixel 30,20) must
    // be covered by red source content.
    let src = [255u8, 0, 0, 255].repeat(4 * 2); // 4×2 solid red
    let mut dst = vec![0u8; 60 * 40 * 4];

    scale_blit_rgba_rotated(
        &mut dst,
        60,
        40,
        &src,
        4,
        2,
        &BlitRect { x: 10, y: 10, width: 40, height: 20 },
        1.0,
        15.0,
        false,
        false,
        false,
        None,
        false,
    );

    // Centre of the rect (canvas pixel 30, 20) should be red.
    let cx = 30usize;
    let cy = 20usize;
    let idx = (cy * 60 + cx) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert_eq!(dst[idx + 1], 0, "Centre G");
    assert_eq!(dst[idx + 2], 0, "Centre B");
    assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");
}

/// Test RGBA→NV12 AVX2 chroma conversion matches scalar reference.
/// Uses a 640-wide frame to fully exercise the AVX2 path (8 chroma samples/iter).
#[test]
#[allow(clippy::many_single_char_names)] // Standard image-processing shorthand (w, h, r, g, b, etc.)
fn test_rgba8_to_nv12_avx2_chroma_matches_scalar() {
    let w = 640u32;
    let h = 4u32;
    let wu = w as usize;
    let hu = h as usize;
    let chroma_w = wu / 2;
    let chroma_h = hu / 2;

    // Create a varied RGBA pattern.
    let mut rgba = vec![0u8; wu * hu * 4];
    for row in 0..hu {
        for col in 0..wu {
            let off = (row * wu + col) * 4;
            rgba[off] = ((col * 3 + row * 7) % 256) as u8; // R
            rgba[off + 1] = ((col * 5 + row * 11) % 256) as u8; // G
            rgba[off + 2] = ((col * 7 + row * 13) % 256) as u8; // B
            rgba[off + 3] = 255; // A
        }
    }

    // Convert using the public function (dispatches to AVX2).
    let nv12_size = wu * hu + chroma_w * 2 * chroma_h;
    let mut nv12_simd = vec![0u8; nv12_size];
    pixel_ops::rgba8_to_nv12_buf(&rgba, w, h, &mut nv12_simd);

    // Compute scalar reference for the chroma plane.
    let y_size = wu * hu;
    for crow in 0..chroma_h {
        let r0 = crow * 2;
        for ccol in 0..chroma_w {
            let c0 = ccol * 2;
            let mut sr = 0i32;
            let mut sg = 0i32;
            let mut sb = 0i32;
            let mut count = 0i32;
            for dr in 0..2u32 {
                let rr = r0 + dr as usize;
                if rr >= hu {
                    continue;
                }
                for dc in 0..2u32 {
                    let cc = c0 + dc as usize;
                    if cc < wu {
                        let off = (rr * wu + cc) * 4;
                        sr += i32::from(rgba[off]);
                        sg += i32::from(rgba[off + 1]);
                        sb += i32::from(rgba[off + 2]);
                        count += 1;
                    }
                }
            }
            let r_avg = sr / count;
            let g_avg = sg / count;
            let b_avg = sb / count;
            let expected_u = ((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128;
            let expected_v = ((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128;
            let expected_u = expected_u.clamp(0, 255) as u8;
            let expected_v = expected_v.clamp(0, 255) as u8;

            let uv_off = y_size + crow * chroma_w * 2 + ccol * 2;
            let got_u = nv12_simd[uv_off];
            let got_v = nv12_simd[uv_off + 1];

            // Allow ±2 for rounding differences between SIMD and scalar.
            assert!(
                (i16::from(got_u) - i16::from(expected_u)).abs() <= 2,
                "crow={crow} ccol={ccol}: U got={got_u}, expected={expected_u}"
            );
            assert!(
                (i16::from(got_v) - i16::from(expected_v)).abs() <= 2,
                "crow={crow} ccol={ccol}: V got={got_v}, expected={expected_v}"
            );
        }
    }

    // Also verify Y plane matches scalar reference.
    for row in 0..hu {
        for col in 0..wu {
            let off = (row * wu + col) * 4;
            let r = i32::from(rgba[off]);
            let g = i32::from(rgba[off + 1]);
            let b = i32::from(rgba[off + 2]);
            let expected_y = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
            let got_y = nv12_simd[row * wu + col];
            assert!(
                (i16::from(got_y) - i16::from(expected_y)).abs() <= 1,
                "row={row} col={col}: Y got={got_y}, expected={expected_y}"
            );
        }
    }
}

// ── Crop / zoom unit tests ──────────────────────────────────────────

/// Create a 4×4 RGBA8 frame with four distinct colour quadrants:
///   TL = red, TR = green, BL = blue, BR = white.
fn make_quadrant_frame() -> VideoFrame {
    let mut data = vec![0u8; 4 * 4 * 4];
    for y in 0..4u32 {
        for x in 0..4u32 {
            let idx = ((y * 4 + x) * 4) as usize;
            let (r, g, b) = match (x < 2, y < 2) {
                (true, true) => (255, 0, 0),       // top-left: red
                (false, true) => (0, 255, 0),      // top-right: green
                (true, false) => (0, 0, 255),      // bottom-left: blue
                (false, false) => (255, 255, 255), // bottom-right: white
            };
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = 255;
        }
    }
    VideoFrame::new(4, 4, PixelFormat::Rgba8, data).unwrap()
}

#[test]
fn test_composite_frame_with_crop_zoom() {
    // 2× zoom centred on top-left quadrant (crop_x=0.0, crop_y=0.0)
    // should show only the red quadrant, scaled to fill a 4×4 canvas.
    let frame = make_quadrant_frame();
    let layer = LayerSnapshot {
        data: frame.data,
        width: 4,
        height: 4,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 0.0,
        crop_y: 0.0,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Every pixel on the canvas should be red (from the TL quadrant).
    for (i, pixel) in buf.chunks_exact(4).enumerate() {
        assert_eq!(pixel[0], 255, "pixel {i} R");
        assert_eq!(pixel[1], 0, "pixel {i} G");
        assert_eq!(pixel[2], 0, "pixel {i} B");
        assert_eq!(pixel[3], 255, "pixel {i} A");
    }
}

#[test]
fn test_crop_pan_right_edge() {
    // 2× zoom with crop_x=1.0 → shows top-right quadrant (green).
    let frame = make_quadrant_frame();
    let layer = LayerSnapshot {
        data: frame.data,
        width: 4,
        height: 4,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 1.0,
        crop_y: 0.0,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    for (i, pixel) in buf.chunks_exact(4).enumerate() {
        assert_eq!(pixel[0], 0, "pixel {i} R");
        assert_eq!(pixel[1], 255, "pixel {i} G");
        assert_eq!(pixel[2], 0, "pixel {i} B");
    }
}

#[test]
fn test_crop_tilt_bottom() {
    // 2× zoom with crop_y=1.0, crop_x=0.0 → shows bottom-left quadrant (blue).
    let frame = make_quadrant_frame();
    let layer = LayerSnapshot {
        data: frame.data,
        width: 4,
        height: 4,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 0.0,
        crop_y: 1.0,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    for (i, pixel) in buf.chunks_exact(4).enumerate() {
        assert_eq!(pixel[0], 0, "pixel {i} R");
        assert_eq!(pixel[1], 0, "pixel {i} G");
        assert_eq!(pixel[2], 255, "pixel {i} B");
    }
}

#[test]
fn test_crop_no_zoom_returns_full_frame() {
    // crop_zoom=1.0 should show the entire source unchanged.
    let frame = make_quadrant_frame();
    let layer = LayerSnapshot {
        data: frame.data,
        width: 4,
        height: 4,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 4, height: 4 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.0,
        crop_y: 0.0,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(4, 4, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // (0,0) = red
    assert_eq!(&buf[0..4], &[255, 0, 0, 255]);
    // (3,0) = green
    let idx = 3 * 4;
    assert_eq!(&buf[idx..idx + 4], &[0, 255, 0, 255]);
    // (0,3) = blue
    let idx = (3 * 4) * 4;
    assert_eq!(&buf[idx..idx + 4], &[0, 0, 255, 255]);
    // (3,3) = white
    let idx = (3 * 4 + 3) * 4;
    assert_eq!(&buf[idx..idx + 4], &[255, 255, 255, 255]);
}

#[test]
fn test_crop_validation() {
    // crop_zoom < 1.0 should fail validation.
    let mut cfg = CompositorConfig::default();
    cfg.layers.insert("in_0".to_string(), LayerConfig { crop_zoom: 0.5, ..Default::default() });
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_err());

    // crop_x out of range should fail.
    let mut cfg = CompositorConfig::default();
    cfg.layers.insert("in_0".to_string(), LayerConfig { crop_x: 1.5, ..Default::default() });
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_err());

    // crop_y out of range should fail.
    let mut cfg = CompositorConfig::default();
    cfg.layers.insert("in_0".to_string(), LayerConfig { crop_y: -0.1, ..Default::default() });
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_err());

    // Valid crop values should pass.
    let mut cfg = CompositorConfig::default();
    cfg.layers.insert(
        "in_0".to_string(),
        LayerConfig { crop_zoom: 2.0, crop_x: 0.3, crop_y: 0.7, ..Default::default() },
    );
    assert!(cfg.validate(&GlobalCompositorConfig::default()).is_ok());
}

#[test]
fn test_scale_blit_with_src_region() {
    // 4×4 quadrant source, blit with a crop region selecting only
    // the top-left 2×2 (red quadrant) onto a 4×4 canvas.
    let src = make_quadrant_frame();
    let src_data = src.data();
    let mut dst = vec![0u8; 4 * 4 * 4];

    scale_blit_rgba(
        &mut dst,
        4,
        4,
        src_data,
        4,
        4,
        &BlitRect { x: 0, y: 0, width: 4, height: 4 },
        1.0,
        false,
        false,
        false,
        Some((0, 0, 2, 2)), // crop to top-left 2×2
        false,
    );

    // All pixels should be red (sampled from the TL quadrant).
    for (i, pixel) in dst.chunks_exact(4).enumerate() {
        assert_eq!(pixel[0], 255, "pixel {i} R");
        assert_eq!(pixel[1], 0, "pixel {i} G");
        assert_eq!(pixel[2], 0, "pixel {i} B");
    }
}

#[test]
fn test_scale_blit_rotated_with_src_region() {
    // 4×4 source, crop to top-left 2×2 (red), rotated 0° → centre
    // of the destination rect should be red.
    let src = make_quadrant_frame();
    let src_data = src.data();
    let mut dst = vec![0u8; 10 * 10 * 4]; // 10×10 canvas

    scale_blit_rgba_rotated(
        &mut dst,
        10,
        10,
        src_data,
        4,
        4,
        &BlitRect { x: 0, y: 0, width: 10, height: 10 },
        1.0,
        0.0,
        false,
        false,
        false,
        Some((0, 0, 2, 2)),
        false,
    );

    // Centre pixel (5,5) should be red.
    let cx = 5usize;
    let cy = 5usize;
    let idx = (cy * 10 + cx) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert_eq!(dst[idx + 1], 0, "Centre G");
    assert_eq!(dst[idx + 2], 0, "Centre B");
}

#[test]
fn test_crop_with_rotation_and_mirror() {
    // 4×4 quadrant source, crop to top-right 2×2 (green) with mirror_h
    // and a small rotation.  The mirror should stay within the crop region.
    // With mirror_h, the green quadrant's pixels are reflected horizontally
    // within the crop window (still green).  The 15° rotation exercises
    // the rotated blit path (rotation >= 0.01°).
    let src = make_quadrant_frame();
    let src_data = src.data();
    let mut dst = vec![0u8; 10 * 10 * 4]; // 10×10 canvas

    scale_blit_rgba_rotated(
        &mut dst,
        10,
        10,
        src_data,
        4,
        4,
        &BlitRect { x: 0, y: 0, width: 10, height: 10 },
        1.0,
        15.0,               // non-zero rotation to use rotated path
        false,              // skip_transparent
        true,               // mirror_h
        false,              // mirror_v
        Some((2, 0, 2, 2)), // top-right quadrant (green)
        false,
    );

    // Centre pixel should be green (mirrored green is still green).
    let cx = 5usize;
    let cy = 5usize;
    let idx = (cy * 10 + cx) * 4;
    assert_eq!(dst[idx], 0, "Centre R should be 0 (green quad)");
    assert_eq!(dst[idx + 1], 255, "Centre G should be 255 (green quad)");
    assert_eq!(dst[idx + 2], 0, "Centre B should be 0 (green quad)");
}

#[test]
fn test_crop_with_mirror_v_rotated() {
    // 4×4 quadrant source, crop to bottom-left 2×2 (blue) with mirror_v
    // and rotation.  mirror_v within the blue crop region → still blue.
    let src = make_quadrant_frame();
    let src_data = src.data();
    let mut dst = vec![0u8; 10 * 10 * 4];

    scale_blit_rgba_rotated(
        &mut dst,
        10,
        10,
        src_data,
        4,
        4,
        &BlitRect { x: 0, y: 0, width: 10, height: 10 },
        1.0,
        10.0, // non-zero rotation
        false,
        false,
        true,               // mirror_v
        Some((0, 2, 2, 2)), // bottom-left quadrant (blue)
        false,
    );

    let cx = 5usize;
    let cy = 5usize;
    let idx = (cy * 10 + cx) * 4;
    assert_eq!(dst[idx], 0, "Centre R should be 0 (blue quad)");
    assert_eq!(dst[idx + 1], 0, "Centre G should be 0 (blue quad)");
    assert_eq!(dst[idx + 2], 255, "Centre B should be 255 (blue quad)");
}

// ── Chroma subsampling alignment tests ──────────────────────────────

/// Create a solid-colour I420 `VideoFrame`.
///
/// Converts a solid RGBA colour to I420 via `rgba8_to_i420_buf` so the
/// test exercises the real conversion path.
fn make_i420_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> VideoFrame {
    let rgba = make_rgba_frame(width, height, r, g, b, 255);
    let luma_w = width as usize;
    let luma_h = height as usize;
    let chroma_w = luma_w.div_ceil(2);
    let chroma_h = luma_h.div_ceil(2);
    let i420_size = luma_w * luma_h + 2 * chroma_w * chroma_h;
    let mut i420_data = vec![0u8; i420_size];
    pixel_ops::rgba8_to_i420_buf(rgba.data(), width, height, &mut i420_data);
    VideoFrame::new(width, height, PixelFormat::I420, i420_data).unwrap()
}

/// Create a solid-colour NV12 `VideoFrame`.
fn make_nv12_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> VideoFrame {
    let rgba = make_rgba_frame(width, height, r, g, b, 255);
    let luma_w = width as usize;
    let luma_h = height as usize;
    let chroma_w = luma_w.div_ceil(2);
    let chroma_h = luma_h.div_ceil(2);
    let nv12_size = luma_w * luma_h + chroma_w * 2 * chroma_h;
    let mut nv12_data = vec![0u8; nv12_size];
    pixel_ops::rgba8_to_nv12_buf(rgba.data(), width, height, &mut nv12_data);
    VideoFrame::new(width, height, PixelFormat::Nv12, nv12_data).unwrap()
}

#[test]
fn test_crop_aligns_odd_origin_for_i420_composite() {
    // 8×8 solid-red I420 source with crop parameters that would
    // normally produce an odd crop origin.
    //
    // 2× zoom → crop region 4×4.  max_x = max_y = 4.
    // crop_x = 0.75 → raw x = round(0.75 * 4) = 3 (odd).
    // crop_y = 0.75 → raw y = round(0.75 * 4) = 3 (odd).
    //
    // After chroma alignment the origin should snap to (2, 2).
    // Because the source is solid red, the output canvas should
    // consist entirely of (near-)red pixels — any chroma misalignment
    // would shift colours noticeably.
    let frame = make_i420_frame(8, 8, 255, 0, 0);
    let layer = LayerSnapshot {
        data: frame.data,
        width: 8,
        height: 8,
        pixel_format: PixelFormat::I420,
        rect: Some(Rect { x: 0, y: 0, width: 8, height: 8 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 0.75,
        crop_y: 0.75,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(8, 8, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Every pixel should be close to red (BT.601 round-trip tolerance).
    for (i, pixel) in buf.chunks_exact(4).enumerate() {
        assert!(pixel[0] > 200, "pixel {i}: R={}, expected >200", pixel[0]);
        assert!(pixel[1] < 30, "pixel {i}: G={}, expected <30", pixel[1]);
        assert!(pixel[2] < 30, "pixel {i}: B={}, expected <30", pixel[2]);
    }
}

#[test]
fn test_crop_aligns_odd_origin_for_nv12_composite() {
    // Same test as above but with an NV12 source.
    let frame = make_nv12_frame(8, 8, 255, 0, 0);
    let layer = LayerSnapshot {
        data: frame.data,
        width: 8,
        height: 8,
        pixel_format: PixelFormat::Nv12,
        rect: Some(Rect { x: 0, y: 0, width: 8, height: 8 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 0.75,
        crop_y: 0.75,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(8, 8, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    for (i, pixel) in buf.chunks_exact(4).enumerate() {
        assert!(pixel[0] > 200, "pixel {i}: R={}, expected >200", pixel[0]);
        assert!(pixel[1] < 30, "pixel {i}: G={}, expected <30", pixel[1]);
        assert!(pixel[2] < 30, "pixel {i}: B={}, expected <30", pixel[2]);
    }
}

#[test]
fn test_crop_aligns_odd_origin_for_i420_rotated() {
    // Exercise the rotated blit path (rotation > 0.01°) with an I420
    // source and crop parameters that produce odd origins.  The centre
    // pixel of the output should still be red.
    let frame = make_i420_frame(8, 8, 255, 0, 0);
    let layer = LayerSnapshot {
        data: frame.data,
        width: 8,
        height: 8,
        pixel_format: PixelFormat::I420,
        rect: Some(Rect { x: 0, y: 0, width: 16, height: 16 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 15.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 2.0,
        crop_x: 0.75,
        crop_y: 0.75,
        crop_shape: config::CropShape::Rect,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(16, 16, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Centre pixel (8, 8) should be close to red.
    let cx = 8usize;
    let cy = 8usize;
    let idx = (cy * 16 + cx) * 4;
    assert!(buf[idx] > 200, "Centre R={}, expected >200", buf[idx]);
    assert!(buf[idx + 1] < 30, "Centre G={}, expected <30", buf[idx + 1]);
    assert!(buf[idx + 2] < 30, "Centre B={}, expected <30", buf[idx + 2]);
}

// ── Oneshot / batch mode regression tests ───────────────────────────

/// Regression test: in oneshot mode, sending N frames in a burst (the way
/// batch colorbars do) must produce exactly N output frames.
///
/// Before the fix, the compositor's drain-to-latest logic discarded all
/// but the last frame per tick, reducing 20 input frames to just a
/// handful of outputs.
#[tokio::test]
async fn test_oneshot_batch_frame_preservation() {
    let frame_count: usize = 20;

    let (input_tx, input_rx) = mpsc::channel(256);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_oneshot_test_context(inputs, 256);

    let config = CompositorConfig { width: 4, height: 4, ..Default::default() };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Dump all frames at once — no sleep between sends.
    for _ in 0..frame_count {
        let frame = make_rgba_frame(4, 4, 255, 0, 0, 255);
        input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert_eq!(
        output_packets.len(),
        frame_count,
        "Expected exactly {frame_count} output frames in oneshot mode, got {}",
        output_packets.len(),
    );
}

/// Regression test: compositor output timestamps must be monotonically
/// increasing at exact `1_000_000 / fps` microsecond intervals,
/// regardless of input frame timestamps.
///
/// Before the fix, the compositor copied timestamps from input frames.
/// In batch mode this produced erratic gaps; in any mode it was wrong
/// because input timestamps don't reflect the compositor's output cadence.
#[tokio::test]
async fn test_oneshot_output_timestamps_monotonic() {
    let frame_count: usize = 10;
    let fps: u32 = 30;
    let expected_duration_us: u64 = 1_000_000 / u64::from(fps);

    let (input_tx, input_rx) = mpsc::channel(256);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_oneshot_test_context(inputs, 256);

    let config = CompositorConfig { width: 4, height: 4, fps, ..Default::default() };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send frames with deliberately wrong timestamps to prove the
    // compositor ignores them.
    for i in 0..frame_count {
        let mut frame = make_rgba_frame(4, 4, 0, 255, 0, 255);
        frame.metadata = Some(PacketMetadata {
            timestamp_us: Some(999_999 + i as u64 * 77_777), // arbitrary junk
            duration_us: Some(11_111),
            sequence: Some(i as u64 + 100),
            keyframe: None,
        });
        input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert_eq!(output_packets.len(), frame_count);

    for (i, pkt) in output_packets.iter().enumerate() {
        if let Packet::Video(ref f) = pkt {
            let meta = f.metadata.as_ref().expect("metadata should be present");
            let expected_ts = i as u64 * expected_duration_us;
            assert_eq!(
                meta.timestamp_us,
                Some(expected_ts),
                "frame {i}: expected timestamp_us={expected_ts}, got {:?}",
                meta.timestamp_us,
            );
            assert_eq!(
                meta.duration_us,
                Some(expected_duration_us),
                "frame {i}: expected duration_us={expected_duration_us}, got {:?}",
                meta.duration_us,
            );
            assert_eq!(meta.sequence, Some(i as u64));
        } else {
            panic!("Expected video packet at index {i}");
        }
    }
}

/// Regression test: in oneshot mode the compositor must not be limited
/// by real-time tick pacing — processing N frames should complete in
/// significantly less than N / fps wall-clock seconds.
///
/// Before the fix, the compositor used `tokio::time::interval(1s / fps)`
/// even in batch mode, capping throughput at wall-clock fps.
#[tokio::test]
async fn test_oneshot_processes_faster_than_realtime() {
    let frame_count: usize = 10;
    let fps: u32 = 5;
    // At real-time pacing, 10 frames at 5 fps = 2 seconds.
    // Without pacing the tiny 4×4 frames should finish well under 1.5s
    // even on a loaded CI runner.  The previous 30@30 (budget 500ms)
    // flaked on the GPU runner because per-frame scheduling overhead
    // (~30ms) nearly matched the pacing interval (33ms).
    let max_allowed = std::time::Duration::from_millis(1500);

    let (input_tx, input_rx) = mpsc::channel(256);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_oneshot_test_context(inputs, 256);

    let config = CompositorConfig { width: 4, height: 4, fps, ..Default::default() };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    let start = std::time::Instant::now();

    for _ in 0..frame_count {
        let frame = make_rgba_frame(4, 4, 0, 0, 255, 255);
        input_tx.send(Packet::Video(frame)).await.unwrap();
    }
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let elapsed = start.elapsed();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert_eq!(output_packets.len(), frame_count);

    assert!(
        elapsed < max_allowed,
        "Oneshot compositor took {elapsed:?} for {frame_count} frames at {fps} fps — \
         expected < {max_allowed:?} (should not be real-time paced).  \
         If this is close to {} ms (real-time pace), the oneshot tick \
         path may have regressed to interval-based pacing.",
        frame_count as u64 * 1000 / u64::from(fps),
    );
}

#[test]
fn test_scale_blit_crop_shape_circle_masks_corners() {
    // Blit a solid red 20×20 source onto a 20×20 canvas with crop_shape: circle
    // enabled.  The centre pixel must be red (inside the circle) and the
    // corner pixel (0,0) must remain transparent (outside the circle).
    let src = [255u8, 0, 0, 255].repeat(20 * 20);
    let mut dst = vec![0u8; 20 * 20 * 4];

    scale_blit_rgba(
        &mut dst,
        20,
        20,
        &src,
        20,
        20,
        &BlitRect { x: 0, y: 0, width: 20, height: 20 },
        1.0,
        false,
        false,
        false,
        None,
        true, // crop_shape = circle
    );

    // Centre pixel (10,10) should be red — well inside the circle.
    let cx = 10usize;
    let cy = 10usize;
    let idx = (cy * 20 + cx) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert_eq!(dst[idx + 1], 0, "Centre G");
    assert_eq!(dst[idx + 2], 0, "Centre B");
    assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");

    // Corner pixel (0,0) is well outside the inscribed circle and should
    // remain transparent black.
    assert_eq!(dst[0], 0, "Corner (0,0) R should be 0");
    assert_eq!(dst[3], 0, "Corner (0,0) A should be 0");

    // Corner pixel (19,19) should also be transparent.
    let corner_idx = (19 * 20 + 19) * 4;
    assert_eq!(dst[corner_idx + 3], 0, "Corner (19,19) A should be 0");
}

#[test]
fn test_scale_blit_rotated_crop_shape_circle_masks_corners() {
    // Same test but through the rotated blit path (with a small rotation).
    let src = [255u8, 0, 0, 255].repeat(20 * 20);
    let mut dst = vec![0u8; 40 * 40 * 4];

    scale_blit_rgba_rotated(
        &mut dst,
        40,
        40,
        &src,
        20,
        20,
        &BlitRect { x: 10, y: 10, width: 20, height: 20 },
        1.0,
        5.0, // small rotation to use the rotated path
        false,
        false,
        false,
        None,
        true, // crop_shape = circle
    );

    // Centre of the rect (canvas pixel 20,20) should be red.
    let cx = 20usize;
    let cy = 20usize;
    let idx = (cy * 40 + cx) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert_eq!(dst[idx + 1], 0, "Centre G");
    assert_eq!(dst[idx + 2], 0, "Centre B");
    assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");

    // The rect corner (10,10) is outside both the rotated rect and the
    // ellipse — should be transparent.
    let corner_idx = (10usize * 40 + 10) * 4;
    assert_eq!(dst[corner_idx + 3], 0, "Rect corner should be transparent");
}

#[test]
fn test_composite_frame_crop_shape_circle() {
    // A 20×20 red layer with crop_shape: circle on a 20×20 canvas.
    // Centre should be red, corners should be transparent black.
    let frame = make_rgba_frame(20, 20, 255, 0, 0, 255);
    let layer = LayerSnapshot {
        data: frame.data,
        width: 20,
        height: 20,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: 20, height: 20 }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Circle,
    };

    let mut cache = ConversionCache::new();
    let result = composite_frame(20, 20, &[Some(layer)], &[], &[], None, &mut cache);
    let buf = result.as_slice();

    // Centre pixel (10,10) should be red.
    let cx = 10usize;
    let cy = 10usize;
    let idx = (cy * 20 + cx) * 4;
    assert_eq!(buf[idx], 255, "Centre R");
    assert_eq!(buf[idx + 1], 0, "Centre G");
    assert_eq!(buf[idx + 2], 0, "Centre B");
    assert!(buf[idx + 3] > 200, "Centre A");

    // Corner (0,0) should be transparent black.
    assert_eq!(buf[0], 0, "Corner R");
    assert_eq!(buf[3], 0, "Corner A");
}

#[test]
fn test_scale_blit_crop_shape_circle_nonsquare_rect() {
    // Verify that crop_shape: circle on a non-square destination rect
    // produces a true circle (using min dimension as diameter), NOT an
    // ellipse.  A 40×20 rect should crop to a circle of diameter 20
    // centred in the rect.  A pixel at (5, 10) — inside an ellipse but
    // outside the true circle — must remain transparent.
    let src = [255u8, 0, 0, 255].repeat(40 * 20);
    let mut dst = vec![0u8; 40 * 20 * 4];

    scale_blit_rgba(
        &mut dst,
        40,
        20,
        &src,
        40,
        20,
        &BlitRect { x: 0, y: 0, width: 40, height: 20 },
        1.0,
        false,
        false,
        false,
        None,
        true, // crop_shape = circle
    );

    // Centre pixel (20, 10) should be red — inside the circle.
    let idx = (10 * 40 + 20) * 4;
    assert_eq!(dst[idx], 255, "Centre R");
    assert!(dst[idx + 3] > 200, "Centre A should be mostly opaque");

    // Pixel (5, 10) — on the horizontal midline but well outside a
    // circle of radius 10 centred at (20, 10).  Distance from centre
    // is 15, which is > 10.  An ellipse would include this point.
    let outside_idx = (10 * 40 + 5) * 4;
    assert_eq!(dst[outside_idx + 3], 0, "Pixel (5,10) should be transparent — outside true circle");

    // Pixel (30, 10) — symmetric to (5,10) on the other side.
    let outside_idx2 = (10 * 40 + 35) * 4;
    assert_eq!(
        dst[outside_idx2 + 3],
        0,
        "Pixel (35,10) should be transparent — outside true circle"
    );
}

#[test]
fn test_composite_frame_crop_shape_circle_skip_clear() {
    // Regression test: when crop_shape is circle on the first (full-canvas)
    // layer, the skip_clear optimisation must NOT fire — pixels outside the
    // ellipse must be cleared to transparent black.  With a pooled buffer
    // the recycled memory may contain stale data from a prior frame; if the
    // canvas isn't cleared those stale pixels would leak through.
    //
    // We simulate this by using a VideoFramePool, filling the recycled
    // buffer with garbage, then compositing a crop_shape=circle layer and
    // asserting that the corner pixel is zeroed.
    use streamkit_core::VideoFramePool;

    let pool = VideoFramePool::video_default();
    let canvas_w: u32 = 20;
    let canvas_h: u32 = 20;
    let total_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;

    // Prime the pool: get a buffer, fill it with non-zero garbage, then
    // drop it so it returns to the pool.
    {
        let mut primer = pool.get(total_bytes);
        primer.as_mut_slice().fill(0xAB);
    }

    // Now composite_frame will recycle that buffer (pool hit).
    let frame = make_rgba_frame(canvas_w, canvas_h, 255, 0, 0, 255);
    let layer = LayerSnapshot {
        data: frame.data,
        width: canvas_w,
        height: canvas_h,
        pixel_format: PixelFormat::Rgba8,
        rect: Some(Rect { x: 0, y: 0, width: canvas_w, height: canvas_h }),
        opacity: 1.0,
        z_index: 0,
        rotation_degrees: 0.0,
        mirror_horizontal: false,
        mirror_vertical: false,
        crop_zoom: 1.0,
        crop_x: 0.5,
        crop_y: 0.5,
        crop_shape: config::CropShape::Circle,
    };

    let mut cache = ConversionCache::new();
    let result =
        composite_frame(canvas_w, canvas_h, &[Some(layer)], &[], &[], Some(&pool), &mut cache);
    let buf = result.as_slice();

    // Corner (0,0) is outside the ellipse — must be transparent black,
    // NOT the 0xAB garbage from the recycled buffer.
    assert_eq!(buf[0], 0, "Corner R must be 0 (was stale pool data)");
    assert_eq!(buf[1], 0, "Corner G must be 0");
    assert_eq!(buf[2], 0, "Corner B must be 0");
    assert_eq!(buf[3], 0, "Corner A must be 0 (was stale pool data)");

    // Centre pixel should still be red.
    let cx = 10usize;
    let cy = 10usize;
    let idx = (cy * canvas_w as usize + cx) * 4;
    assert_eq!(buf[idx], 255, "Centre R");
    assert!(buf[idx + 3] > 200, "Centre A");
}

// ── resolve_scene / view-data layout tests ─────────────────────────────

/// Helper: build an `InputSlot` with the given name and optional latest frame.
fn make_slot(name: &str, frame: Option<VideoFrame>) -> InputSlot {
    let (_tx, rx) = mpsc::channel::<Packet>(1);
    InputSlot {
        name: name.to_string(),
        rx,
        latest_frame: frame,
        last_source_dims: None,
        hint_tx: None,
    }
}

#[test]
fn test_resolve_scene_explicit_rect() {
    let mut config = CompositorConfig { width: 1280, height: 720, ..Default::default() };
    config.layers.insert(
        "in_0".to_string(),
        LayerConfig {
            rect: Some(Rect { x: 100, y: 50, width: 640, height: 360 }),
            opacity: 0.5,
            z_index: 3,
            rotation_degrees: 45.0,
            ..Default::default()
        },
    );

    let slots = [make_slot("in_0", None)];
    let empty_overlays: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![]);
    let scene = resolve_scene(&slots, &config, &empty_overlays, &empty_overlays);

    // View-data layer carries only server-computed geometry.
    assert_eq!(scene.layout.layers.len(), 1);
    let rl = &scene.layout.layers[0];
    assert_eq!(rl.id, "in_0");
    assert_eq!(rl.x, 100);
    assert_eq!(rl.y, 50);
    assert_eq!(rl.width, 640);
    assert_eq!(rl.height, 360);

    // Config fields (opacity, rotation, z_index, etc.) are NOT in the view-data
    // struct — they live only in the internal ResolvedSlotConfig.
    assert!((scene.configs[0].opacity - 0.5).abs() < f32::EPSILON);
    assert_eq!(scene.configs[0].z_index, 3);
    assert!((scene.configs[0].rotation_degrees - 45.0).abs() < f32::EPSILON);
}

#[test]
fn test_resolve_scene_fullscreen_fallback() {
    // A single slot without explicit config → fullscreen.
    let config = CompositorConfig { width: 1920, height: 1080, ..Default::default() };
    let slots = [make_slot("in_0", None)];
    let empty: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![]);
    let scene = resolve_scene(&slots, &config, &empty, &empty);

    let rl = &scene.layout.layers[0];
    assert_eq!(rl.x, 0);
    assert_eq!(rl.y, 0);
    assert_eq!(rl.width, 1920);
    assert_eq!(rl.height, 1080);
}

#[test]
fn test_resolve_scene_auto_pip() {
    // Two slots, second has no config → auto-PiP (bottom-right, 1/3 canvas).
    let config = CompositorConfig { width: 1200, height: 900, ..Default::default() };
    let slots = [make_slot("in_0", None), make_slot("in_1", None)];
    let empty: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![]);
    let scene = resolve_scene(&slots, &config, &empty, &empty);

    assert_eq!(scene.layout.layers.len(), 2);
    // First slot: fullscreen
    assert_eq!(scene.layout.layers[0].width, 1200);
    // Second slot: PiP (1/3 canvas = 400×300, inset 20px from bottom-right)
    let pip = &scene.layout.layers[1];
    assert_eq!(pip.width, 400);
    assert_eq!(pip.height, 300);
    assert_eq!(pip.x, 1200 - 400 - 20);
    assert_eq!(pip.y, 900 - 300 - 20);
}

#[test]
fn test_resolve_scene_auto_pip_aspect_fit() {
    // Auto-PiP with a frame present → aspect-fit within PiP bounds.
    let config = CompositorConfig { width: 1200, height: 900, ..Default::default() };
    // 4:3 source, PiP bounds would be 400×300
    let frame = make_rgba_frame(800, 600, 0, 0, 0, 255);
    let slots = [make_slot("in_0", None), make_slot("in_1", Some(frame))];
    let empty: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![]);
    let scene = resolve_scene(&slots, &config, &empty, &empty);

    // 800×600 (4:3) fits exactly in 400×300 (also 4:3) → 400×300
    let pip = &scene.layout.layers[1];
    assert_eq!(pip.width, 400);
    assert_eq!(pip.height, 300);
}

#[test]
fn test_resolve_scene_overlay_geometry() {
    let config = CompositorConfig { width: 1280, height: 720, ..Default::default() };
    let slots: Vec<InputSlot> = vec![];
    let empty: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![]);

    let text_overlay = Arc::new(overlay::DecodedOverlay {
        id: "text_0".to_string(),
        rgba_data: Arc::from(vec![0u8; 4]),
        width: 200,
        height: 40,
        rect: Rect { x: 50, y: 100, width: 200, height: 40 },
        opacity: 0.8,
        rotation_degrees: 15.0,
        z_index: 5,
        mirror_horizontal: true,
        mirror_vertical: false,
        measured_text_width: Some(195),
        measured_text_height: Some(38),
        source_kind: overlay::OverlaySourceKind::Raster,
    });
    let text_overlays: Arc<[Arc<overlay::DecodedOverlay>]> = Arc::from(vec![text_overlay]);

    let scene = resolve_scene(&slots, &config, &empty, &text_overlays);

    // View-data overlay carries only geometry + measurements.
    assert_eq!(scene.layout.text_overlays.len(), 1);
    let ro = &scene.layout.text_overlays[0];
    assert_eq!(ro.id, "text_0");
    assert_eq!(ro.x, 50);
    assert_eq!(ro.y, 100);
    assert_eq!(ro.width, 200);
    assert_eq!(ro.height, 40);
    assert_eq!(ro.measured_text_width, Some(195));
    assert_eq!(ro.measured_text_height, Some(38));
}

// ── Image overlay decode tests ──────────────────────────────────────────────

/// Helper: create a minimal valid PNG in memory (1×1 red pixel).
fn make_test_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).expect("PNG encode");
    buf.into_inner()
}

/// Helper: write a test PNG to a temp file under `samples/images/` and
/// return the relative path.  The directory is created if it doesn't exist.
fn write_test_asset(subdir: &str, filename: &str) -> String {
    let dir = std::path::PathBuf::from("samples/images").join(subdir);
    std::fs::create_dir_all(&dir).expect("create test dir");
    let path = dir.join(filename);
    std::fs::write(&path, make_test_png()).expect("write test PNG");
    format!("samples/images/{subdir}/{filename}")
}

/// Drop guard that removes a test asset file when it goes out of scope,
/// ensuring cleanup even if the test panics.
struct TestAssetGuard {
    path: String,
}

impl Drop for TestAssetGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn default_transform(w: u32, h: u32) -> config::OverlayTransform {
    config::OverlayTransform {
        rect: Rect { x: 0, y: 0, width: w, height: h },
        ..Default::default()
    }
}

#[test]
fn test_decode_image_overlay_from_asset_path() {
    let asset_path = write_test_asset("user", "test_decode_asset.png");
    let _guard = TestAssetGuard { path: asset_path.clone() };
    let cfg = config::ImageOverlayConfig {
        id: "img_asset".to_string(),
        asset_path: asset_path.clone(),
        transform: default_transform(10, 10),
    };
    overlay::validate_asset_path(&asset_path).expect("path should be valid");
    let bytes = std::fs::read(&asset_path).expect("read test asset");
    let result = overlay::decode_image_overlay(&cfg, &bytes, 7680);
    let decoded = result.expect("should decode from asset bytes");
    assert_eq!(decoded.id, "img_asset");
    assert!(decoded.width > 0);
    assert!(decoded.height > 0);
}

#[test]
fn test_decode_image_overlay_bad_path_traversal() {
    let result = overlay::validate_asset_path("samples/images/../../etc/passwd");
    assert!(result.is_err(), "path traversal should be rejected");
}

#[test]
fn test_decode_image_overlay_bad_path_prefix() {
    let result = overlay::validate_asset_path("/etc/passwd");
    assert!(result.is_err(), "paths outside samples/images/ should be rejected");
}

#[test]
fn test_image_overlay_cache_key_asset_path() {
    // Two configs with different asset_path values should not be considered
    // content-equal by apply_update_params' cache check.
    let a = config::ImageOverlayConfig {
        id: "img_a".to_string(),
        asset_path: "samples/images/user/a.png".to_string(),
        transform: default_transform(10, 10),
    };
    let b = config::ImageOverlayConfig {
        id: "img_a".to_string(),
        asset_path: "samples/images/user/b.png".to_string(),
        transform: default_transform(10, 10),
    };
    // Same id but different asset_path → content differs.
    assert_ne!(a.asset_path, b.asset_path);

    // Same asset_path → content same.
    let c = config::ImageOverlayConfig {
        id: "img_a".to_string(),
        asset_path: "samples/images/user/a.png".to_string(),
        transform: default_transform(10, 10),
    };
    assert_eq!(a.asset_path, c.asset_path);
}

#[test]
fn test_text_overlay_cache_reuses_arc_on_unchanged_config() {
    let txt_cfg = config::TextOverlayConfig {
        id: "cached".to_string(),
        text: "Hello".to_string(),
        transform: config::OverlayTransform::default(),
        color: [255, 255, 255, 255],
        font_size: 24,
        font_name: None,
    };
    let limits = GlobalCompositorConfig::default();
    let mut config =
        CompositorConfig { text_overlays: vec![txt_cfg.clone()], ..Default::default() };

    // Initial rasterize.
    let initial = Arc::new(rasterize_text_overlay(
        &txt_cfg,
        limits.max_canvas_dimension,
        limits.max_text_length,
    ));
    let mut text_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![initial]);
    let mut image_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);
    let original_ptr = Arc::as_ptr(&text_overlays[0]);

    // Send identical UpdateParams — should reuse the same Arc.
    let params = serde_json::json!({
        "text_overlays": [{
            "id": "cached",
            "text": "Hello",
            "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
            "color": [255, 255, 255, 255],
            "font_size": 24
        }]
    });
    let mut stats = NodeStatsTracker::new("test".to_string(), None);
    CompositorNode::apply_update_params(
        &mut config,
        &limits,
        &mut image_overlays,
        &mut text_overlays,
        params,
        &mut stats,
    );
    assert_eq!(
        Arc::as_ptr(&text_overlays[0]),
        original_ptr,
        "Unchanged text overlay should reuse the same Arc"
    );

    // Change the text — should produce a new Arc.
    let params = serde_json::json!({
        "text_overlays": [{
            "id": "cached",
            "text": "World",
            "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
            "color": [255, 255, 255, 255],
            "font_size": 24
        }]
    });
    CompositorNode::apply_update_params(
        &mut config,
        &limits,
        &mut image_overlays,
        &mut text_overlays,
        params,
        &mut stats,
    );
    assert_ne!(
        Arc::as_ptr(&text_overlays[0]),
        original_ptr,
        "Changed text overlay should produce a new Arc"
    );
}

#[tokio::test]
async fn test_compositor_output_format_nv12() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = CompositorConfig {
        width: 4,
        height: 4,
        output_format: Some("nv12".to_string()),
        ..Default::default()
    };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    let frame = make_rgba_frame(2, 2, 255, 0, 0, 255);
    input_tx.send(Packet::Video(frame)).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Expected at least 1 output frame");

    if let Packet::Video(ref out_frame) = output_packets[0] {
        assert_eq!(out_frame.width, 4);
        assert_eq!(out_frame.height, 4);
        assert_eq!(out_frame.pixel_format, PixelFormat::Nv12);
        // NV12: Y plane (4*4) + interleaved UV plane (2*2*2) = 24 bytes.
        assert_eq!(out_frame.data().len(), 24);
    } else {
        panic!("Expected video packet");
    }
}

#[test]
fn test_text_overlay_cache_handles_length_changes() {
    let make_txt = |id: &str, text: &str| config::TextOverlayConfig {
        id: id.to_string(),
        text: text.to_string(),
        transform: config::OverlayTransform::default(),
        color: [255, 255, 255, 255],
        font_size: 24,
        font_name: None,
    };
    let limits = GlobalCompositorConfig::default();
    let mut stats = NodeStatsTracker::new("test".to_string(), None);

    // Start with 2 overlays.
    let txt_a = make_txt("a", "Alpha");
    let txt_b = make_txt("b", "Beta");
    let mut config = CompositorConfig {
        text_overlays: vec![txt_a.clone(), txt_b.clone()],
        ..Default::default()
    };
    let initial_a = Arc::new(rasterize_text_overlay(
        &txt_a,
        limits.max_canvas_dimension,
        limits.max_text_length,
    ));
    let initial_b = Arc::new(rasterize_text_overlay(
        &txt_b,
        limits.max_canvas_dimension,
        limits.max_text_length,
    ));
    let mut text_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![initial_a, initial_b]);
    let mut image_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![]);
    let ptr_a = Arc::as_ptr(&text_overlays[0]);

    // Shrink to 1 overlay (keep "a" unchanged).
    let params = serde_json::json!({
        "text_overlays": [{
            "id": "a", "text": "Alpha",
            "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
            "color": [255, 255, 255, 255], "font_size": 24
        }]
    });
    CompositorNode::apply_update_params(
        &mut config,
        &limits,
        &mut image_overlays,
        &mut text_overlays,
        params,
        &mut stats,
    );
    assert_eq!(text_overlays.len(), 1, "Should have 1 overlay after shrink");
    assert_eq!(
        Arc::as_ptr(&text_overlays[0]),
        ptr_a,
        "Unchanged overlay 'a' should reuse the same Arc"
    );

    // Grow to 3 overlays (keep "a", add "c" and "d").
    let params = serde_json::json!({
        "text_overlays": [
            { "id": "a", "text": "Alpha",
              "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
              "color": [255, 255, 255, 255], "font_size": 24 },
            { "id": "c", "text": "Charlie",
              "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
              "color": [255, 0, 0, 255], "font_size": 32 },
            { "id": "d", "text": "Delta",
              "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
              "color": [0, 255, 0, 255], "font_size": 16 }
        ]
    });
    CompositorNode::apply_update_params(
        &mut config,
        &limits,
        &mut image_overlays,
        &mut text_overlays,
        params,
        &mut stats,
    );
    assert_eq!(text_overlays.len(), 3, "Should have 3 overlays after grow");
    assert_eq!(
        Arc::as_ptr(&text_overlays[0]),
        ptr_a,
        "Unchanged overlay 'a' should still reuse the same Arc"
    );
    // New overlays 'c' and 'd' were freshly rasterized (we just verify they exist
    // and are valid — pointer comparison is unreliable since the allocator may
    // reuse freed addresses).
    assert!(!text_overlays[1].rgba_data.is_empty(), "New overlay 'c' should have pixels");
    assert!(!text_overlays[2].rgba_data.is_empty(), "New overlay 'd' should have pixels");
}

#[tokio::test]
async fn test_compositor_output_format_i420() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

    let config = CompositorConfig {
        width: 4,
        height: 4,
        output_format: Some("i420".to_string()),
        ..Default::default()
    };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    let frame = make_rgba_frame(2, 2, 0, 255, 0, 255);
    input_tx.send(Packet::Video(frame)).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    drop(input_tx);

    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(!output_packets.is_empty(), "Expected at least 1 output frame");

    if let Packet::Video(ref out_frame) = output_packets[0] {
        assert_eq!(out_frame.width, 4);
        assert_eq!(out_frame.height, 4);
        assert_eq!(out_frame.pixel_format, PixelFormat::I420);
        // I420: Y plane (4*4) + U plane (2*2) + V plane (2*2) = 24 bytes.
        assert_eq!(out_frame.data().len(), 24);
    } else {
        panic!("Expected video packet");
    }
}

#[tokio::test]
async fn test_compositor_output_format_runtime_change() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let mut inputs = HashMap::new();
    inputs.insert("in_0".to_string(), input_rx);

    // Build context manually so we keep a handle to the control channel.
    let (ctrl_tx, control_rx) = mpsc::channel(10);
    let (state_tx, mut state_rx) = mpsc::channel(10);
    let (stats_tx, _stats_rx) = mpsc::channel(10);
    let (pin_mgmt_tx, pin_mgmt_rx) = mpsc::channel(10);
    drop(pin_mgmt_tx);

    let mock_sender = crate::test_utils::MockOutputSender::new();
    let output_sender = mock_sender.to_output_sender("test_node".to_string());

    let context = streamkit_core::node::NodeContext {
        inputs,
        input_types: HashMap::new(),
        control_rx,
        output_sender,
        batch_size: 10,
        state_tx,
        stats_tx: Some(stats_tx),
        telemetry_tx: None,
        session_id: None,
        cancellation_token: None,
        pin_management_rx: Some(pin_mgmt_rx),
        audio_pool: None,
        video_pool: None,
        pipeline_mode: streamkit_core::node::PipelineMode::Dynamic,
        view_data_tx: None,
    };

    // Start with no output_format (RGBA8).
    // Force CPU mode so the test isn't blocked by GpuContext::try_init()
    // competing for the GPU with other parallel tests on the self-hosted
    // runner.  This test validates runtime output_format switching, not
    // GPU compositing.
    let config = CompositorConfig {
        width: 4,
        height: 4,
        gpu_mode: Some("cpu".to_string()),
        ..Default::default()
    };
    let node = CompositorNode::new(config, GlobalCompositorConfig::default());

    let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

    assert_state_initializing(&mut state_rx).await;
    assert_state_running(&mut state_rx).await;

    // Send a frame — should come out as RGBA8.
    let frame = make_rgba_frame(2, 2, 255, 0, 0, 255);
    input_tx.send(Packet::Video(frame)).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Send UpdateParams to switch output_format to NV12.
    let update = serde_json::json!({ "output_format": "nv12" });
    ctrl_tx.send(NodeControlMessage::UpdateParams(update)).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Send another frame — should come out as NV12.
    let frame2 = make_rgba_frame(2, 2, 0, 255, 0, 255);
    input_tx.send(Packet::Video(frame2)).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    drop(input_tx);
    drop(ctrl_tx);
    assert_state_stopped(&mut state_rx).await;
    node_handle.await.unwrap().unwrap();

    let output_packets = mock_sender.get_packets_for_pin("out").await;
    assert!(
        output_packets.len() >= 2,
        "Expected at least 2 output frames, got {}",
        output_packets.len()
    );

    // First frame should be RGBA8.
    if let Packet::Video(ref f) = output_packets[0] {
        assert_eq!(f.pixel_format, PixelFormat::Rgba8, "First frame should be RGBA8");
    } else {
        panic!("Expected video packet");
    }

    // Last frame should be NV12 (after the UpdateParams took effect).
    let last = &output_packets[output_packets.len() - 1];
    if let Packet::Video(ref f) = last {
        assert_eq!(
            f.pixel_format,
            PixelFormat::Nv12,
            "Last frame should be NV12 after UpdateParams"
        );
    } else {
        panic!("Expected video packet");
    }
}

// ── Upstream resize hint tests ──────────────────────────────────────

#[test]
fn send_resize_hints_fires_on_rect_change() {
    let (hint_tx, mut hint_rx) = mpsc::channel::<UpstreamHint>(4);
    let slot = InputSlot {
        name: "cam".to_string(),
        rx: mpsc::channel(1).1,
        latest_frame: None,
        last_source_dims: None,
        hint_tx: Some(hint_tx),
    };

    let old_config = CompositorConfig {
        layers: {
            let mut m = HashMap::new();
            m.insert(
                "cam".to_string(),
                LayerConfig {
                    rect: Some(Rect { x: 0, y: 0, width: 640, height: 480 }),
                    ..Default::default()
                },
            );
            m
        },
        ..Default::default()
    };
    let new_config = CompositorConfig {
        layers: {
            let mut m = HashMap::new();
            m.insert(
                "cam".to_string(),
                LayerConfig {
                    rect: Some(Rect { x: 0, y: 0, width: 1280, height: 720 }),
                    ..Default::default()
                },
            );
            m
        },
        ..Default::default()
    };

    CompositorNode::send_resize_hints(&old_config, &new_config, &[slot]);

    let hint = hint_rx.try_recv().expect("should have received a hint");
    assert_eq!(hint, UpstreamHint::PreferredSize { width: 1280, height: 720 });
}

#[test]
fn send_resize_hints_skips_unchanged_rect() {
    let (hint_tx, mut hint_rx) = mpsc::channel::<UpstreamHint>(4);
    let slot = InputSlot {
        name: "cam".to_string(),
        rx: mpsc::channel(1).1,
        latest_frame: None,
        last_source_dims: None,
        hint_tx: Some(hint_tx),
    };

    let config = CompositorConfig {
        layers: {
            let mut m = HashMap::new();
            m.insert(
                "cam".to_string(),
                LayerConfig {
                    rect: Some(Rect { x: 0, y: 0, width: 640, height: 480 }),
                    ..Default::default()
                },
            );
            m
        },
        ..Default::default()
    };

    CompositorNode::send_resize_hints(&config, &config, &[slot]);

    assert!(hint_rx.try_recv().is_err(), "no hint should be sent when rect is unchanged");
}

#[test]
fn send_resize_hints_skips_slot_without_hint_tx() {
    let slot = InputSlot {
        name: "cam".to_string(),
        rx: mpsc::channel(1).1,
        latest_frame: None,
        last_source_dims: None,
        hint_tx: None,
    };

    let old_config = CompositorConfig::default();
    let new_config = CompositorConfig {
        layers: {
            let mut m = HashMap::new();
            m.insert(
                "cam".to_string(),
                LayerConfig {
                    rect: Some(Rect { x: 0, y: 0, width: 1280, height: 720 }),
                    ..Default::default()
                },
            );
            m
        },
        ..Default::default()
    };

    // Should not panic even with hint_tx: None
    CompositorNode::send_resize_hints(&old_config, &new_config, &[slot]);
}

// ── SVG overlay tests ───────────────────────────────────────────────

/// Minimal valid SVG for testing.
const TEST_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/></svg>"#;

/// 2:1 aspect ratio SVG for aspect-fit testing.
const TEST_SVG_WIDE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="blue"/></svg>"#;

fn svg_image_config(id: &str, w: u32, h: u32) -> config::ImageOverlayConfig {
    config::ImageOverlayConfig {
        id: id.to_string(),
        asset_path: "samples/images/system/test.svg".to_string(),
        transform: config::OverlayTransform {
            rect: Rect { x: 0, y: 0, width: w, height: h },
            opacity: 1.0,
            rotation_degrees: 0.0,
            z_index: 0,
            mirror_horizontal: false,
            mirror_vertical: false,
        },
    }
}

#[test]
fn test_rasterize_svg_basic() {
    let cfg = svg_image_config("svg-basic", 100, 100);
    let result = overlay::decode_image_overlay(&cfg, TEST_SVG.as_bytes(), 7680);
    let ov = result.expect("SVG decode should succeed");
    assert!(ov.width > 0);
    assert!(ov.height > 0);
    assert!(matches!(ov.source_kind, overlay::OverlaySourceKind::Vector { .. }));
    assert!(!ov.rgba_data.is_empty());
}

#[test]
fn test_rasterize_svg_aspect_fit() {
    // 2:1 SVG (200x100) into a square 100x100 rect.
    let cfg = svg_image_config("svg-aspect", 100, 100);
    let result = overlay::decode_image_overlay(&cfg, TEST_SVG_WIDE.as_bytes(), 7680);
    let ov = result.expect("SVG decode should succeed");
    // Aspect-preserving fit: width should fill 100, height should be 50.
    assert_eq!(ov.width, 100);
    assert_eq!(ov.height, 50);
    // Centred vertically: y offset should be (100 - 50) / 2 = 25.
    assert_eq!(ov.rect.y, 25);
    assert_eq!(ov.rect.x, 0);
}

#[test]
fn test_rasterize_svg_max_dimension_clamp() {
    // Request a very large target but clamp to max_dimension=50.
    let cfg = svg_image_config("svg-clamp", 200, 200);
    let result = overlay::decode_image_overlay(&cfg, TEST_SVG.as_bytes(), 50);
    let ov = result.expect("SVG decode should succeed");
    assert!(ov.width <= 50);
    assert!(ov.height <= 50);
}

#[test]
fn test_rasterize_svg_invalid_data() {
    let cfg = config::ImageOverlayConfig {
        id: "svg-invalid".to_string(),
        asset_path: "samples/images/system/bad.svg".to_string(),
        transform: config::OverlayTransform {
            rect: Rect { x: 0, y: 0, width: 100, height: 100 },
            opacity: 1.0,
            rotation_degrees: 0.0,
            z_index: 0,
            mirror_horizontal: false,
            mirror_vertical: false,
        },
    };
    let result = overlay::decode_image_overlay(&cfg, b"not an svg at all", 7680);
    assert!(result.is_err(), "Non-SVG bytes with .svg extension should fail");
}

#[test]
fn test_decode_image_overlay_dispatches_svg() {
    let cfg = svg_image_config("svg-dispatch", 80, 80);
    let result = overlay::decode_image_overlay(&cfg, TEST_SVG.as_bytes(), 7680);
    let ov = result.expect("SVG should be dispatched and decoded");
    assert!(matches!(ov.source_kind, overlay::OverlaySourceKind::Vector { .. }));
}

#[test]
fn test_unpremultiply_alpha() {
    // Premultiplied red at 50% alpha: R=128, G=0, B=0, A=128
    // Straight alpha: R = 128 / (128/255) ≈ 255, G=0, B=0, A=128
    // Due to integer rounding, R may be 254 or 255.
    let mut data = vec![128u8, 0, 0, 128];
    overlay::unpremultiply_alpha_for_test(&mut data);
    assert!(data[0] >= 254, "R should be ~255, got {}", data[0]);
    assert_eq!(data[1], 0); // G
    assert_eq!(data[2], 0); // B
    assert_eq!(data[3], 128); // A unchanged

    // Fully opaque pixel should remain unchanged.
    let mut opaque = vec![200u8, 100, 50, 255];
    overlay::unpremultiply_alpha_for_test(&mut opaque);
    assert_eq!(opaque, vec![200, 100, 50, 255]);

    // Fully transparent pixel should remain unchanged.
    let mut transparent = vec![0u8, 0, 0, 0];
    overlay::unpremultiply_alpha_for_test(&mut transparent);
    assert_eq!(transparent, vec![0, 0, 0, 0]);
}

#[test]
fn test_is_svg_detection() {
    // Extension detection
    assert!(overlay::is_svg_for_test(b"anything", "logo.svg"));
    assert!(overlay::is_svg_for_test(b"anything", "logo.svgz"));
    assert!(!overlay::is_svg_for_test(b"anything", "logo.png"));

    // Content detection (no SVG extension)
    assert!(overlay::is_svg_for_test(b"<svg xmlns='http://www.w3.org/2000/svg'>", "file.xml"));
    assert!(overlay::is_svg_for_test(b"<?xml version='1.0'?><svg>", "file.xml"));
    assert!(!overlay::is_svg_for_test(b"PNG header stuff", "file.xml"));
}

#[test]
fn test_svg_viewbox_dimensions() {
    let dims = overlay::svg_viewbox_dimensions(TEST_SVG.as_bytes());
    assert_eq!(dims, Some((100, 100)));

    let dims = overlay::svg_viewbox_dimensions(TEST_SVG_WIDE.as_bytes());
    assert_eq!(dims, Some((200, 100)));

    let dims = overlay::svg_viewbox_dimensions(b"not valid svg");
    assert_eq!(dims, None);
}

#[test]
fn test_rebuild_svg_rerasterizes_on_resize() {
    let svg_bytes = TEST_SVG.as_bytes();
    let cfg_100 = svg_image_config("svg-resize", 100, 100);
    let cfg_200 = config::ImageOverlayConfig {
        id: "svg-resize".to_string(),
        asset_path: "samples/images/system/test.svg".to_string(),
        transform: config::OverlayTransform {
            rect: Rect { x: 0, y: 0, width: 200, height: 200 },
            opacity: 1.0,
            rotation_degrees: 0.0,
            z_index: 0,
            mirror_horizontal: false,
            mirror_vertical: false,
        },
    };

    // Build initial overlay at 100x100.
    let initial = overlay::decode_image_overlay(&cfg_100, svg_bytes, 7680).expect("initial decode");
    assert_eq!(initial.width, 100);
    assert!(matches!(initial.source_kind, overlay::OverlaySourceKind::Vector { .. }));

    let old_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![Arc::new(initial)]);

    let old_config = CompositorConfig { image_overlays: vec![cfg_100], ..Default::default() };
    let new_config = CompositorConfig { image_overlays: vec![cfg_200], ..Default::default() };

    let rebuilt = CompositorNode::rebuild_image_overlays(
        &old_config,
        &new_config,
        &old_overlays,
        &config::GlobalCompositorConfig::default(),
    );

    assert_eq!(rebuilt.len(), 1);
    // Re-rasterized at new dimensions.
    assert_eq!(rebuilt[0].width, 200);
    assert_eq!(rebuilt[0].height, 200);
    assert!(matches!(rebuilt[0].source_kind, overlay::OverlaySourceKind::Vector { .. }));
}

#[test]
fn test_rebuild_svg_reuses_bitmap_when_unchanged() {
    let svg_bytes = TEST_SVG.as_bytes();
    let cfg = svg_image_config("svg-reuse", 100, 100);

    let initial = overlay::decode_image_overlay(&cfg, svg_bytes, 7680).expect("initial decode");
    let initial_arc = Arc::new(initial);

    let old_overlays: Arc<[Arc<DecodedOverlay>]> = Arc::from(vec![Arc::clone(&initial_arc)]);

    let config_with_overlay = CompositorConfig { image_overlays: vec![cfg], ..Default::default() };

    // Rebuild with identical config — should reuse the existing bitmap.
    let rebuilt = CompositorNode::rebuild_image_overlays(
        &config_with_overlay,
        &config_with_overlay,
        &old_overlays,
        &config::GlobalCompositorConfig::default(),
    );

    assert_eq!(rebuilt.len(), 1);
    // content_same is true (same path + same dimensions), so the bitmap
    // data was cloned from the existing overlay (shallow clone of Vec).
    // The width/height should be identical.
    assert_eq!(rebuilt[0].width, initial_arc.width);
    assert_eq!(rebuilt[0].height, initial_arc.height);
}
