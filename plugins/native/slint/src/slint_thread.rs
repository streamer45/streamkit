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
use slint::{ComponentHandle, LogicalSize, PhysicalSize, SharedString};
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

/// A property pair discovered at registration: a source property
/// `{name}` and its corresponding `prev-{name}`.  When the source
/// property changes in an `UpdateConfig`, the old value is
/// automatically written to the prev property.
struct TrackedProp {
    /// Source property name (snake_case), e.g. `"text"`.
    source: String,
    /// Prev property name (snake_case), e.g. `"prev_text"`.
    prev: String,
    /// Type-appropriate default when the source has no prior value.
    default_value: serde_json::Value,
}

/// Per-instance state living on the shared Slint thread.
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
    /// Original configured dimensions from init.  Used to compute
    /// the DPI scale factor when upstream resize hints request
    /// different physical dimensions — content is rendered at the
    /// original logical proportions but at higher physical
    /// resolution for crisper text and vector graphics.
    original_width: u32,
    original_height: u32,
    /// Properties tracked for automatic previous-value injection.
    /// Populated at registration when the component declares a
    /// `prev-{name}` property alongside `{name}` with a matching
    /// type (opt-in by the `.slint` author).
    tracked_props: Vec<TrackedProp>,
    /// Auto-incrementing revision counter.  `Some(n)` when the
    /// component declares a `revision` (number) property; bumped
    /// whenever at least one tracked property changes value.
    revision: Option<i64>,
}

/// Entry point for the shared Slint thread.
///
/// Processes work items from all plugin instances.  The platform backend is
/// set once on this thread; all `SlintInstance` values live here.
#[allow(clippy::needless_pass_by_value)]
fn slint_thread_main(work_rx: std::sync::mpsc::Receiver<SlintWorkItem>) {
    let mut instances: HashMap<NodeId, InstanceState> = HashMap::new();
    let mut platform_set = false;

    while let Ok(work) = work_rx.recv() {
        match work {
            SlintWorkItem::Register { node_id, config, result_tx } => {
                handle_register(&mut instances, &mut platform_set, node_id, config, result_tx);
            },
            SlintWorkItem::Render { node_id } => {
                handle_render(&mut instances, &node_id);
            },
            SlintWorkItem::UpdateConfig { node_id, config } => {
                if let Some(state) = instances.get_mut(&node_id) {
                    state.apply_config_update(&node_id, config);
                }
            },
            SlintWorkItem::Resize { node_id, width, height } => {
                if let Some(state) = instances.get_mut(&node_id) {
                    state.apply_resize(&node_id, width, height);
                }
            },
            SlintWorkItem::Unregister { node_id } => {
                instances.remove(&node_id);
            },
        }
    }
}

/// Handle a `Register` work item: compile the `.slint` file, discover
/// properties, and insert the instance into the map.
fn handle_register(
    instances: &mut HashMap<NodeId, InstanceState>,
    platform_set: &mut bool,
    node_id: NodeId,
    config: SlintConfig,
    result_tx: std::sync::mpsc::SyncSender<SlintThreadResult>,
) {
    match create_slint_instance(&config, platform_set) {
        Ok(instance) => {
            let properties = discover_properties(&instance.definition, &instance.component);
            let tracked_props = discover_tracked_props(&instance.definition);
            let revision = if instance.definition.properties().any(|(n, _)| n == "revision") {
                Some(0i64)
            } else {
                None
            };

            tracing::info!(
                node_id = %node_id,
                slint_file = %config.slint_file,
                discovered_properties = properties.len(),
                tracked_pairs = tracked_props.len(),
                has_revision = revision.is_some(),
                "Created Slint instance",
            );
            let _ = result_tx.send(SlintThreadResult::InitOk { properties });
            instances.insert(
                node_id,
                InstanceState {
                    instance,
                    original_width: config.width,
                    original_height: config.height,
                    config,
                    result_tx,
                    cached_frame: None,
                    cached_keyframe_idx: None,
                    dirty: true,
                    tracked_props,
                    revision,
                },
            );
        },
        Err(e) => {
            tracing::error!(node_id = %node_id, error = %e, "Failed to create Slint instance");
            let _ = result_tx.send(SlintThreadResult::InitErr(e));
        },
    }
}

/// Handle a `Render` work item: produce a frame (from cache or fresh render)
/// and send it back on the result channel.
fn handle_render(instances: &mut HashMap<NodeId, InstanceState>, node_id: &NodeId) {
    let Some(state) = instances.get_mut(node_id) else { return };

    // Pump Slint timers/animations (process-global) so Timer callbacks
    // and CSS-like transitions advance even when the frame is served
    // from cache.
    slint::platform::update_timers_and_animations();

    let rgba_data = if state.config.static_ui {
        let kf_idx = if state.config.property_keyframes.is_empty() {
            None
        } else {
            let interval = state.config.keyframe_interval.max(1);
            Some(
                (state.instance.frame_counter / interval) as usize
                    % state.config.property_keyframes.len(),
            )
        };

        let need_render =
            state.dirty || state.cached_keyframe_idx != kf_idx || state.cached_frame.is_none();

        if need_render {
            let data = render_slint_frame(&mut state.instance, &state.config);
            state.cached_frame = Some(data);
            state.cached_keyframe_idx = kf_idx;
            state.dirty = false;
        } else {
            state.instance.frame_counter = state.instance.frame_counter.wrapping_add(1);
        }
        state.cached_frame.clone().unwrap_or_default()
    } else {
        render_slint_frame(&mut state.instance, &state.config)
    };

    match state.result_tx.try_send(SlintThreadResult::Frame { rgba_data }) {
        Ok(()) => {},
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            tracing::debug!(node_id = %node_id, "Result channel full, dropping frame");
        },
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            instances.remove(node_id);
        },
    }
}

impl InstanceState {
    /// Apply an `UpdateConfig`: inject previous values for tracked
    /// properties, bump the revision counter, and mark the instance dirty.
    fn apply_config_update(&mut self, node_id: &NodeId, mut config: SlintConfig) {
        let mut any_changed = false;
        for tp in &self.tracked_props {
            let new_val = config.properties.get(&tp.source);
            let old_val = self.config.properties.get(&tp.source);
            if new_val.is_some() && new_val != old_val {
                let prev = old_val.cloned().unwrap_or_else(|| tp.default_value.clone());
                tracing::debug!(
                    node_id = %node_id,
                    property = %tp.source,
                    prev = %prev,
                    new = %new_val.unwrap_or(&serde_json::Value::Null),
                    "Tracked property changed, injecting prev value",
                );
                config.properties.insert(tp.prev.clone(), prev);
                any_changed = true;
            }
        }
        if any_changed {
            if let Some(rev) = &mut self.revision {
                *rev += 1;
                tracing::debug!(node_id = %node_id, revision = *rev, "Bumped revision counter");
                config.properties.insert("revision".to_string(), serde_json::json!(*rev));
            }
        }
        tracing::debug!(
            node_id = %node_id,
            properties = ?config.properties.keys().collect::<Vec<_>>(),
            "UpdateConfig applied",
        );
        self.config = config;
        self.dirty = true;
    }

    /// Apply a resize: update dimensions, recompute the DPI scale factor,
    /// and reallocate the rendering buffer.
    fn apply_resize(&mut self, node_id: &NodeId, width: u32, height: u32) {
        if self.instance.width == width && self.instance.height == height {
            return;
        }
        self.instance.width = width;
        self.instance.height = height;
        self.config.width = width;
        self.config.height = height;

        // Compute DPI scale factor so content renders at original logical
        // proportions but higher physical resolution.
        #[allow(clippy::cast_precision_loss)]
        let scale = f32::min(
            width as f32 / self.original_width.max(1) as f32,
            height as f32 / self.original_height.max(1) as f32,
        )
        .max(0.1);

        // Scale must be applied first so Slint computes correct logical
        // coordinates from the physical dimensions.
        self.instance.component.window().dispatch_event(
            slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale },
        );
        self.instance.window.set_size(PhysicalSize::new(width, height));

        let pixel_count = (width as usize) * (height as usize);
        self.instance.buffer = vec![PremultipliedRgbaColor::default(); pixel_count];
        self.cached_frame = None;
        self.dirty = true;
        tracing::info!(
            node_id = %node_id,
            width, height,
            scale_factor = %scale,
            "Resized Slint instance via upstream hint",
        );
    }
}

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

/// Discover `prev-{name}` / `{name}` property pairs for automatic
/// previous-value injection.
///
/// A pair is tracked when the component declares both `{name}` and
/// `prev-{name}` with the same `ValueType`.  Only JSON-representable
/// types (string, number, bool) are supported.  Property names are
/// returned in snake_case to match the StreamKit config convention.
fn discover_tracked_props(definition: &ComponentDefinition) -> Vec<TrackedProp> {
    let prop_map: HashMap<String, ValueType> = definition.properties().collect();
    let mut tracked = Vec::new();
    for (name, vt) in &prop_map {
        if let Some(source) = name.strip_prefix("prev-") {
            if prop_map.get(source) == Some(vt) {
                let default_value = match vt {
                    ValueType::String => serde_json::Value::String(String::new()),
                    ValueType::Number => serde_json::json!(0),
                    ValueType::Bool => serde_json::Value::Bool(false),
                    _ => continue,
                };
                tracked.push(TrackedProp {
                    source: source.replace('-', "_"),
                    prev: name.replace('-', "_"),
                    default_value,
                });
            }
        }
    }
    tracked
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

/// Map JSON property values to Slint `Value` and set them on the component.
///
/// `revision` is always set **last** so that any properties it drives
/// (e.g. crossfade layer selection via `use-a: Math.mod(revision, 2)`)
/// see up-to-date data values (`text`, `prev-text`, etc.) rather than
/// stale ones.  Without this ordering, HashMap iteration might set
/// `revision` before `text`, creating a one-frame flash of old content.
fn set_properties(component: &ComponentInstance, properties: &HashMap<String, serde_json::Value>) {
    let mut deferred_revision = None;
    for (key, json_val) in properties {
        if key == "revision" {
            deferred_revision = Some(json_val);
            continue;
        }
        let slint_val = json_to_slint_value(json_val);
        if let Err(e) = component.set_property(key, slint_val) {
            tracing::warn!(property = %key, error = %e, "Failed to set Slint property");
        }
    }
    if let Some(json_val) = deferred_revision {
        let slint_val = json_to_slint_value(json_val);
        if let Err(e) = component.set_property("revision", slint_val) {
            tracing::warn!(property = "revision", error = %e, "Failed to set Slint property");
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
