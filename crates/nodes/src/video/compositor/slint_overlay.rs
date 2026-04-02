// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Slint UI overlay rendering for the video compositor.
//!
//! Compiles `.slint` files at init time via `slint-interpreter` and renders
//! them into RGBA8 bitmaps using the software renderer.  The resulting
//! [`DecodedOverlay`] feeds directly into the existing `composite_frame`
//! pipeline — no changes to `kernel.rs`.
//!
//! ## Threading
//!
//! All Slint objects (`MinimalSoftwareWindow`, `ComponentInstance`) are
//! `!Send` (`Rc`-based).  They live exclusively on the compositor's
//! persistent `spawn_blocking` thread, matching the existing compositing
//! architecture.

use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::WindowAdapter;
use slint::{ComponentHandle, LogicalSize, SharedString};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value};

use super::config::SlintOverlayConfig;
use super::overlay::DecodedOverlay;
use streamkit_core::StreamKitError;

// ── Public types ────────────────────────────────────────────────────────────

/// A compiled Slint overlay instance ready for per-frame rendering.
///
/// Created once at pipeline init on the compositor's blocking thread.
/// `!Send` by design — must not leave that thread.
pub struct SlintOverlayInstance {
    window: Rc<MinimalSoftwareWindow>,
    component: ComponentInstance,
    #[allow(dead_code)]
    definition: ComponentDefinition,
    buffer: Vec<PremultipliedRgbaColor>,
    width: u32,
    height: u32,
    /// Frame counter for property keyframe cycling.
    frame_counter: u32,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compile a `.slint` file and create a renderable overlay instance.
///
/// Must be called on the compositor's blocking thread (`!Send`).
///
/// # Errors
///
/// Returns an error if the `.slint` file cannot be compiled or if no
/// matching component definition is found.
pub fn create_slint_overlay(
    config: &SlintOverlayConfig,
) -> Result<SlintOverlayInstance, StreamKitError> {
    let width = config.transform.rect.width;
    let height = config.transform.rect.height;
    if width == 0 || height == 0 {
        return Err(StreamKitError::Configuration(format!(
            "Slint overlay '{}': rect width and height must be > 0",
            config.id
        )));
    }

    // Compile the .slint file.
    let compiler = slint_interpreter::Compiler::default();
    let result = spin_on::spin_on(compiler.build_from_path(&config.slint_file));

    // Check for compilation errors.
    let diags: Vec<_> = result
        .diagnostics()
        .filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error)
        .collect();
    if !diags.is_empty() {
        let msgs: Vec<String> = diags.iter().map(|d| d.message().to_string()).collect();
        return Err(StreamKitError::Configuration(format!(
            "Slint compilation errors in '{}': {}",
            config.slint_file,
            msgs.join("; ")
        )));
    }

    // Get the component definition.
    let definition = if let Some(ref name) = config.component {
        result.component(name).ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "Slint overlay '{}': component '{}' not found in '{}'",
                config.id, name, config.slint_file
            ))
        })?
    } else {
        // Use the first exported component.
        result.components().next().ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "Slint overlay '{}': no exported components in '{}'",
                config.id, config.slint_file
            ))
        })?
    };

    // Create the minimal software window.
    // Overlay dimensions are bounded by max_canvas_dimension (default 7680),
    // so the u32→f32 precision loss above ~16M is irrelevant here.
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    #[allow(clippy::cast_precision_loss)]
    window.set_size(LogicalSize::new((width as f32).max(1.0), (height as f32).max(1.0)));

    // Set as the Slint platform backend for this thread.
    // This may fail if already set (e.g. multiple overlays), which is fine.
    let window_adapter = window.clone() as Rc<dyn WindowAdapter>;
    let _ = slint::platform::set_platform(Box::new(SlintBackend { window: window_adapter }));

    // Instantiate the component.
    let component = definition.create().map_err(|e| {
        StreamKitError::Configuration(format!(
            "Slint overlay '{}': failed to create component instance: {e}",
            config.id
        ))
    })?;

    // Set initial properties.
    set_properties(&component, &config.properties);

    // Allocate pixel buffer.
    let pixel_count = (width as usize) * (height as usize);
    let buffer = vec![PremultipliedRgbaColor::default(); pixel_count];

    // Show the component so it becomes visible for rendering.
    component.show().map_err(|e| {
        StreamKitError::Configuration(format!(
            "Slint overlay '{}': failed to show component: {e}",
            config.id
        ))
    })?;

    Ok(SlintOverlayInstance {
        window,
        component,
        definition,
        buffer,
        width,
        height,
        frame_counter: 0,
    })
}

/// Re-render a Slint overlay, updating properties from the config.
///
/// Always renders on each call to ensure the overlay stays in sync with
/// property updates.  Returns a fresh `DecodedOverlay` with the rendered
/// RGBA8 bitmap.
pub fn render_slint_overlay(
    instance: &mut SlintOverlayInstance,
    config: &SlintOverlayConfig,
) -> DecodedOverlay {
    // Build the effective property map: base properties merged with the
    // current keyframe (if keyframes are configured).
    let effective_props = if config.property_keyframes.is_empty() {
        std::borrow::Cow::Borrowed(&config.properties)
    } else {
        let interval = config.keyframe_interval.max(1);
        let idx = (instance.frame_counter / interval) as usize % config.property_keyframes.len();
        let mut merged = config.properties.clone();
        merged.extend(config.property_keyframes[idx].iter().map(|(k, v)| (k.clone(), v.clone())));
        std::borrow::Cow::Owned(merged)
    };
    instance.frame_counter = instance.frame_counter.wrapping_add(1);

    // Push property updates into the component instance.
    set_properties(&instance.component, &effective_props);

    // Pump Slint's internal animation timers so time-based animations
    // (e.g. slide-in transitions) advance on each compositor tick.
    slint::platform::update_timers_and_animations();

    // Render into the pixel buffer.
    let width = instance.width;
    instance.window.draw_if_needed(|renderer| {
        renderer.render(&mut instance.buffer, width as usize);
    });

    // Convert premultiplied buffer to straight-alpha RGBA8.
    let rgba_data = premultiplied_to_straight_rgba(&instance.buffer);

    DecodedOverlay {
        id: config.id.clone(),
        rgba_data,
        width: instance.width,
        height: instance.height,
        rect: config.transform.rect,
        opacity: config.transform.opacity,
        rotation_degrees: config.transform.rotation_degrees,
        z_index: config.transform.z_index,
        mirror_horizontal: config.transform.mirror_horizontal,
        mirror_vertical: config.transform.mirror_vertical,
        measured_text_width: None,
        measured_text_height: None,
    }
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Map JSON property values to Slint `Value` and set them on the component.
fn set_properties(
    component: &ComponentInstance,
    properties: &std::collections::HashMap<String, serde_json::Value>,
) {
    for (key, json_val) in properties {
        let slint_val = json_to_slint_value(json_val);
        if let Err(e) = component.set_property(key, slint_val) {
            tracing::warn!("Failed to set Slint property '{key}': {e}");
        }
    }
}

/// Convert a JSON value to a Slint interpreter `Value`.
fn json_to_slint_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::String(s) => Value::String(SharedString::from(s.as_str())),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        // Slint's Value::Number takes f64.  JSON integers arrive as i64;
        // the i64→f64 cast may lose precision for values > 2^52, which is
        // acceptable for UI property values (scores, counters, etc.).
        #[allow(clippy::cast_precision_loss)]
        serde_json::Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Number(n.as_f64().unwrap_or(0.0)), |i| Value::Number(i as f64)),
        _ => Value::Void,
    }
}

/// Convert a slice of premultiplied-alpha pixels to straight-alpha RGBA8.
///
/// `DecodedOverlay` expects straight (non-premultiplied) RGBA8 data, so we
/// reverse the premultiplication that Slint's software renderer applies.
///
/// The `as u8` casts below are safe: for premultiplied data the invariant
/// `channel <= alpha` holds, so `channel * 255 / alpha <= 255` — always
/// fits in a `u8`.
#[allow(clippy::cast_possible_truncation)]
fn premultiplied_to_straight_rgba(pixels: &[PremultipliedRgbaColor]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for px in pixels {
        if px.alpha == 0 {
            bytes.extend_from_slice(&[0, 0, 0, 0]);
        } else if px.alpha == 255 {
            bytes.extend_from_slice(&[px.red, px.green, px.blue, 255]);
        } else {
            // Un-premultiply: channel = premultiplied * 255 / alpha
            let a = u16::from(px.alpha);
            let r = (u16::from(px.red) * 255 / a) as u8;
            let g = (u16::from(px.green) * 255 / a) as u8;
            let b = (u16::from(px.blue) * 255 / a) as u8;
            bytes.extend_from_slice(&[r, g, b, px.alpha]);
        }
    }
    bytes
}

// ── Slint platform backend ──────────────────────────────────────────────────

/// Minimal Slint platform backend that provides the software window.
///
/// Required by Slint's runtime to know where to render.  Only one platform
/// can be set per process, so subsequent `set_platform` calls after the
/// first will silently fail — which is fine since all overlays on this
/// thread share the same backend.
struct SlintBackend {
    window: Rc<dyn WindowAdapter>,
}

impl slint::platform::Platform for SlintBackend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}
