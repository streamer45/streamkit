// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared Slint renderer thread.
//!
//! `slint::platform::set_platform` is process-global and the types it exposes
//! (`MinimalSoftwareWindow`, `ComponentInstance`) are `!Send` (`Rc`-based).
//! To support multiple plugin instances without UB, all Slint work is
//! funnelled through a single dedicated `std::thread`, lazily spawned on the
//! first instance's init.
//!
//! Each instance gets a unique `NodeId` (UUID) and communicates with the
//! shared thread via tagged work items.  Results are sent back on per-node
//! `std::sync::mpsc` channels so `tick()` can block-receive without needing
//! a tokio runtime.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::WindowAdapter;
use slint::{ComponentHandle, LogicalSize, SharedString};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value, ValueType};

use crate::config::SlintConfig;

/// Opaque identifier for a plugin instance on the shared Slint thread.
pub type NodeId = uuid::Uuid;

/// Describes a single publicly declared property discovered from a compiled
/// `.slint` component.  Used to build the runtime param schema.
#[derive(Debug, Clone)]
pub struct DiscoveredProperty {
    pub name: String,
    pub value_type: DiscoveredValueType,
    /// The initial value of the property as declared in the `.slint` file.
    /// Used as the `default` in the runtime JSON Schema so the UI can show
    /// the correct initial state (e.g. a toggle that is `true` at startup).
    pub initial_value: Option<serde_json::Value>,
}

/// Subset of `slint_interpreter::ValueType` that maps to JSON Schema types
/// the UI can render as controls.
#[derive(Debug, Clone, Copy)]
pub enum DiscoveredValueType {
    Bool,
    Number,
    String,
}

/// Work item sent from a plugin's `tick()` to the shared Slint thread.
pub enum SlintWorkItem {
    /// Register a new instance: compile its `.slint` file and create a component.
    /// The `result_tx` is stored by the shared thread for sending render
    /// results and the init outcome back.
    Register {
        node_id: NodeId,
        config: SlintConfig,
        result_tx: std::sync::mpsc::SyncSender<SlintThreadResult>,
    },
    /// Request a single rendered frame for the given instance.
    Render { node_id: NodeId },
    /// Update the config (properties / keyframes) for subsequent renders.
    UpdateConfig { node_id: NodeId, config: SlintConfig },
    /// Resize the rendering window and buffer for the given instance.
    /// Sent when an upstream hint requests a new preferred size.
    Resize { node_id: NodeId, width: u32, height: u32 },
    /// Unregister an instance — drop its component and result channel.
    Unregister { node_id: NodeId },
}

/// Result sent from the shared Slint thread back to a specific instance.
pub enum SlintThreadResult {
    /// Init succeeded — the instance can start rendering.
    /// Carries the list of publicly declared properties discovered from the
    /// compiled `.slint` component (may be empty).
    InitOk { properties: Vec<DiscoveredProperty> },
    /// Init failed with an error message.
    InitErr(String),
    /// A rendered frame.
    Frame { rgba_data: Vec<u8> },
}

/// Handle to the shared Slint thread's work channel.
struct SlintThreadHandle {
    work_tx: std::sync::mpsc::Sender<SlintWorkItem>,
}

/// Get (or lazily spawn) the shared Slint thread.
///
/// # Panics
///
/// Panics if the OS fails to spawn the dedicated Slint renderer thread
/// (e.g. resource exhaustion).  This is unrecoverable — the plugin cannot
/// render Slint UIs without this thread.
#[allow(clippy::expect_used)]
fn shared_slint_thread() -> &'static SlintThreadHandle {
    static HANDLE: OnceLock<SlintThreadHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (work_tx, work_rx) = std::sync::mpsc::channel::<SlintWorkItem>();
        std::thread::Builder::new()
            .name("slint-plugin-renderer".to_string())
            .spawn(move || slint_thread_main(work_rx))
            .expect("Failed to spawn shared Slint renderer thread");
        SlintThreadHandle { work_tx }
    })
}

/// Send a work item to the shared Slint thread.
///
/// # Errors
///
/// Returns an error if the shared thread has panicked or been dropped.
pub fn send_work(item: SlintWorkItem) -> Result<(), String> {
    shared_slint_thread()
        .work_tx
        .send(item)
        .map_err(|_| "Slint renderer thread is no longer running".to_string())
}

// ── Slint thread main loop ──────────────────────────────────────────────────

/// Entry point for the shared Slint thread.
///
/// Processes work items from all plugin instances.  The platform backend is
/// set once on this thread; all `SlintInstance` values live here.
#[allow(clippy::needless_pass_by_value)]
fn slint_thread_main(work_rx: std::sync::mpsc::Receiver<SlintWorkItem>) {
    /// Per-instance state living on the shared thread.
    struct InstanceState {
        instance: SlintInstance,
        config: SlintConfig,
        result_tx: std::sync::mpsc::SyncSender<SlintThreadResult>,
        /// Cached straight-alpha RGBA8 output from the last render.
        cached_frame: Option<Vec<u8>>,
        /// Keyframe index that produced `cached_frame`.
        cached_keyframe_idx: Option<usize>,
        /// Set by `UpdateConfig` to force a re-render on the next frame.
        dirty: bool,
    }

    let mut instances: HashMap<NodeId, InstanceState> = HashMap::new();
    let mut platform_set = false;

    while let Ok(work) = work_rx.recv() {
        match work {
            SlintWorkItem::Register { node_id, config, result_tx } => {
                match create_slint_instance(&config, &mut platform_set) {
                    Ok(instance) => {
                        // Discover publicly declared properties from the compiled
                        // component.  Only types the UI can render as controls
                        // (bool, number, string) are included.
                        let properties =
                            discover_properties(&instance.definition, &instance.component);

                        tracing::info!(
                            node_id = %node_id,
                            slint_file = %config.slint_file,
                            discovered_properties = properties.len(),
                            "Created Slint instance",
                        );
                        let _ = result_tx.send(SlintThreadResult::InitOk { properties });
                        instances.insert(
                            node_id,
                            InstanceState {
                                instance,
                                config,
                                result_tx,
                                cached_frame: None,
                                cached_keyframe_idx: None,
                                dirty: true,
                            },
                        );
                    },
                    Err(e) => {
                        tracing::error!(
                            node_id = %node_id,
                            error = %e,
                            "Failed to create Slint instance",
                        );
                        let _ = result_tx.send(SlintThreadResult::InitErr(e));
                    },
                }
            },
            SlintWorkItem::Render { node_id } => {
                if let Some(state) = instances.get_mut(&node_id) {
                    // Pump Slint timers/animations (process-global) so Timer
                    // callbacks and CSS-like transitions advance even when the
                    // frame is served from cache.  This call is idempotent and
                    // wall-clock-based, so running it N times per tick cycle
                    // (once per instance) is harmless.
                    slint::platform::update_timers_and_animations();

                    let rgba_data = if state.config.static_ui {
                        // ── Static UI path: cache the rendered frame ────────
                        let kf_idx = if state.config.property_keyframes.is_empty() {
                            None
                        } else {
                            let interval = state.config.keyframe_interval.max(1);
                            Some(
                                (state.instance.frame_counter / interval) as usize
                                    % state.config.property_keyframes.len(),
                            )
                        };

                        let need_render = state.dirty
                            || state.cached_keyframe_idx != kf_idx
                            || state.cached_frame.is_none();

                        if need_render {
                            let data = render_slint_frame(&mut state.instance, &state.config);
                            state.cached_frame = Some(data);
                            state.cached_keyframe_idx = kf_idx;
                            state.dirty = false;
                        } else {
                            // Advance frame counter so keyframe boundaries
                            // are detected at the right time.
                            state.instance.frame_counter =
                                state.instance.frame_counter.wrapping_add(1);
                        }
                        // Clone from cache — avoids a redundant allocation
                        // compared to cloning before storing.
                        state.cached_frame.clone().unwrap_or_default()
                    } else {
                        // ── Dynamic UI path: always re-render ───────────────
                        render_slint_frame(&mut state.instance, &state.config)
                    };

                    // Use try_send to avoid blocking: if the consumer is slow,
                    // drop the frame rather than stalling the shared thread.
                    match state.result_tx.try_send(SlintThreadResult::Frame { rgba_data }) {
                        Ok(()) => {},
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            tracing::debug!(
                                node_id = %node_id,
                                "Result channel full, dropping frame",
                            );
                        },
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            instances.remove(&node_id);
                        },
                    }
                }
            },
            SlintWorkItem::UpdateConfig { node_id, config } => {
                if let Some(state) = instances.get_mut(&node_id) {
                    state.config = config;
                    state.dirty = true;
                }
            },
            SlintWorkItem::Resize { node_id, width, height } => {
                if let Some(state) = instances.get_mut(&node_id) {
                    if state.instance.width != width || state.instance.height != height {
                        state.instance.width = width;
                        state.instance.height = height;
                        state.config.width = width;
                        state.config.height = height;
                        #[allow(clippy::cast_precision_loss)]
                        state.instance.window.set_size(LogicalSize::new(
                            (width as f32).max(1.0),
                            (height as f32).max(1.0),
                        ));
                        let pixel_count = (width as usize) * (height as usize);
                        state.instance.buffer =
                            vec![PremultipliedRgbaColor::default(); pixel_count];
                        state.cached_frame = None;
                        state.dirty = true;
                        tracing::info!(
                            node_id = %node_id,
                            width, height,
                            "Resized Slint instance via upstream hint",
                        );
                    }
                }
            },
            SlintWorkItem::Unregister { node_id } => {
                instances.remove(&node_id);
            },
        }
    }
}

// ── Slint rendering internals ───────────────────────────────────────────────

/// Scope guard that clears the `CURRENT_WINDOW` thread-local on drop.
///
/// Used by `create_slint_instance` to ensure the thread-local is cleaned up
/// even if `definition.create()` or `component.show()` fails via `?`.
struct ClearWindow;

impl Drop for ClearWindow {
    fn drop(&mut self) {
        CURRENT_WINDOW.with(|cell| *cell.borrow_mut() = None);
    }
}

/// A compiled Slint component instance ready for per-frame rendering.
///
/// Created once at init on the shared Slint thread.
/// `!Send` by design — must not leave that thread.
struct SlintInstance {
    window: Rc<MinimalSoftwareWindow>,
    component: ComponentInstance,
    /// Kept alive to prevent the compiled Slint component definition from being dropped.
    #[allow(dead_code)]
    definition: ComponentDefinition,
    buffer: Vec<PremultipliedRgbaColor>,
    width: u32,
    height: u32,
    /// Frame counter for property keyframe cycling.
    frame_counter: u32,
}

/// Compile a `.slint` file and create a renderable instance.
///
/// Must be called on the shared Slint thread (`!Send` types).
/// The `platform_set` flag tracks whether `set_platform` has already been
/// called — it must happen exactly once per process.
fn create_slint_instance(
    config: &SlintConfig,
    platform_set: &mut bool,
) -> Result<SlintInstance, String> {
    let width = config.width;
    let height = config.height;

    // Compile the .slint file.
    let compiler = slint_interpreter::Compiler::default();
    let result = pollster::block_on(compiler.build_from_path(&config.slint_file));

    // Check for compilation errors.
    let diags: Vec<_> = result
        .diagnostics()
        .filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error)
        .collect();
    if !diags.is_empty() {
        let msgs: Vec<String> = diags.iter().map(|d| d.message().to_string()).collect();
        return Err(format!(
            "Slint compilation errors in '{}': {}",
            config.slint_file,
            msgs.join("; ")
        ));
    }

    // Get the component definition.
    let definition = if let Some(ref name) = config.component {
        result
            .component(name)
            .ok_or_else(|| format!("Component '{}' not found in '{}'", name, config.slint_file))?
    } else {
        // Use the first exported component.
        result
            .components()
            .next()
            .ok_or_else(|| format!("No exported components in '{}'", config.slint_file))?
    };

    // Create the minimal software window.
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    #[allow(clippy::cast_precision_loss)]
    window.set_size(LogicalSize::new((width as f32).max(1.0), (height as f32).max(1.0)));

    // Set the Slint platform backend exactly once per process.
    if !*platform_set {
        slint::platform::set_platform(Box::new(SlintBackend))
            .map_err(|e| format!("Failed to set Slint platform: {e}"))?;
        *platform_set = true;
    }

    // Swap in this instance's window so `create_window_adapter()` returns
    // the correct one during `definition.create()` and `component.show()`.
    // The `ClearWindow` guard ensures the thread-local is cleared even if
    // either call fails via `?`, preventing a stale `Rc<dyn WindowAdapter>`
    // from lingering until the next `Register`.
    let window_adapter = window.clone() as Rc<dyn WindowAdapter>;
    CURRENT_WINDOW.with(|cell| *cell.borrow_mut() = Some(window_adapter));
    let _guard = ClearWindow;

    // Instantiate the component.
    let component = definition
        .create()
        .map_err(|e| format!("Failed to create Slint component instance: {e}"))?;

    // Set initial properties.
    set_properties(&component, &config.properties);

    // Allocate pixel buffer.
    let pixel_count = (width as usize) * (height as usize);
    let buffer = vec![PremultipliedRgbaColor::default(); pixel_count];

    // Show the component so it becomes visible for rendering.
    component.show().map_err(|e| format!("Failed to show Slint component: {e}"))?;

    // _guard drops here, clearing CURRENT_WINDOW.

    Ok(SlintInstance { window, component, definition, buffer, width, height, frame_counter: 0 })
}

/// Enumerate the publicly declared properties of a compiled Slint component
/// and return those whose types map to JSON Schema primitives the UI can
/// render as controls (boolean → toggle, number → slider, string → text).
///
/// Also reads the initial value of each property from the instantiated
/// component so the UI can show the correct initial state.
///
/// **Limitation:** `.slint` files are assumed to be static for the lifetime
/// of the node.  Property discovery happens once at initialization; if the
/// source file changes, the node must be re-created to pick up new properties.
fn discover_properties(
    definition: &ComponentDefinition,
    component: &ComponentInstance,
) -> Vec<DiscoveredProperty> {
    definition
        .properties()
        .filter_map(|(name, value_type)| {
            let vt = match value_type {
                ValueType::Bool => DiscoveredValueType::Bool,
                ValueType::Number => DiscoveredValueType::Number,
                ValueType::String => DiscoveredValueType::String,
                // Image, Model, Struct, Brush, etc. are not tuneable.
                _ => return None,
            };
            // Read the initial value from the live component instance so the
            // UI can display the correct default (e.g. clock_running = true).
            let initial_value = component.get_property(&name).ok().and_then(|v| match v {
                Value::Bool(b) => Some(serde_json::Value::Bool(b)),
                Value::Number(n) => {
                    let json_num = serde_json::Number::from_f64(n);
                    if json_num.is_none() {
                        tracing::warn!(
                            property = %name,
                            value = %n,
                            "Slint property has NaN/Infinity value, dropping default"
                        );
                    }
                    json_num.map(serde_json::Value::Number)
                },
                Value::String(s) => Some(serde_json::Value::String(s.to_string())),
                _ => None,
            });
            // Slint normalizes identifiers to kebab-case internally
            // (e.g. `clock_running` → `clock-running`).  The rest of the
            // StreamKit stack (YAML params, JSON UpdateParams, set_properties)
            // uses snake_case, so convert back to match.
            let name = name.replace('-', "_");
            Some(DiscoveredProperty { name, value_type: vt, initial_value })
        })
        .collect()
}

/// Render a single frame from the Slint instance, returning raw RGBA8 data.
///
/// Applies property keyframe cycling.  Timer/animation pumping is handled
/// by the caller (`slint_thread_main`) so it runs unconditionally.
fn render_slint_frame(instance: &mut SlintInstance, config: &SlintConfig) -> Vec<u8> {
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

    // Force a full redraw every frame.
    instance.window.request_redraw();

    // Render into the pixel buffer.
    let width = instance.width;
    instance.window.draw_if_needed(|renderer| {
        renderer.render(&mut instance.buffer, width as usize);
    });

    // Convert premultiplied buffer to straight-alpha RGBA8.
    premultiplied_to_straight_rgba(&instance.buffer)
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Map JSON property values to Slint `Value` and set them on the component.
fn set_properties(component: &ComponentInstance, properties: &HashMap<String, serde_json::Value>) {
    for (key, json_val) in properties {
        let slint_val = json_to_slint_value(json_val);
        if let Err(e) = component.set_property(key, slint_val) {
            tracing::warn!(property = %key, error = %e, "Failed to set Slint property");
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

// Thread-local holding the window adapter that `SlintBackend::create_window_adapter()`
// should return.
thread_local! {
    static CURRENT_WINDOW: RefCell<Option<Rc<dyn WindowAdapter>>> = const { RefCell::new(None) };
}

/// Minimal Slint platform backend.
///
/// Required by Slint's runtime to know where to render.  Set exactly once
/// on the shared Slint thread.
struct SlintBackend;

impl slint::platform::Platform for SlintBackend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        CURRENT_WINDOW.with(|cell| {
            cell.borrow()
                .clone()
                .ok_or_else(|| slint::PlatformError::Other("No current Slint window set".into()))
        })
    }
}
