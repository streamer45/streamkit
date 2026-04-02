// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Standalone Slint UI video source node.
//!
//! Compiles a `.slint` file at init time via `slint-interpreter` and renders
//! RGBA8 frames using the software renderer at a configurable resolution and
//! frame rate.  The node has no inputs — it is a video source, just like
//! `video::colorbars`.  Its output can be wired into the compositor (or any
//! other node that accepts raw video) as a regular layer.
//!
//! ## Threading
//!
//! `slint::platform::set_platform` is process-global, and the types it
//! manages (`MinimalSoftwareWindow`, `ComponentInstance`) are `!Send`
//! (`Rc`-based).  To support multiple `SlintNode` instances in one process
//! without UB, **all** Slint operations are funnelled through a single
//! dedicated `std::thread` (lazily spawned via `OnceLock`).  Each node
//! communicates with this shared thread through tagged work items and
//! per-node result channels.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use async_trait::async_trait;
use schemars::schema_for;
use schemars::JsonSchema;
use serde::Deserialize;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::WindowAdapter;
use slint::{ComponentHandle, LogicalSize, SharedString};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value};
use streamkit_core::control::NodeControlMessage;
use streamkit_core::registry::StaticPins;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::{
    Packet, PacketMetadata, PacketType, PixelFormat, RawVideoFormat, VideoFrame,
};
use streamkit_core::{
    config_helpers, state_helpers, InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

// ── Defaults ────────────────────────────────────────────────────────────────

const fn default_width() -> u32 {
    640
}

const fn default_height() -> u32 {
    480
}

const fn default_fps() -> u32 {
    30
}

const fn default_frame_count() -> u32 {
    0
}

const fn default_keyframe_interval() -> u32 {
    90
}

// ── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the standalone Slint UI video source node.
///
/// Produces RGBA8 frames by rendering a compiled `.slint` component via the
/// software renderer.  Properties can be set at init and updated at runtime
/// via `UpdateParams`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SlintConfig {
    /// Output frame width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output frame height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Output frame rate.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Path to the `.slint` file (must start with `samples/slint/`).
    #[serde(default)]
    pub slint_file: String,
    /// Name of the exported component to instantiate.  When omitted, the
    /// first exported component in the file is used.
    #[serde(default)]
    pub component: Option<String>,
    /// Key-value map of Slint properties to set on the component instance.
    /// Strings → `SharedString`, numbers → `f64`, booleans → `bool`.
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional list of property snapshots to cycle through over time.
    /// Each entry is a partial property map merged on top of `properties`.
    #[serde(default)]
    pub property_keyframes: Vec<HashMap<String, serde_json::Value>>,
    /// Number of frames between keyframe switches (default: 90 ≈ 3 s at 30 fps).
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval: u32,
    /// Total frames to generate.  0 = infinite (real-time pacing).
    #[serde(default = "default_frame_count")]
    pub frame_count: u32,
}

impl Default for SlintConfig {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            slint_file: String::new(),
            component: None,
            properties: HashMap::new(),
            property_keyframes: Vec::new(),
            keyframe_interval: default_keyframe_interval(),
            frame_count: default_frame_count(),
        }
    }
}

impl SlintConfig {
    /// Validate configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns an error string if dimensions are zero, fps is zero, or the
    /// slint file path is invalid.
    fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("width and height must be > 0".to_string());
        }
        if self.fps == 0 {
            return Err("fps must be > 0".to_string());
        }
        validate_slint_asset_path(&self.slint_file)
    }

    /// Merge runtime-changeable fields from an `UpdateParams` payload into
    /// this config.
    ///
    /// Only `properties`, `property_keyframes`, and `keyframe_interval` are
    /// updated.  Immutable init-time fields (`slint_file`, `component`,
    /// `width`, `height`, `fps`, `frame_count`) are preserved from the
    /// original config so that partial JSON payloads (e.g.
    /// `{"properties": {"score": 5}}`) work without re-validating the path.
    fn merge_update(&mut self, update: &Self) {
        self.properties.clone_from(&update.properties);
        self.property_keyframes.clone_from(&update.property_keyframes);
        self.keyframe_interval = update.keyframe_interval;
    }
}

/// Validates that a Slint asset path is safe to read.
///
/// # Errors
///
/// Returns an error string if the path contains traversal sequences or does
/// not start with `samples/slint/`.
fn validate_slint_asset_path(path: &str) -> Result<(), String> {
    if path.contains("..") || !path.starts_with("samples/slint/") {
        return Err(format!(
            "Invalid slint_file: must start with 'samples/slint/' and not contain '..': {path}"
        ));
    }
    Ok(())
}

// ── Shared Slint thread ─────────────────────────────────────────────────────
//
// `slint::platform::set_platform` is process-global and the types it exposes
// (`MinimalSoftwareWindow`, `ComponentInstance`) are `!Send` (`Rc`-based).
// To support multiple `SlintNode` instances without UB, all Slint work is
// funnelled through a single dedicated `std::thread`, lazily spawned on the
// first node's init.
//
// Each node gets a unique `NodeId` (UUID) and communicates with the shared
// thread via tagged work items.  Results are sent back on a per-node
// `tokio::sync::mpsc` channel (using `blocking_send` on the Slint thread
// so the async `run()` method can `.recv().await` without blocking the
// tokio worker thread).

/// Opaque identifier for a node's instance on the shared Slint thread.
type NodeId = uuid::Uuid;

/// Work item sent from a node's async loop to the shared Slint thread.
enum SlintWorkItem {
    /// Register a new node: compile its `.slint` file and create an instance.
    /// The `result_tx` is stored by the shared thread for sending render
    /// results and the init outcome back.
    Register {
        node_id: NodeId,
        config: SlintConfig,
        result_tx: tokio::sync::mpsc::Sender<SlintThreadResult>,
    },
    /// Request a single rendered frame for the given node.
    Render { node_id: NodeId },
    /// Update the config (properties / keyframes) for subsequent renders.
    UpdateConfig { node_id: NodeId, config: SlintConfig },
    /// Unregister a node — drop its instance and result channel.
    Unregister { node_id: NodeId },
}

/// Result sent from the shared Slint thread back to a specific node.
enum SlintThreadResult {
    /// Init succeeded — the node can start rendering.
    InitOk,
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
/// (e.g. resource exhaustion).  This is unrecoverable — the process cannot
/// render Slint UIs without this thread.
#[allow(clippy::expect_used)]
fn shared_slint_thread() -> &'static SlintThreadHandle {
    static HANDLE: OnceLock<SlintThreadHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (work_tx, work_rx) = std::sync::mpsc::channel::<SlintWorkItem>();
        std::thread::Builder::new()
            .name("slint-renderer".to_string())
            .spawn(move || slint_thread_main(work_rx))
            .expect("Failed to spawn shared Slint renderer thread");
        SlintThreadHandle { work_tx }
    })
}

/// Entry point for the shared Slint thread.
///
/// Processes work items from all `SlintNode` instances.  The platform
/// backend is set once on this thread; all `SlintInstance` values live here.
#[allow(clippy::needless_pass_by_value)] // Receiver must be owned by this thread
fn slint_thread_main(work_rx: std::sync::mpsc::Receiver<SlintWorkItem>) {
    /// Per-node state living on the shared thread.
    struct NodeState {
        instance: SlintInstance,
        config: SlintConfig,
        result_tx: tokio::sync::mpsc::Sender<SlintThreadResult>,
    }

    let mut nodes: HashMap<NodeId, NodeState> = HashMap::new();
    let mut platform_set = false;

    while let Ok(work) = work_rx.recv() {
        match work {
            SlintWorkItem::Register { node_id, config, result_tx } => {
                match create_slint_instance(&config, &mut platform_set) {
                    Ok(instance) => {
                        tracing::info!(
                            "Created Slint instance '{}' from '{}'",
                            node_id,
                            config.slint_file
                        );
                        let _ = result_tx.blocking_send(SlintThreadResult::InitOk);
                        nodes.insert(node_id, NodeState { instance, config, result_tx });
                    },
                    Err(e) => {
                        tracing::error!("Failed to create Slint instance '{}': {e}", node_id);
                        let _ = result_tx.blocking_send(SlintThreadResult::InitErr(e.to_string()));
                    },
                }
            },
            SlintWorkItem::Render { node_id } => {
                if let Some(state) = nodes.get_mut(&node_id) {
                    let rgba_data = render_slint_frame(&mut state.instance, &state.config);
                    // If the result channel is closed the node has been dropped;
                    // clean up its state on the next Unregister (or here eagerly).
                    if state
                        .result_tx
                        .blocking_send(SlintThreadResult::Frame { rgba_data })
                        .is_err()
                    {
                        nodes.remove(&node_id);
                    }
                }
            },
            SlintWorkItem::UpdateConfig { node_id, config } => {
                if let Some(state) = nodes.get_mut(&node_id) {
                    state.config = config;
                }
            },
            SlintWorkItem::Unregister { node_id } => {
                nodes.remove(&node_id);
            },
        }
    }
}

// ── Slint rendering internals ───────────────────────────────────────────────
//
// These types and functions are adapted from the former
// `compositor/slint_overlay.rs` module.  They live on the shared Slint
// thread and are `!Send` by design.

/// A compiled Slint component instance ready for per-frame rendering.
///
/// Created once at pipeline init on the shared Slint thread.
/// `!Send` by design — must not leave that thread.
struct SlintInstance {
    window: Rc<MinimalSoftwareWindow>,
    component: ComponentInstance,
    // Kept alive to prevent the compiled Slint component definition from being dropped.
    #[allow(dead_code)]
    definition: ComponentDefinition,
    buffer: Vec<PremultipliedRgbaColor>,
    width: u32,
    /// Frame counter for property keyframe cycling.
    frame_counter: u32,
}

/// Compile a `.slint` file and create a renderable instance.
///
/// Must be called on the shared Slint thread (`!Send` types).
/// The `platform_set` flag tracks whether `set_platform` has already been
/// called — it must happen exactly once per process.
///
/// # Errors
///
/// Returns an error if the `.slint` file cannot be compiled or if no
/// matching component definition is found.
fn create_slint_instance(
    config: &SlintConfig,
    platform_set: &mut bool,
) -> Result<SlintInstance, StreamKitError> {
    let width = config.width;
    let height = config.height;

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
                "Component '{}' not found in '{}'",
                name, config.slint_file
            ))
        })?
    } else {
        // Use the first exported component.
        result.components().next().ok_or_else(|| {
            StreamKitError::Configuration(format!(
                "No exported components in '{}'",
                config.slint_file
            ))
        })?
    };

    // Create the minimal software window.
    // Dimensions are bounded by practical limits, so the u32→f32 precision
    // loss above ~16M is irrelevant here.
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    #[allow(clippy::cast_precision_loss)]
    window.set_size(LogicalSize::new((width as f32).max(1.0), (height as f32).max(1.0)));

    // Set the Slint platform backend exactly once per process.
    // All instances share this thread, so the first call suffices.
    if !*platform_set {
        let _ = slint::platform::set_platform(Box::new(SlintBackend));
        *platform_set = true;
    }

    // Swap in this instance's window so `create_window_adapter()` returns
    // the correct one during `definition.create()` and `component.show()`.
    let window_adapter = window.clone() as Rc<dyn WindowAdapter>;
    CURRENT_WINDOW.with(|cell| *cell.borrow_mut() = Some(window_adapter));

    // Instantiate the component.
    let component = definition.create().map_err(|e| {
        StreamKitError::Configuration(format!("Failed to create Slint component instance: {e}"))
    })?;

    // Set initial properties.
    set_properties(&component, &config.properties);

    // Allocate pixel buffer.
    let pixel_count = (width as usize) * (height as usize);
    let buffer = vec![PremultipliedRgbaColor::default(); pixel_count];

    // Show the component so it becomes visible for rendering.
    component.show().map_err(|e| {
        StreamKitError::Configuration(format!("Failed to show Slint component: {e}"))
    })?;

    Ok(SlintInstance { window, component, definition, buffer, width, frame_counter: 0 })
}

/// Render a single frame from the Slint instance, returning raw RGBA8 data.
///
/// Applies property keyframe cycling and pumps Slint animation timers.
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

    // Pump Slint's internal animation timers so time-based animations
    // (e.g. slide-in transitions) advance on each tick.
    slint::platform::update_timers_and_animations();

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
// should return.  Before calling `definition.create()` / `component.show()`,
// `create_slint_instance` swaps in the correct per-node window so Slint
// associates the component with the right `MinimalSoftwareWindow`.
thread_local! {
    static CURRENT_WINDOW: RefCell<Option<Rc<dyn WindowAdapter>>> = const { RefCell::new(None) };
}

/// Minimal Slint platform backend.
///
/// Required by Slint's runtime to know where to render.  Set exactly once
/// on the shared Slint thread.  Returns whatever window adapter is currently
/// stored in the `CURRENT_WINDOW` thread-local.
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

// ── Node ────────────────────────────────────────────────────────────────────

/// Standalone Slint UI video source node.
///
/// No input pins.  Outputs `PacketType::RawVideo(Rgba8)` on `"out"`.
/// Follows the Ready → Start lifecycle (same as `ColorBarsNode`).
pub struct SlintNode {
    config: SlintConfig,
}

#[async_trait]
impl ProcessorNode for SlintNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Rgba8,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    #[allow(clippy::too_many_lines)]
    async fn run(mut self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Validate config.
        if let Err(e) = self.config.validate() {
            return Err(StreamKitError::Configuration(e));
        }

        let width = self.config.width;
        let height = self.config.height;
        let fps = self.config.fps;
        let frame_count = self.config.frame_count;
        let duration_us = 1_000_000 / u64::from(fps);

        tracing::info!(
            "SlintNode starting: {}x{} @ {} fps, slint_file='{}', frame_count={}",
            width,
            height,
            fps,
            self.config.slint_file,
            frame_count,
        );

        // Source nodes emit Ready state and wait for Start signal.
        state_helpers::emit_ready(&context.state_tx, &node_name);
        tracing::info!("SlintNode ready, waiting for start signal");

        loop {
            match context.control_rx.recv().await {
                Some(NodeControlMessage::Start) => {
                    tracing::info!("SlintNode received start signal");
                    break;
                },
                Some(NodeControlMessage::UpdateParams(_)) => {},
                Some(NodeControlMessage::Shutdown) => {
                    tracing::info!("SlintNode received shutdown before start");
                    return Ok(());
                },
                None => {
                    tracing::warn!("Control channel closed before start signal received");
                    return Ok(());
                },
            }
        }

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // ── Register with the shared Slint thread ───────────────────────
        let node_id = uuid::Uuid::new_v4();
        let thread_handle = shared_slint_thread();

        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<SlintThreadResult>(2);

        if thread_handle
            .work_tx
            .send(SlintWorkItem::Register { node_id, config: self.config.clone(), result_tx })
            .is_err()
        {
            return Err(StreamKitError::Runtime("Shared Slint thread is not running".to_string()));
        }

        // Wait for init result from the shared thread.
        match result_rx.recv().await {
            Some(SlintThreadResult::InitOk) => {
                tracing::info!("SlintNode '{}' registered on shared thread", node_id);
            },
            Some(SlintThreadResult::InitErr(e)) => {
                return Err(StreamKitError::Configuration(format!(
                    "Slint instance creation failed: {e}"
                )));
            },
            Some(SlintThreadResult::Frame { .. }) => {
                return Err(StreamKitError::Runtime(
                    "Unexpected frame result during init".to_string(),
                ));
            },
            None => {
                return Err(StreamKitError::Runtime(
                    "Shared Slint thread channel closed during init".to_string(),
                ));
            },
        }

        // ── Frame generation loop ───────────────────────────────────────
        // Real-time pacing for dynamic (frame_count == 0) mode.
        let mut interval = if frame_count == 0 {
            let period = std::time::Duration::from_micros(duration_us);
            Some(tokio::time::interval(period))
        } else {
            None
        };

        let mut seq: u64 = 0;

        loop {
            // Honour finite frame count.
            if frame_count > 0 && seq >= u64::from(frame_count) {
                tracing::info!("SlintNode finished after {seq} frames");
                break;
            }

            // Check cancellation.
            if let Some(token) = &context.cancellation_token {
                if token.is_cancelled() {
                    tracing::info!("SlintNode cancelled after {seq} frames");
                    break;
                }
            }

            // Pace in real-time mode.
            if let Some(ref mut iv) = interval {
                tokio::select! {
                    _ = iv.tick() => {},
                    Some(msg) = context.control_rx.recv() => {
                        match msg {
                            NodeControlMessage::Shutdown => {
                                tracing::info!("SlintNode received shutdown during generation");
                                break;
                            },
                            NodeControlMessage::UpdateParams(params) => {
                                if let Ok(update) = serde_json::from_value::<SlintConfig>(params) {
                                    self.config.merge_update(&update);
                                    let _ = thread_handle.work_tx.send(
                                        SlintWorkItem::UpdateConfig { node_id, config: self.config.clone() },
                                    );
                                }
                            },
                            NodeControlMessage::Start => {},
                        }
                        continue;
                    }
                }
            }

            // In batch mode, still check for shutdown.
            if interval.is_none() {
                if let Ok(msg) = context.control_rx.try_recv() {
                    match msg {
                        NodeControlMessage::Shutdown => {
                            tracing::info!("SlintNode received shutdown during batch generation");
                            break;
                        },
                        NodeControlMessage::UpdateParams(params) => {
                            if let Ok(update) = serde_json::from_value::<SlintConfig>(params) {
                                self.config.merge_update(&update);
                                let _ = thread_handle.work_tx.send(SlintWorkItem::UpdateConfig {
                                    node_id,
                                    config: self.config.clone(),
                                });
                            }
                        },
                        NodeControlMessage::Start => {},
                    }
                }
            }

            // Request a frame from the shared thread.
            if thread_handle.work_tx.send(SlintWorkItem::Render { node_id }).is_err() {
                tracing::error!("Shared Slint thread exited unexpectedly");
                break;
            }

            // Wait for the rendered frame.
            let rgba_data = match result_rx.recv().await {
                Some(SlintThreadResult::Frame { rgba_data }) => rgba_data,
                Some(_) => {
                    tracing::warn!("Unexpected result from shared Slint thread");
                    continue;
                },
                None => {
                    tracing::error!("Shared Slint thread result channel closed");
                    break;
                },
            };

            let timestamp_us = seq * duration_us;
            let metadata = Some(PacketMetadata {
                timestamp_us: Some(timestamp_us),
                duration_us: Some(duration_us),
                sequence: Some(seq),
                keyframe: Some(true),
            });

            let frame = if let Some(pool) = &context.video_pool {
                let mut pooled = pool.get(rgba_data.len());
                pooled.as_mut_slice().copy_from_slice(&rgba_data);
                VideoFrame::from_pooled(width, height, PixelFormat::Rgba8, pooled, metadata)?
            } else {
                VideoFrame::with_metadata(width, height, PixelFormat::Rgba8, rgba_data, metadata)?
            };

            if context.output_sender.send("out", Packet::Video(frame)).await.is_err() {
                tracing::debug!("Output channel closed, stopping SlintNode");
                break;
            }

            stats_tracker.sent();
            stats_tracker.maybe_send();
            seq += 1;
        }

        // Unregister from the shared thread so it can drop our instance.
        let _ = thread_handle.work_tx.send(SlintWorkItem::Unregister { node_id });

        stats_tracker.force_send();
        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
        Ok(())
    }
}

// ── Registration ────────────────────────────────────────────────────────────

#[allow(clippy::expect_used, clippy::missing_panics_doc)]
pub fn register_slint_nodes(registry: &mut NodeRegistry) {
    let default_node = SlintNode {
        config: SlintConfig {
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            slint_file: String::new(),
            component: None,
            properties: HashMap::new(),
            property_keyframes: Vec::new(),
            keyframe_interval: default_keyframe_interval(),
            frame_count: default_frame_count(),
        },
    };

    registry.register_static_with_description(
        "video::slint",
        |params| {
            let config: SlintConfig = config_helpers::parse_config_optional(params)?;
            if let Err(e) = config.validate() {
                return Err(StreamKitError::Configuration(e));
            }
            Ok(Box::new(SlintNode { config }))
        },
        serde_json::to_value(schema_for!(SlintConfig))
            .expect("SlintConfig schema should serialize to JSON"),
        StaticPins { inputs: default_node.input_pins(), outputs: default_node.output_pins() },
        vec!["video".to_string(), "generators".to_string()],
        false,
        "Renders a Slint UI component into RGBA8 video frames. \
         Compiles a .slint file at init and produces frames at the configured \
         resolution and frame rate. Properties can be updated at runtime via UpdateParams.",
    );
}
