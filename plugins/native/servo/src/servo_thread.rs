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
//! - After [`POISON_THRESHOLD`] consecutive render panics an instance is
//!   *poisoned*: Servo calls are skipped entirely and the cached frame is
//!   returned until a URL change (`UpdateConfig`) resets the state.
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

/// Number of consecutive render panics before an instance is marked as
/// poisoned and Servo calls are skipped entirely.
const POISON_THRESHOLD: u32 = 5;

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
///
/// `loaded` is read by `handle_render`'s post-load gate to fire one-shot
/// post-load actions (custom CSS injection) on the first tick after the
/// page reaches `LoadStatus::Complete`.
///
/// `painted` gates surface reads: surfman reuses the underlying
/// `SoftwareRenderingContext` surface across instances, so before Servo
/// has painted the current page at least once the surface may still hold
/// a previous (unrelated) capture's pixels.  Until the first paint,
/// `handle_render` emits a fully transparent frame rather than leaking
/// stale pixels.  Reset on URL change in `handle_update_config`.
#[derive(Default)]
struct FrameDelegate {
    loaded: Cell<bool>,
    painted: Cell<bool>,
}

impl WebViewDelegate for FrameDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.loaded.set(true);
        }
        // Servo 0.1.0 does not expose a Failed variant.  Load failures
        // would manifest as the page never reaching Complete; there is
        // no synchronous wait at register time, so failures simply
        // leave the page in its loading state and `handle_render`
        // returns whatever has been painted.
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.painted.set(true);
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
    /// Current width of the `SoftwareRenderingContext` — the size Servo
    /// actually renders at.  Stays constant when only the output dimensions
    /// change (via `Resize` hints), but is updated when the viewport
    /// resolution changes via `UpdateConfig` (which calls `WebView::resize`
    /// and therefore resizes the underlying `RenderingContext`).
    rc_width: u32,
    /// Current height of the `SoftwareRenderingContext`.  See `rc_width`.
    rc_height: u32,
    /// Cached last successfully rendered frame for resilience.
    last_good_frame: Option<Vec<u8>>,
    /// Cumulative render count for metrics.
    render_count: u64,
    /// Sum of render durations for average calculation.
    render_duration_sum: Duration,
    /// Number of consecutive render panics for this instance.
    consecutive_panic_count: u32,
    /// When `true`, Servo calls are skipped and the cached frame is
    /// returned directly.  Set after [`POISON_THRESHOLD`] consecutive
    /// render panics; cleared on URL change (`UpdateConfig`).
    poisoned: bool,
    /// Tracks whether one-shot post-load work (custom CSS injection) has
    /// been performed for the current URL.  Set to `true` after we've
    /// observed `LoadStatus::Complete` and run the post-load actions.
    /// Reset on `UpdateConfig` when the URL changes so the new page
    /// gets the same treatment.
    post_load_done: bool,
}

/// Entry point for the shared Servo thread.
///
/// Creates a single process-global `Servo` instance on first registration
/// and processes work items from all plugin instances.  Each handler is
/// wrapped in `catch_unwind` so a panic in one node does not terminate the
/// shared thread.
#[allow(clippy::needless_pass_by_value)] // Receiver must be moved into the thread entry point
#[allow(clippy::cognitive_complexity)] // Main event loop — splitting would obscure control flow
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
                // Poisoned instances skip Servo entirely and return the
                // cached frame until a URL change resets the state.
                if instances.get(&node_id).is_some_and(|s| s.poisoned) {
                    send_fallback_frame(&instances, &node_id);
                    continue;
                }

                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_render(&mut instances, servo.as_ref(), &node_id);
                }));
                match result {
                    Ok(()) => {
                        if let Some(state) = instances.get_mut(&node_id) {
                            state.consecutive_panic_count = 0;
                        }
                    },
                    Err(panic) => {
                        let msg = panic_message(&panic);
                        tracing::error!(
                            node_id = %node_id,
                            error = %msg,
                            "Panic during Servo Render — sending fallback frame",
                        );
                        if let Some(state) = instances.get_mut(&node_id) {
                            state.consecutive_panic_count += 1;
                            if state.consecutive_panic_count >= POISON_THRESHOLD {
                                state.poisoned = true;
                                tracing::error!(
                                    node_id = %node_id,
                                    consecutive_panics = state.consecutive_panic_count,
                                    "Servo instance poisoned after {} consecutive render \
                                     panics — skipping Servo calls until URL change",
                                    POISON_THRESHOLD,
                                );
                            }
                        }
                        send_fallback_frame(&instances, &node_id);
                    },
                }
            },
            ServoWorkItem::UpdateConfig { node_id, config } => {
                // Reset poison state on URL change so the instance gets
                // a fresh start with the new page.
                if let Some(state) = instances.get_mut(&node_id) {
                    let url_changed = !config.url.is_empty() && config.url != state.config.url;
                    if url_changed && (state.poisoned || state.consecutive_panic_count > 0) {
                        if state.poisoned {
                            tracing::info!(
                                node_id = %node_id,
                                new_url = %config.url,
                                "Resetting poisoned state on URL change",
                            );
                        }
                        state.poisoned = false;
                        state.consecutive_panic_count = 0;
                    }
                }

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
    tracing::info!(instances_cleared = count, "Servo thread shutting down gracefully");
    // `servo` is dropped here -- its Drop impl sends Exit and spins
    // until the constellation finishes shutting down.
}

/// Extract a human-readable message from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Send a fallback frame (last good frame or transparent) after a panic.
fn send_fallback_frame(instances: &HashMap<NodeId, InstanceState>, node_id: &NodeId) {
    let Some(state) = instances.get(node_id) else {
        return;
    };
    let fallback = state.last_good_frame.as_ref().map_or_else(
        || {
            let len = (state.config.width as usize) * (state.config.height as usize) * 4;
            vec![0u8; len]
        },
        Clone::clone,
    );
    let _ = state.result_tx.send(ServoThreadResult::Frame { rgba_data: fallback });
}

/// Handle a `Register` work item: create the WebView and the per-instance
/// rendering context, then return `InitOk` *immediately*.
///
/// Page loading is deferred — the first few `handle_render` ticks will
/// return transparent / partially-painted frames while Servo's event
/// loop progresses the load asynchronously.  This keeps node-init
/// latency bounded by GPU surface allocation (sub-second) instead of
/// blocking on the page's full first paint (5+ seconds for typical
/// websites).  The `load_timeout_secs` config field is currently a
/// no-op; it remains in the schema for forward compatibility (a future
/// change may use it to cap wait-for-load progression for diagnostics
/// or to move the node into Degraded if the page never loads).
///
/// One-shot post-load work (custom CSS injection) is gated on
/// `post_load_done` and runs in `handle_render` once
/// `delegate.loaded` flips, so the contract that "custom_css is
/// applied after load" is preserved.
fn handle_register(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: &mut Option<Servo>,
    node_id: NodeId,
    config: ServoConfig,
    result_tx: std::sync::mpsc::SyncSender<ServoThreadResult>,
) {
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
            tracing::info!(
                node_id = %node_id,
                url = %config.url,
                output = %format_args!("{}x{}", config.width, config.height),
                viewport = %format_args!("{}x{}", config.effective_viewport_width(), config.effective_viewport_height()),
                scaling = config.needs_scaling(),
                "Created Servo WebView (page load deferred)",
            );

            let rc_width = config.effective_viewport_width();
            let rc_height = config.effective_viewport_height();

            let _ = result_tx.send(ServoThreadResult::InitOk);
            instances.insert(
                node_id,
                InstanceState {
                    webview,
                    rendering_context,
                    delegate,
                    config,
                    result_tx,
                    rc_width,
                    rc_height,
                    last_good_frame: None,
                    render_count: 0,
                    render_duration_sum: Duration::ZERO,
                    consecutive_panic_count: 0,
                    poisoned: false,
                    post_load_done: false,
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
        // Instance or Servo not found — send a fallback frame so the
        // caller's blocking recv() in tick() does not deadlock.
        send_fallback_frame(instances, node_id);
        return;
    };

    let render_start = Instant::now();

    // Bind this instance's context before pumping so its own paint targets
    // its own surface.  `SoftwareRenderingContext`/surfman share GL state
    // process-wide, so the read below additionally re-binds (see there).
    let _ = state.rendering_context.make_current();

    // Pump the event loop to let Servo process pending work — this is
    // also what advances the deferred page load registered in
    // `handle_register`.
    servo.spin_event_loop();

    // Run one-shot post-load actions (custom CSS) the first tick that
    // observes a successful load.  Gated to fire exactly once per URL
    // (`UpdateConfig` resets `post_load_done` on URL change).
    if !state.post_load_done && state.delegate.loaded.get() {
        if let Some(ref css) = state.config.custom_css {
            inject_custom_css(&state.webview, servo, css);
        }
        state.post_load_done = true;
        tracing::info!(
            node_id = %node_id,
            url = %state.config.url,
            "Page reached LoadStatus::Complete — post-load actions applied",
        );
    }

    // Until Servo has painted the current page at least once, the surfman
    // surface backing this rendering context may still contain a previous,
    // unrelated capture's pixels (surfman reuses surfaces across contexts).
    // Emit a fully transparent frame instead of reading stale content, so a
    // freshly-opened clip/cast never leaks another session's page.
    if !state.delegate.painted.get() {
        let len = (state.config.width as usize) * (state.config.height as usize) * 4;
        let rgba_data = vec![0u8; len];
        state.render_count += 1;
        state.render_duration_sum += render_start.elapsed();
        if state.result_tx.send(ServoThreadResult::Frame { rgba_data }).is_err() {
            instances.remove(node_id);
        }
        return;
    }

    // Always read the full rendering context (rc_width × rc_height) —
    // this is the native size Servo is currently rendering at.  These
    // stay constant under `Resize` hints (output-only) but are updated
    // under `UpdateConfig` when the viewport resolution changes.
    let rect = Box2D::new(
        Point2D::new(0, 0),
        Point2D::new(
            i32::try_from(state.rc_width).unwrap_or(i32::MAX),
            i32::try_from(state.rc_height).unwrap_or(i32::MAX),
        ),
    );

    // Scale when the rendering context size differs from the output.
    let needs_scaling =
        state.rc_width != state.config.width || state.rc_height != state.config.height;

    // `read_to_image` reads whichever surfman context is currently bound
    // process-wide.  `spin_event_loop` paints every webview and leaves the
    // last-painted instance's context current, so re-bind ours immediately
    // before the read to guarantee we capture this instance's surface and
    // never a concurrent node's (the cross-session pixel leak).
    let _ = state.rendering_context.make_current();
    let rgba_data = if let Some(img) = state.rendering_context.read_to_image(rect) {
        let raw = if needs_scaling {
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
        // Reuse the old cache buffer for sending (avoids per-frame allocation).
        // Move the fresh `raw` into the cache and copy its data into the
        // reused buffer which is sent to the consumer.
        let mut send_buf =
            state.last_good_frame.take().unwrap_or_else(|| Vec::with_capacity(raw.len()));
        send_buf.clear();
        send_buf.extend_from_slice(&raw);
        state.last_good_frame = Some(raw);
        send_buf
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
        let avg_us = state.render_duration_sum.as_micros() / u128::from(state.render_count);
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
            // Re-arm the first-paint gate so `handle_render` emits
            // transparent frames until the new page paints, preventing the
            // outgoing page's pixels from bleeding into the new capture.
            state.delegate.painted.set(false);
            // Drop the cached frame so a `read_to_image` miss after the new
            // page paints can't fall back to the previous URL's last frame.
            state.last_good_frame = None;
            // Reset the one-shot post-load gate so the new page gets
            // its custom-CSS injection (if any) once it finishes
            // loading.  Render ticks will run `handle_render`'s
            // post-load block when `delegate.loaded` flips.
            state.post_load_done = false;
        }
    }

    // CSS-only changes (no URL change) apply immediately if the current
    // page is already loaded.  Otherwise they fold into the
    // render-driven post-load gate above, which will pick up the new
    // value because we're about to merge `new_config` into `state.config`.
    if css_changed && !url_changed && state.delegate.loaded.get() {
        if let Some(ref css) = new_config.custom_css {
            inject_custom_css(&state.webview, servo, css);
        }
    }

    // Commit the config only after all side effects have succeeded.
    // This ensures that on panic (caught by catch_unwind in the caller),
    // state.config still reflects the actual webview state, allowing
    // retries with the same URL to trigger navigation again.
    let viewport_changed = state.config.merge_update(new_config);

    // If the viewport resolution changed, resize the rendering context.
    if viewport_changed {
        let vw = state.config.effective_viewport_width();
        let vh = state.config.effective_viewport_height();
        if state.rc_width != vw || state.rc_height != vh {
            tracing::info!(
                node_id = %node_id,
                old = %format_args!("{}x{}", state.rc_width, state.rc_height),
                new = %format_args!("{vw}x{vh}"),
                "Resizing Servo viewport via config update",
            );
            let new_size = PhysicalSize::new(vw, vh);
            state.webview.resize(new_size);
            state.rc_width = vw;
            state.rc_height = vh;
            state.last_good_frame = None;
            // Re-arm the first-paint gate: resizing reallocates the
            // surfman surface, which is pooled across instances and may
            // hold a neighbour's pixels until the resized page repaints.
            state.delegate.painted.set(false);
            servo.spin_event_loop();
        }
    }
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
