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
//!
//! ## Hardening
//!
//! - Work item handlers are wrapped in `catch_unwind` so a panic in one
//!   node does not bring down the shared thread (and all other nodes).
//! - Page load errors are detected via timeout when `LoadStatus::Complete`
//!   is not received within the configured deadline.
//! - Each instance caches the last successfully rendered frame; on render
//!   failures the cached frame is returned instead of a blank.
//! - Frame render timing is emitted via `tracing` for observability.

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
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
    /// Resize the output dimensions (from compositor upstream hint).
    Resize { node_id: NodeId, width: u32, height: u32 },
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

/// Minimal delegate that drives the compositor and tracks load status.
///
/// The critical contract is calling `webview.paint()` inside
/// `notify_new_frame_ready` -- without it the software rendering context's
/// framebuffer never receives pixels.
#[derive(Default)]
struct FrameDelegate {
    loaded: Cell<bool>,
    load_failed: Cell<bool>,
    frames: Cell<u64>,
}

impl WebViewDelegate for FrameDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        match status {
            LoadStatus::Complete => {
                self.loaded.set(true);
                self.load_failed.set(false);
            },
            // Servo 0.1.0 does not expose a Failed variant.  Load failures
            // are detected via timeout (page never reaches Complete).
            _ => {},
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
    /// Cached last successfully rendered frame for resilience.
    last_good_frame: Option<Vec<u8>>,
    /// Cumulative render count for metrics.
    render_count: u64,
    /// Sum of render durations for average calculation.
    render_duration_sum: Duration,
}

/// Entry point for the shared Servo thread.
///
/// Creates a single process-global `Servo` instance on first registration
/// and processes work items from all plugin instances.  Each handler is
/// wrapped in `catch_unwind` so a panic in one node does not terminate the
/// shared thread.
#[allow(clippy::needless_pass_by_value)] // Receiver must be moved into the thread entry point
fn servo_thread_main(work_rx: std::sync::mpsc::Receiver<ServoWorkItem>) {
    let mut instances: HashMap<NodeId, InstanceState> = HashMap::new();
    // Servo's Opts is a process-global singleton -- we lazily create the
    // single Servo instance on the first Register and keep it alive for
    // the lifetime of the thread.
    let mut servo: Option<Servo> = None;

    while let Ok(work) = work_rx.recv() {
        match work {
            ServoWorkItem::Register { node_id, config, result_tx } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_register(&mut instances, &mut servo, node_id, config, result_tx);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!(
                        node_id = %node_id,
                        error = %msg,
                        "Panic during Servo Register — instance not created",
                    );
                }
            },
            ServoWorkItem::Render { node_id } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_render(&mut instances, servo.as_ref(), &node_id);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!(
                        node_id = %node_id,
                        error = %msg,
                        "Panic during Servo Render — sending fallback frame",
                    );
                    send_fallback_frame(&mut instances, &node_id);
                }
            },
            ServoWorkItem::UpdateConfig { node_id, config } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_update_config(&mut instances, servo.as_ref(), &node_id, &config);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!(
                        node_id = %node_id,
                        error = %msg,
                        "Panic during Servo UpdateConfig — config not applied",
                    );
                }
            },
            ServoWorkItem::Resize { node_id, width, height } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_resize(&mut instances, &node_id, width, height);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!(
                        node_id = %node_id,
                        error = %msg,
                        "Panic during Servo Resize",
                    );
                }
            },
            ServoWorkItem::Unregister { node_id } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    if let Some(state) = instances.remove(&node_id) {
                        if state.render_count > 0 {
                            let avg_us = state.render_duration_sum.as_micros()
                                / u128::from(state.render_count);
                            tracing::info!(
                                node_id = %node_id,
                                total_frames = state.render_count,
                                avg_render_us = avg_us,
                                "Unregistered Servo instance",
                            );
                        }
                    }
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!(
                        node_id = %node_id,
                        error = %msg,
                        "Panic during Servo Unregister — instance may leak",
                    );
                }
            },
        }
    }

    // ── Graceful shutdown ───────────────────────────────────────────────
    //
    // When all senders are dropped the recv() loop exits.  We must drop
    // every WebView *before* dropping the Servo instance so that each
    // WebView's Drop impl can send `CloseWebView` to the constellation
    // while it is still alive.  After clearing instances we pump the
    // event loop so Servo can process the close messages, avoiding the
    // "pthread_mutex_destroy failed: Device or resource busy" error
    // from SpiderMonkey's mutex teardown.
    let count = instances.len();
    instances.clear();
    if let Some(ref s) = servo {
        // Pump the event loop a few times to let the constellation
        // process the WebView close messages.
        for _ in 0..20 {
            s.spin_event_loop();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    tracing::info!(
        instances_cleared = count,
        "Servo thread shutting down gracefully",
    );
    // `servo` is dropped here -- its Drop impl sends Exit and spins
    // until the constellation finishes shutting down.
}

/// Extract a human-readable message from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Send a fallback frame (last good frame or transparent) after a panic.
fn send_fallback_frame(instances: &mut HashMap<NodeId, InstanceState>, node_id: &NodeId) {
    let Some(state) = instances.get(node_id) else {
        return;
    };
    let fallback = if let Some(ref cached) = state.last_good_frame {
        cached.clone()
    } else {
        let len = (state.config.width as usize) * (state.config.height as usize) * 4;
        vec![0u8; len]
    };
    let _ = state.result_tx.send(ServoThreadResult::Frame { rgba_data: fallback });
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

    let load_timeout = Duration::from_secs(u64::from(config.load_timeout_secs));

    match create_webview(servo_ref, &config) {
        Ok((webview, rendering_context, delegate)) => {
            // Wait for the initial page load so the first Render has content.
            let load_start = Instant::now();
            wait_for_load(servo_ref, &delegate, &config.url, &node_id, load_timeout);
            let load_duration = load_start.elapsed();

            if delegate.load_failed.get() {
                tracing::warn!(
                    node_id = %node_id,
                    url = %config.url,
                    load_ms = load_duration.as_millis(),
                    "Page load reported failure — proceeding with partial content",
                );
            } else if !delegate.loaded.get() {
                tracing::warn!(
                    node_id = %node_id,
                    url = %config.url,
                    load_ms = load_duration.as_millis(),
                    "Page load timed out — proceeding with partial content",
                );
            } else {
                tracing::info!(
                    node_id = %node_id,
                    url = %config.url,
                    load_ms = load_duration.as_millis(),
                    "Page loaded successfully",
                );
            }

            // Force at least one post-load frame via a rAF nudge.
            nudge_frame(servo_ref, &webview, &delegate);

            // Inject custom CSS if provided.
            if let Some(ref css) = config.custom_css {
                inject_custom_css(&webview, servo_ref, css);
            }

            tracing::info!(
                node_id = %node_id,
                url = %config.url,
                output = %format_args!("{}x{}", config.width, config.height),
                viewport = %format_args!("{}x{}", config.effective_viewport_width(), config.effective_viewport_height()),
                scaling = config.needs_scaling(),
                "Created Servo WebView on shared instance",
            );

            let _ = result_tx.send(ServoThreadResult::InitOk);
            instances.insert(
                node_id,
                InstanceState {
                    webview,
                    rendering_context,
                    delegate,
                    config,
                    result_tx,
                    last_good_frame: None,
                    render_count: 0,
                    render_duration_sum: Duration::ZERO,
                },
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

    let render_start = Instant::now();

    // Pump the event loop to let Servo process pending work.
    servo.spin_event_loop();

    // Read pixels from the rendering context at viewport resolution.
    let vw = state.config.effective_viewport_width();
    let vh = state.config.effective_viewport_height();
    let rect = Box2D::new(
        Point2D::new(0, 0),
        Point2D::new(
            i32::try_from(vw).unwrap_or(i32::MAX),
            i32::try_from(vh).unwrap_or(i32::MAX),
        ),
    );

    let rgba_data = if let Some(img) = state.rendering_context.read_to_image(rect) {
        // Scale to output dimensions if viewport differs from output.
        let raw = if state.config.needs_scaling() {
            let scaled = image::imageops::resize(
                &img,
                state.config.width,
                state.config.height,
                image::imageops::FilterType::Triangle,
            );
            scaled.into_raw()
        } else {
            img.into_raw()
        };
        state.last_good_frame = Some(raw.clone());
        raw
    } else if let Some(ref cached) = state.last_good_frame {
        tracing::debug!(node_id = %node_id, "read_to_image returned None, using cached frame");
        cached.clone()
    } else {
        // No cached frame — send transparent at output resolution.
        let len = (state.config.width as usize) * (state.config.height as usize) * 4;
        vec![0u8; len]
    };

    let render_duration = render_start.elapsed();
    state.render_count += 1;
    state.render_duration_sum += render_duration;

    // Log render time periodically (every 300 frames ~ 10s at 30fps).
    if state.render_count % 300 == 0 {
        let avg_us =
            state.render_duration_sum.as_micros() / u128::from(state.render_count);
        tracing::debug!(
            node_id = %node_id,
            frame = state.render_count,
            render_us = render_duration.as_micros(),
            avg_render_us = avg_us,
            "Servo render metrics",
        );
    }

    if state.result_tx.send(ServoThreadResult::Frame { rgba_data }).is_err() {
        instances.remove(node_id);
    }
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

    if url_changed {
        if let Ok(parsed) = url::Url::parse(&new_config.url) {
            state.webview.load(parsed);
            state.delegate.loaded.set(false);
            state.delegate.load_failed.set(false);

            let load_timeout =
                Duration::from_secs(u64::from(new_config.load_timeout_secs));
            let load_start = Instant::now();
            wait_for_load(servo, &state.delegate, &new_config.url, node_id, load_timeout);

            if state.delegate.load_failed.get() {
                tracing::warn!(
                    node_id = %node_id,
                    url = %new_config.url,
                    load_ms = load_start.elapsed().as_millis(),
                    "URL navigation load failed",
                );
            } else if !state.delegate.loaded.get() {
                tracing::warn!(
                    node_id = %node_id,
                    url = %new_config.url,
                    load_ms = load_start.elapsed().as_millis(),
                    "URL navigation load timed out",
                );
            }
        }
    }

    if css_changed || url_changed {
        let css = if css_changed {
            new_config.custom_css.as_deref()
        } else {
            state.config.custom_css.as_deref()
        };
        if let Some(css) = css {
            inject_custom_css(&state.webview, servo, css);
        }
    }

    // Commit the config only after all side effects have succeeded.
    // This ensures that on panic (caught by catch_unwind in the caller),
    // state.config still reflects the actual webview state, allowing
    // retries with the same URL to trigger navigation again.
    state.config.merge_update(new_config);
}

/// Handle a `Resize` work item: update output dimensions from a compositor
/// upstream hint.  Only the output (frame) dimensions change; the Servo
/// viewport remains the same so the page layout is unaffected.  This means
/// the scaling ratio may change, but the page doesn't reflow.
fn handle_resize(
    instances: &mut HashMap<NodeId, InstanceState>,
    node_id: &NodeId,
    width: u32,
    height: u32,
) {
    let Some(state) = instances.get_mut(node_id) else {
        return;
    };
    if state.config.width == width && state.config.height == height {
        return;
    }
    tracing::info!(
        node_id = %node_id,
        old = %format_args!("{}x{}", state.config.width, state.config.height),
        new = %format_args!("{width}x{height}"),
        "Resized Servo output via upstream hint",
    );
    state.config.width = width;
    state.config.height = height;
    // Invalidate the cached frame since dimensions changed.
    state.last_good_frame = None;
}

/// Create a `WebView` with its own `SoftwareRenderingContext` on the shared
/// Servo instance.  The rendering context uses the *viewport* dimensions
/// (which may be larger than the output frame), so the page layout has
/// room to breathe.  Scaling to output dimensions happens in `handle_render`.
fn create_webview(
    servo: &Servo,
    config: &ServoConfig,
) -> Result<(WebView, Rc<SoftwareRenderingContext>, Rc<FrameDelegate>), String> {
    let vw = config.effective_viewport_width();
    let vh = config.effective_viewport_height();
    let size = PhysicalSize::new(vw, vh);
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

/// Wait for the page to reach `LoadStatus::Complete`, with a configurable
/// timeout.  Returns when the delegate's `loaded` flag is set, or on timeout.
fn wait_for_load(
    servo: &Servo,
    delegate: &FrameDelegate,
    url: &str,
    node_id: &NodeId,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while !delegate.loaded.get() {
        if Instant::now() > deadline {
            tracing::warn!(
                node_id = %node_id,
                url = %url,
                timeout_secs = timeout.as_secs(),
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
            let s = document.getElementById('__streamkit_custom_css'); \
            if (!s) {{ s = document.createElement('style'); s.id = '__streamkit_custom_css'; document.head.appendChild(s); }} \
            s.textContent = '{escaped}'; \
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
