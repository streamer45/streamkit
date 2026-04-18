// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared Servo renderer thread.
//!
//! Servo types (`Servo`, `WebView`, `SoftwareRenderingContext`) are `!Send`
//! and `!Sync` (`Rc`-based).  To support multiple plugin instances without
//! UB, all Servo work is funnelled through a single dedicated `std::thread`,
//! lazily spawned on the first instance's init.
//!
//! Servo's `Opts` is a **process-global singleton** — only one `Servo`
//! instance can exist.  Multiple nodes share this single `Servo` instance,
//! each with their own `SoftwareRenderingContext` and `WebView`.
//!
//! Each instance gets a unique `NodeId` (UUID) and communicates with the
//! shared thread via tagged work items.  Results are sent back on per-node
//! `std::sync::mpsc` channels so `tick()` can block-receive without needing
//! a tokio runtime.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use euclid::{Box2D, Point2D, Scale};
use servo::{
    LoadStatus, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate,
};

use crate::config::ServoConfig;

/// Opaque identifier for a plugin instance on the shared Servo thread.
pub type NodeId = uuid::Uuid;

/// Work item sent from a plugin's `tick()` to the shared Servo thread.
pub enum ServoWorkItem {
    /// Register a new instance: create a WebView on the shared Servo and
    /// navigate to URL.  The `result_tx` is stored by the shared thread for
    /// sending render results and the init outcome back.
    Register {
        node_id: NodeId,
        config: ServoConfig,
        result_tx: std::sync::mpsc::SyncSender<ServoThreadResult>,
    },
    /// Request a single rendered frame for the given instance.
    Render { node_id: NodeId },
    /// Update the config (URL) for subsequent renders.
    UpdateConfig { node_id: NodeId, config: ServoConfig },
    /// Unregister an instance -- drop its WebView and result channel.
    Unregister { node_id: NodeId },
}

/// Result sent from the shared Servo thread back to a specific instance.
pub enum ServoThreadResult {
    /// Init succeeded -- the instance can start rendering.
    InitOk,
    /// Init failed with an error message.
    InitErr(String),
    /// A rendered frame.
    Frame { rgba_data: Vec<u8> },
}

/// Handle to the shared Servo thread's work channel.
struct ServoThreadHandle {
    work_tx: std::sync::mpsc::Sender<ServoWorkItem>,
}

/// Get (or lazily spawn) the shared Servo thread.
///
/// # Panics
///
/// Panics if the OS fails to spawn the dedicated Servo renderer thread
/// (e.g. resource exhaustion).  This is unrecoverable -- the plugin cannot
/// render web pages without this thread.
#[allow(clippy::expect_used)]
fn shared_servo_thread() -> &'static ServoThreadHandle {
    static HANDLE: OnceLock<ServoThreadHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (work_tx, work_rx) = std::sync::mpsc::channel::<ServoWorkItem>();
        std::thread::Builder::new()
            .name("servo-plugin-renderer".to_string())
            .spawn(move || servo_thread_main(work_rx))
            .expect("Failed to spawn shared Servo renderer thread");
        ServoThreadHandle { work_tx }
    })
}

/// Send a work item to the shared Servo thread.
///
/// # Errors
///
/// Returns an error if the shared thread has panicked or been dropped.
pub fn send_work(item: ServoWorkItem) -> Result<(), String> {
    shared_servo_thread()
        .work_tx
        .send(item)
        .map_err(|_| "Servo renderer thread is no longer running".to_string())
}

// -- Servo thread internals --------------------------------------------------

/// Minimal delegate that drives the compositor.
///
/// The critical contract is calling `webview.paint()` inside
/// `notify_new_frame_ready` -- without it the software rendering context's
/// framebuffer never receives pixels.
#[derive(Default)]
struct FrameDelegate {
    loaded: Cell<bool>,
    frames: Cell<u64>,
}

impl WebViewDelegate for FrameDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.loaded.set(true);
        }
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.frames.set(self.frames.get() + 1);
    }
}

/// Per-instance state living on the shared Servo thread.
///
/// Each instance has its own `SoftwareRenderingContext` and `WebView`,
/// but they all share the single process-global `Servo` instance.
struct InstanceState {
    webview: WebView,
    rendering_context: Rc<SoftwareRenderingContext>,
    delegate: Rc<FrameDelegate>,
    config: ServoConfig,
    result_tx: std::sync::mpsc::SyncSender<ServoThreadResult>,
}

/// Maximum time to wait for the initial page load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Entry point for the shared Servo thread.
///
/// Creates a single process-global `Servo` instance on first registration
/// and processes work items from all plugin instances.
#[allow(clippy::needless_pass_by_value)]
fn servo_thread_main(work_rx: std::sync::mpsc::Receiver<ServoWorkItem>) {
    let mut instances: HashMap<NodeId, InstanceState> = HashMap::new();
    // Servo's Opts is a process-global singleton -- we lazily create the
    // single Servo instance on the first Register and keep it alive for
    // the lifetime of the thread.
    let mut servo: Option<Servo> = None;

    while let Ok(work) = work_rx.recv() {
        match work {
            ServoWorkItem::Register { node_id, config, result_tx } => {
                handle_register(&mut instances, &mut servo, node_id, config, result_tx);
            },
            ServoWorkItem::Render { node_id } => {
                handle_render(&mut instances, servo.as_ref(), &node_id);
            },
            ServoWorkItem::UpdateConfig { node_id, config } => {
                handle_update_config(&mut instances, servo.as_ref(), &node_id, &config);
            },
            ServoWorkItem::Unregister { node_id } => {
                instances.remove(&node_id);
            },
        }
    }
}

/// Handle a `Register` work item: create a WebView on the shared Servo
/// instance (creating the Servo if this is the first registration),
/// navigate to URL, and wait for the initial load.
fn handle_register(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: &mut Option<Servo>,
    node_id: NodeId,
    config: ServoConfig,
    result_tx: std::sync::mpsc::SyncSender<ServoThreadResult>,
) {
    // Create the process-global Servo instance on first use.
    let servo_ref = servo.get_or_insert_with(|| {
        let prefs = servo::Preferences {
            network_http_proxy_uri: String::new(),
            network_https_proxy_uri: String::new(),
            ..servo::Preferences::default()
        };
        let s: Servo = ServoBuilder::default().preferences(prefs).build();
        s.setup_logging();
        s
    });

    match create_webview(servo_ref, &config) {
        Ok((webview, rendering_context, delegate)) => {
            // Wait for the initial page load so the first Render has content.
            wait_for_load(servo_ref, &delegate, &config.url, &node_id);

            // Force at least one post-load frame via a rAF nudge.
            nudge_frame(servo_ref, &webview, &delegate);

            // Inject custom CSS if provided.
            if let Some(ref css) = config.custom_css {
                inject_custom_css(&webview, servo_ref, css);
            }

            tracing::info!(
                node_id = %node_id,
                url = %config.url,
                width = config.width,
                height = config.height,
                "Created Servo WebView on shared instance",
            );

            let _ = result_tx.send(ServoThreadResult::InitOk);
            instances.insert(
                node_id,
                InstanceState { webview, rendering_context, delegate, config, result_tx },
            );
        },
        Err(e) => {
            tracing::error!(node_id = %node_id, error = %e, "Failed to create Servo WebView");
            let _ = result_tx.send(ServoThreadResult::InitErr(e));
        },
    }
}

/// Handle a `Render` work item: pump the event loop and read pixels.
fn handle_render(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: Option<&Servo>,
    node_id: &NodeId,
) {
    let (Some(state), Some(servo)) = (instances.get_mut(node_id), servo) else {
        return;
    };

    // Pump the event loop to let Servo process pending work.
    servo.spin_event_loop();

    // Read pixels from the rendering context.
    let rect = Box2D::new(
        Point2D::new(0, 0),
        Point2D::new(
            i32::try_from(state.config.width).unwrap_or(i32::MAX),
            i32::try_from(state.config.height).unwrap_or(i32::MAX),
        ),
    );

    let rgba_data = if let Some(img) = state.rendering_context.read_to_image(rect) {
        img.into_raw()
    } else {
        // Fallback: send a transparent frame.
        let len = (state.config.width as usize) * (state.config.height as usize) * 4;
        vec![0u8; len]
    };

    let _ = state.result_tx.send(ServoThreadResult::Frame { rgba_data });
}

/// Handle an `UpdateConfig` work item: navigate to a new URL if changed.
fn handle_update_config(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: Option<&Servo>,
    node_id: &NodeId,
    new_config: &ServoConfig,
) {
    let (Some(state), Some(servo)) = (instances.get_mut(node_id), servo) else {
        return;
    };

    let url_changed = new_config.url != state.config.url && !new_config.url.is_empty();
    let css_changed = new_config.custom_css != state.config.custom_css;

    state.config.merge_update(new_config);

    if url_changed {
        if let Ok(parsed) = url::Url::parse(&state.config.url) {
            state.webview.load(parsed);
            state.delegate.loaded.set(false);

            wait_for_load(servo, &state.delegate, &state.config.url, node_id);
        }
    }

    if css_changed {
        if let Some(ref css) = state.config.custom_css {
            inject_custom_css(&state.webview, servo, css);
        }
    }
}

/// Create a `WebView` with its own `SoftwareRenderingContext` on the shared
/// Servo instance.
fn create_webview(
    servo: &Servo,
    config: &ServoConfig,
) -> Result<(WebView, Rc<SoftwareRenderingContext>, Rc<FrameDelegate>), String> {
    let size = PhysicalSize::new(config.width, config.height);
    let rendering_context: Rc<SoftwareRenderingContext> = Rc::new(
        SoftwareRenderingContext::new(size)
            .map_err(|e| format!("Failed to create SoftwareRenderingContext: {e:?}"))?,
    );

    let _ = rendering_context.make_current();

    let delegate: Rc<FrameDelegate> = Rc::new(FrameDelegate::default());

    let parsed_url =
        url::Url::parse(&config.url).map_err(|e| format!("Invalid URL '{}': {e}", config.url))?;

    let webview: WebView = WebViewBuilder::new(servo, rendering_context.clone())
        .url(parsed_url)
        .hidpi_scale_factor(Scale::new(1.0))
        .delegate(delegate.clone() as Rc<dyn WebViewDelegate>)
        .build();

    Ok((webview, rendering_context, delegate))
}

/// Wait for the page to reach `LoadStatus::Complete`, with a timeout.
fn wait_for_load(servo: &Servo, delegate: &FrameDelegate, url: &str, node_id: &NodeId) {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while !delegate.loaded.get() {
        if Instant::now() > deadline {
            tracing::warn!(
                node_id = %node_id,
                url = %url,
                "Timed out waiting for page load, proceeding anyway",
            );
            break;
        }
        servo.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Force a post-load frame by triggering a `requestAnimationFrame` nudge
/// and waiting for a new frame to be painted.
fn nudge_frame(servo: &Servo, webview: &WebView, delegate: &FrameDelegate) {
    let js_done = Rc::new(Cell::new(false));
    {
        let js_done_inner = js_done.clone();
        webview.evaluate_javascript(
            "new Promise(r => requestAnimationFrame(() => { \
             document.documentElement.getBoundingClientRect(); r(); \
             }))",
            move |_r| js_done_inner.set(true),
        );
    }
    let js_deadline = Instant::now() + Duration::from_secs(5);
    while !js_done.get() {
        if Instant::now() > js_deadline {
            break;
        }
        servo.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }

    // Wait for at least one post-load frame_ready.
    let frames_at_load = delegate.frames.get();
    let frame_deadline = Instant::now() + Duration::from_secs(5);
    while delegate.frames.get() <= frames_at_load {
        if Instant::now() > frame_deadline {
            break;
        }
        servo.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Inject custom CSS into a loaded page via JavaScript.
fn inject_custom_css(webview: &WebView, servo: &Servo, css: &str) {
    let escaped =
        css.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n").replace('\r', "\\r");
    let js = format!(
        "(() => {{ \
            const s = document.createElement('style'); \
            s.textContent = '{escaped}'; \
            document.head.appendChild(s); \
        }})()"
    );
    let done = Rc::new(Cell::new(false));
    let done_inner = done.clone();
    webview.evaluate_javascript(&js, move |_| done_inner.set(true));

    let deadline = Instant::now() + Duration::from_secs(2);
    while !done.get() {
        if Instant::now() > deadline {
            break;
        }
        servo.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
}
