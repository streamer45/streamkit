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
//! - Pages that never reach `LoadStatus::Complete` (common on ad-heavy
//!   sites) are handled by the node's first-load gate: it releases shortly
//!   after the first paint, or at `load_timeout_secs` at the latest.
//! - Each instance caches the last successfully rendered frame; on render
//!   failures the cached frame is returned instead of a blank.
//! - After [`POISON_THRESHOLD`] consecutive render panics an instance is
//!   *poisoned*: Servo calls are skipped entirely and the cached frame is
//!   returned until a URL change (`UpdateConfig`) resets the state.
//! - Diagnostics are emitted through the host [`Logger`] so they reach the
//!   skit process (the plugin dylib has no `tracing` subscriber of its own).

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use euclid::Scale;
use servo::{
    AuthenticationRequest, DeviceIntPoint, DeviceIntRect, LoadStatus, RenderingContext, Servo,
    ServoBuilder, SoftwareRenderingContext, UrlRequest, WebView, WebViewBuilder, WebViewDelegate,
};
use streamkit_plugin_sdk_native::prelude::Logger;
use streamkit_plugin_sdk_native::{plugin_debug, plugin_error, plugin_info, plugin_warn};

use crate::config::{ServoAuth, ServoBasicAuth, ServoConfig};

/// Number of consecutive render panics before an instance is marked as
/// poisoned and Servo calls are skipped entirely.
const POISON_THRESHOLD: u32 = 5;

/// Shared by the first-load gate and CSS staging so settle CSS is applied
/// before the gate can release the first frame.
pub const POST_PAINT_SETTLE: Duration = Duration::from_secs(2);

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
        logger: Logger,
    },
    /// Request a single rendered frame for the given instance.
    Render { node_id: NodeId },
    /// Pump the event loop and report the instance's load/paint state
    /// without a frame readback.  Used by the node's first-load gate to
    /// poll cheaply while it holds emission.
    Status { node_id: NodeId },
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
    /// Load/paint state for the node's first-load gate.  `painted` is true
    /// once the current page has painted at least once; `loaded` once it
    /// has additionally reached `LoadStatus::Complete`.
    Status { painted: bool, loaded: bool },
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
/// `loaded` is read by `pump_instance`'s post-load gate to fire one-shot
/// post-load actions (custom CSS injection) on the first pump after the
/// page reaches `LoadStatus::Complete`.
///
/// `painted` gates surface reads: surfman reuses the underlying
/// `SoftwareRenderingContext` surface across instances, so before Servo
/// has painted the current page at least once the surface may still hold
/// a previous (unrelated) capture's pixels.  Until the first paint,
/// `handle_render` emits a fully transparent frame rather than leaking
/// stale pixels.  Reset on URL change in `handle_update_config`.
///
/// `basic_auth` answers HTTP Basic/Digest (and proxy) authentication
/// challenges non-interactively.  It is bound at WebView creation and never
/// logged.
#[derive(Default)]
struct FrameDelegate {
    loaded: Cell<bool>,
    painted: Cell<bool>,
    basic_auth: Option<ServoBasicAuth>,
}

impl WebViewDelegate for FrameDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.loaded.set(true);
        }
        // Servo does not expose a Failed variant.  Load failures
        // would manifest as the page never reaching Complete; there is
        // no synchronous wait at register time, so failures simply
        // leave the page in its loading state and `handle_render`
        // returns whatever has been painted.
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.painted.set(true);
    }

    fn request_authentication(&self, _webview: WebView, request: AuthenticationRequest) {
        if let Some(ref basic) = self.basic_auth {
            request.authenticate(basic.username.clone(), basic.password.clone());
        }
        // Without configured credentials the request is dropped, which
        // yields no authentication (the default embedder behavior).
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CustomCssStage {
    FirstPaint,
    PostPaintSettle,
    LoadComplete,
}

impl CustomCssStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::FirstPaint => "first paint",
            Self::PostPaintSettle => "post-paint settle",
            Self::LoadComplete => "load complete",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CustomCssStages {
    first_paint: bool,
    post_paint_settle: bool,
    load_complete: bool,
}

impl CustomCssStages {
    const fn mark(&mut self, stage: CustomCssStage) {
        match stage {
            CustomCssStage::FirstPaint => self.first_paint = true,
            CustomCssStage::PostPaintSettle => self.post_paint_settle = true,
            CustomCssStage::LoadComplete => self.load_complete = true,
        }
    }

    const fn for_css_change(painted: bool, loaded: bool, settled: bool) -> Self {
        Self { first_paint: painted, post_paint_settle: settled, load_complete: loaded }
    }
}

fn next_custom_css_stage(
    painted: bool,
    loaded: bool,
    first_paint_elapsed: Option<Duration>,
    fired: CustomCssStages,
) -> Option<CustomCssStage> {
    if painted && !fired.first_paint {
        return Some(CustomCssStage::FirstPaint);
    }
    if loaded && !fired.load_complete {
        return Some(CustomCssStage::LoadComplete);
    }
    if painted
        && first_paint_elapsed.is_some_and(|elapsed| elapsed >= POST_PAINT_SETTLE)
        && !fired.post_paint_settle
    {
        return Some(CustomCssStage::PostPaintSettle);
    }
    None
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
    /// Records which custom CSS injection triggers have fired for the
    /// current URL.  Reset on `UpdateConfig` when the URL changes.
    custom_css_stages: CustomCssStages,
    /// Time at which the current page first painted, used for the delayed
    /// custom CSS re-application.
    first_paint_at: Option<Instant>,
    /// Host logger — the only way shared-thread diagnostics reach skit,
    /// since the plugin dylib has no `tracing` subscriber.
    logger: Logger,
    /// When the first `Render` for the current page arrived, for
    /// time-to-first-content instrumentation.
    first_render_at: Option<Instant>,
    /// Transparent fallback frames emitted before the current page's
    /// first paint.
    pre_paint_frames: u64,
    /// Whether the first painted frame for the current page has been
    /// logged (with pre-paint frame count and blank-surface check).
    content_frame_logged: bool,
    /// Navigation waiting for the webview's browsing context to come up.
    /// The constellation silently drops a `LoadUrl` sent before it has
    /// activated the context (which happens asynchronously after
    /// `WebViewBuilder::build()`), so navigations that can't ride on the
    /// builder URL — the custom-header initial load, and URL updates
    /// arriving before activation — are parked here and issued from
    /// `handle_render` once `WebView::url()` reports the context is live.
    pending_navigation: Option<url::Url>,
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
    // Fallback for diagnostics when the instance is gone (or was never
    // created): the most recently registered instance's logger.
    let mut thread_logger: Option<Logger> = None;
    // Servo's Opts is a process-global singleton -- we lazily create the
    // single Servo instance on the first Register and keep it alive for
    // the lifetime of the thread.
    let mut servo: Option<Servo> = None;

    while let Ok(work) = work_rx.recv() {
        match work {
            ServoWorkItem::Register { node_id, config, result_tx, logger } => {
                thread_logger = Some(logger.clone());
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_register(&mut instances, &mut servo, node_id, config, result_tx, logger);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    if let Some(logger) = thread_logger.as_ref() {
                        plugin_error!(
                            logger,
                            "[{node_id}] Panic during Servo Register — instance not created: {msg}"
                        );
                    }
                }
            },
            ServoWorkItem::Render { node_id } => {
                // Poisoned instances skip Servo entirely and return the
                // cached frame until a URL change resets the state.
                if instances.get(&node_id).is_some_and(|s| s.poisoned) {
                    send_fallback_frame(&instances, &node_id, thread_logger.as_ref());
                    continue;
                }

                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_render(&mut instances, servo.as_ref(), &node_id, thread_logger.as_ref());
                }));
                match result {
                    Ok(()) => {
                        if let Some(state) = instances.get_mut(&node_id) {
                            state.consecutive_panic_count = 0;
                        }
                    },
                    Err(panic) => {
                        let msg = panic_message(&panic);
                        if let Some(logger) =
                            node_logger(&instances, thread_logger.as_ref(), &node_id)
                        {
                            plugin_error!(
                                logger,
                                "[{node_id}] Panic during Servo Render — sending fallback frame: {msg}"
                            );
                        }
                        record_panic(&mut instances, &node_id);
                        send_fallback_frame(&instances, &node_id, thread_logger.as_ref());
                    },
                }
            },
            ServoWorkItem::Status { node_id } => {
                // Poisoned instances skip Servo calls; `send_status`
                // reports them as ready so the first-frame gate does not
                // wait out its timeout on an instance that will only ever
                // serve cached fallback frames.
                if instances.get(&node_id).is_some_and(|s| s.poisoned) {
                    send_status(&mut instances, &node_id, thread_logger.as_ref());
                    continue;
                }

                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_status(&mut instances, servo.as_ref(), &node_id, thread_logger.as_ref());
                }));
                match result {
                    Ok(()) => {
                        if let Some(state) = instances.get_mut(&node_id) {
                            state.consecutive_panic_count = 0;
                        }
                    },
                    Err(panic) => {
                        let msg = panic_message(&panic);
                        if let Some(logger) =
                            node_logger(&instances, thread_logger.as_ref(), &node_id)
                        {
                            plugin_error!(
                                logger,
                                "[{node_id}] Panic during Servo Status — reporting current state: {msg}"
                            );
                        }
                        record_panic(&mut instances, &node_id);
                        send_status(&mut instances, &node_id, thread_logger.as_ref());
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
                            plugin_info!(
                                state.logger,
                                "[{node_id}] Resetting poisoned state on URL change to '{}'",
                                crate::config::redact_url(&config.url)
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
                    if let Some(logger) = node_logger(&instances, thread_logger.as_ref(), &node_id)
                    {
                        plugin_error!(
                            logger,
                            "[{node_id}] Panic during Servo UpdateConfig — config not applied: {msg}"
                        );
                    }
                }
            },
            ServoWorkItem::Resize { node_id, width, height } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_resize(&mut instances, &node_id, width, height);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    if let Some(logger) = node_logger(&instances, thread_logger.as_ref(), &node_id)
                    {
                        plugin_error!(logger, "[{node_id}] Panic during Servo Resize: {msg}");
                    }
                }
            },
            ServoWorkItem::Unregister { node_id } => {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    if let Some(state) = instances.remove(&node_id) {
                        if state.render_count > 0 {
                            let avg_us = state.render_duration_sum.as_micros()
                                / u128::from(state.render_count);
                            plugin_info!(
                                state.logger,
                                "[{node_id}] Unregistered Servo instance: total_frames = {}, avg_render_us = {avg_us}",
                                state.render_count
                            );
                        }
                    }
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    if let Some(logger) = node_logger(&instances, thread_logger.as_ref(), &node_id)
                    {
                        plugin_error!(
                            logger,
                            "[{node_id}] Panic during Servo Unregister — instance may leak: {msg}"
                        );
                    }
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
    if let Some(logger) = thread_logger.as_ref() {
        plugin_info!(logger, "Servo thread shutting down gracefully: instances_cleared = {count}");
    }
    // `servo` is dropped here -- its Drop impl sends Exit and spins
    // until the constellation finishes shutting down.
}

/// Logger for diagnostics about `node_id`: the instance's own logger, or
/// the most recently registered instance's as a fallback.
fn node_logger<'a>(
    instances: &'a HashMap<NodeId, InstanceState>,
    fallback: Option<&'a Logger>,
    node_id: &NodeId,
) -> Option<&'a Logger> {
    instances.get(node_id).map(|s| &s.logger).or(fallback)
}

/// Extract a human-readable message from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// A fully transparent RGBA8 frame at the given output dimensions, used until
/// the page first paints, on a `read_to_image` miss with no cached frame, and
/// as the post-panic fallback.
fn transparent_frame(width: u32, height: u32) -> Vec<u8> {
    vec![0u8; (width as usize) * (height as usize) * 4]
}

/// Send a fallback frame (last good frame or transparent) after a panic.
///
/// When the instance is missing no reply can be built (its dimensions are
/// unknown); the caller's `recv_timeout` bounds the resulting wait.
fn send_fallback_frame(
    instances: &HashMap<NodeId, InstanceState>,
    node_id: &NodeId,
    fallback_logger: Option<&Logger>,
) {
    let Some(state) = instances.get(node_id) else {
        if let Some(logger) = fallback_logger {
            plugin_error!(
                logger,
                "[{node_id}] Render requested for unknown Servo instance — no frame reply sent"
            );
        }
        return;
    };
    let fallback = state
        .last_good_frame
        .clone()
        .unwrap_or_else(|| transparent_frame(state.config.width, state.config.height));
    let _ = state.result_tx.send(ServoThreadResult::Frame { rgba_data: fallback });
}

/// Track a Servo-call panic for an instance, poisoning it after
/// [`POISON_THRESHOLD`] consecutive panics.
fn record_panic(instances: &mut HashMap<NodeId, InstanceState>, node_id: &NodeId) {
    let Some(state) = instances.get_mut(node_id) else {
        return;
    };
    state.consecutive_panic_count += 1;
    if state.consecutive_panic_count >= POISON_THRESHOLD {
        state.poisoned = true;
        plugin_error!(
            state.logger,
            "[{node_id}] Servo instance poisoned after {POISON_THRESHOLD} consecutive render \
             panics — skipping Servo calls until URL change"
        );
    }
}

/// Handle a `Register` work item: create the WebView and the per-instance
/// rendering context, then return `InitOk` *immediately*.
///
/// Page loading is deferred — the node's first tick polls the shared Servo
/// thread's `Status` item until the page completes loading or has been
/// painted for the post-paint settle period, capped by `load_timeout_secs`.
///
/// Custom CSS is applied at first paint, after the post-paint settle period,
/// and at load completion.  Each trigger fires at most once per URL.
fn handle_register(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: &mut Option<Servo>,
    node_id: NodeId,
    config: ServoConfig,
    result_tx: std::sync::mpsc::SyncSender<ServoThreadResult>,
    logger: Logger,
) {
    let servo_ref = servo.get_or_insert_with(|| {
        let mut prefs = servo::Preferences {
            network_http_proxy_uri: String::new(),
            network_https_proxy_uri: String::new(),
            ..servo::Preferences::default()
        };
        // `Preferences` is a process-global singleton, so the User-Agent of
        // the first registered node applies to every servo node thereafter.
        if let Some(user_agent) = config.auth.as_ref().and_then(|a| a.user_agent.as_ref()) {
            prefs.user_agent.clone_from(user_agent);
        }
        let s: Servo = ServoBuilder::default().preferences(prefs).build();
        s.setup_logging();
        s
    });

    match create_webview(servo_ref, &config) {
        Ok((webview, rendering_context, delegate, pending_navigation)) => {
            plugin_info!(
                logger,
                "[{node_id}] Created Servo WebView (page load deferred): url = '{}', output = {}x{}, viewport = {}x{}, scaling = {}",
                crate::config::redact_url(&config.url),
                config.width,
                config.height,
                config.effective_viewport_width(),
                config.effective_viewport_height(),
                config.needs_scaling()
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
                    custom_css_stages: CustomCssStages::default(),
                    first_paint_at: None,
                    logger,
                    first_render_at: None,
                    pre_paint_frames: 0,
                    content_frame_logged: false,
                    pending_navigation,
                },
            );
        },
        Err(e) => {
            plugin_error!(logger, "[{node_id}] Failed to create Servo WebView: {e}");
            let _ = result_tx.send(ServoThreadResult::InitErr(e));
        },
    }
}

/// Pump the shared event loop on behalf of one instance: bind its
/// rendering context, spin, issue any parked navigation once the browsing
/// context is live, and run staged custom CSS actions.
fn pump_instance(state: &mut InstanceState, servo: &Servo, node_id: &NodeId) {
    // Bind this instance's context before pumping so its own paint targets
    // its own surface.  `SoftwareRenderingContext`/surfman share GL state
    // process-wide, so frame reads additionally re-bind (see
    // `read_painted_frame`).
    let _ = state.rendering_context.make_current();

    // Pump the event loop to let Servo process pending work — this is
    // also what advances the deferred page load registered in
    // `handle_register`.
    servo.spin_event_loop();

    // `WebView::url()` flips to `Some` on the constellation's first
    // history update, i.e. once the browsing context is active and
    // `load_request` is no longer dropped.
    if state.pending_navigation.is_some() && state.webview.url().is_some() {
        if let Some(url) = state.pending_navigation.take() {
            navigate_with_auth(&state.webview, state.config.auth.as_ref(), url);
            // The builder's `about:blank` may already have completed and
            // painted; re-arm both flags so load/paint state (and the
            // first-paint surface gate) track the target page.
            state.delegate.loaded.set(false);
            state.delegate.painted.set(false);
        }
    }

    let painted = page_painted(state);
    if painted && state.first_paint_at.is_none() {
        state.first_paint_at = Some(Instant::now());
    }
    let loaded = page_loaded(state);
    if let Some(stage) = next_custom_css_stage(
        painted,
        loaded,
        state.first_paint_at.map(|at| at.elapsed()),
        state.custom_css_stages,
    ) {
        state.custom_css_stages.mark(stage);
        if let Some(ref css) = state.config.custom_css {
            inject_custom_css(&state.webview, servo, css);
            plugin_info!(
                state.logger,
                "[{node_id}] Custom CSS applied: url = '{}', trigger = {}",
                crate::config::redact_url(&state.config.url),
                stage.as_str()
            );
        }
    }
}

/// Handle a `Status` work item: pump the event loop so the deferred page
/// load progresses, then report load/paint state without the full-frame
/// readback of `handle_render`.  Keeps the node's first-load polling
/// cheap on the shared thread.
fn handle_status(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: Option<&Servo>,
    node_id: &NodeId,
    fallback_logger: Option<&Logger>,
) {
    if let (Some(state), Some(servo)) = (instances.get_mut(node_id), servo) {
        pump_instance(state, servo, node_id);
    }
    send_status(instances, node_id, fallback_logger);
}

/// Report an instance's load/paint state.  Poisoned instances report
/// ready so the first-frame gate does not wait out its full timeout on
/// an instance that will only ever serve cached fallback frames.
fn send_status(
    instances: &mut HashMap<NodeId, InstanceState>,
    node_id: &NodeId,
    fallback_logger: Option<&Logger>,
) {
    let Some(state) = instances.get(node_id) else {
        if let Some(logger) = fallback_logger {
            plugin_error!(
                logger,
                "[{node_id}] Status requested for unknown Servo instance — no status reply sent"
            );
        }
        return;
    };
    let painted = state.poisoned || page_painted(state);
    let loaded = state.poisoned || page_loaded(state);
    if state.result_tx.send(ServoThreadResult::Status { painted, loaded }).is_err() {
        instances.remove(node_id);
    }
}

/// Handle a `Render` work item: pump the event loop and read pixels.
fn handle_render(
    instances: &mut HashMap<NodeId, InstanceState>,
    servo: Option<&Servo>,
    node_id: &NodeId,
    fallback_logger: Option<&Logger>,
) {
    let (Some(state), Some(servo)) = (instances.get_mut(node_id), servo) else {
        // Instance or Servo not found — send a fallback frame so the
        // caller's blocking recv() in tick() does not deadlock.
        send_fallback_frame(instances, node_id, fallback_logger);
        return;
    };

    let render_start = Instant::now();
    if state.first_render_at.is_none() {
        state.first_render_at = Some(render_start);
    }

    pump_instance(state, servo, node_id);

    let rgba_data = if state.delegate.painted.get() {
        let frame = read_painted_frame(state, node_id);
        // Gate on `page_painted` (not the raw flag) so a paint of the
        // builder's `about:blank` on the deferred-navigation path does not
        // consume the one-shot log before the real page's first frame.
        if page_painted(state) && !state.content_frame_logged {
            state.content_frame_logged = true;
            // A painted-but-blank surface distinguishes an early/empty paint
            // signal from a late paint when diagnosing black capture starts.
            let blank = frame.iter().all(|&b| b == 0);
            plugin_info!(
                state.logger,
                "[{node_id}] First painted frame read: pre_paint_frames = {}, since_first_render = {:?}, blank_surface = {blank}",
                state.pre_paint_frames,
                state.first_render_at.map(|at| at.elapsed()).unwrap_or_default()
            );
        }
        frame
    } else {
        state.pre_paint_frames += 1;
        // Until Servo has painted the current page at least once, the surfman
        // surface backing this rendering context may still contain a previous,
        // unrelated capture's pixels (surfman reuses surfaces across contexts).
        // Emit a fully transparent frame instead of reading stale content, so a
        // freshly-opened clip/cast never leaks another session's page.
        transparent_frame(state.config.width, state.config.height)
    };

    let render_duration = render_start.elapsed();
    state.render_count += 1;
    state.render_duration_sum += render_duration;

    // Log render time periodically (every 300 frames ~ 10s at 30fps).
    if state.render_count % 300 == 0 {
        let avg_us = state.render_duration_sum.as_micros() / u128::from(state.render_count);
        plugin_debug!(
            state.logger,
            "[{node_id}] Servo render metrics: frame = {}, render_us = {}, avg_render_us = {avg_us}",
            state.render_count,
            render_duration.as_micros()
        );
    }

    if state.result_tx.send(ServoThreadResult::Frame { rgba_data }).is_err() {
        instances.remove(node_id);
    }
}

/// Whether the instance's current page has painted at least once.
/// A pending deferred navigation means any load/paint state belongs to
/// the builder's `about:blank`, not the target page.
fn page_painted(state: &InstanceState) -> bool {
    state.pending_navigation.is_none() && state.delegate.painted.get()
}

/// Whether the instance's current page has fully loaded and painted.
fn page_loaded(state: &InstanceState) -> bool {
    page_painted(state) && state.delegate.loaded.get()
}

/// Read this instance's painted surface into an RGBA8 frame, scaling to the
/// output size and falling back to the cached frame (or transparent) on a miss.
/// Caller must have already confirmed the page has painted at least once.
fn read_painted_frame(state: &mut InstanceState, node_id: &NodeId) -> Vec<u8> {
    // Always read the full rendering context (rc_width × rc_height) — the
    // native size Servo is currently rendering at. These stay constant under
    // `Resize` hints (output-only) but are updated under `UpdateConfig` when
    // the viewport resolution changes.
    let rect = DeviceIntRect::new(
        DeviceIntPoint::new(0, 0),
        DeviceIntPoint::new(
            i32::try_from(state.rc_width).unwrap_or(i32::MAX),
            i32::try_from(state.rc_height).unwrap_or(i32::MAX),
        ),
    );

    let needs_scaling =
        state.rc_width != state.config.width || state.rc_height != state.config.height;

    // `read_to_image` reads whichever surfman context is currently bound
    // process-wide. `spin_event_loop` paints every webview and leaves the
    // last-painted instance's context current, so re-bind ours immediately
    // before the read to capture this instance's surface and never a concurrent
    // node's (the cross-session pixel leak). A bind failure means the read may
    // return another instance's pixels, so surface it rather than silently leak.
    if let Err(e) = state.rendering_context.make_current() {
        plugin_warn!(
            state.logger,
            "[{node_id}] make_current before read_to_image failed; frame may not be this instance's surface: {e:?}"
        );
    }
    if let Some(img) = state.rendering_context.read_to_image(rect) {
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
        plugin_debug!(state.logger, "[{node_id}] read_to_image returned None, using cached frame");
        cached.clone()
    } else {
        transparent_frame(state.config.width, state.config.height)
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
            if state.webview.url().is_none() {
                state.pending_navigation = Some(parsed);
            } else {
                state.pending_navigation = None;
                navigate_with_auth(&state.webview, state.config.auth.as_ref(), parsed);
            }
            state.delegate.loaded.set(false);
            // Re-arm the first-paint gate so `handle_render` emits
            // transparent frames until the new page paints, preventing the
            // outgoing page's pixels from bleeding into the new capture.
            state.delegate.painted.set(false);
            // Drop the cached frame so a `read_to_image` miss after the new
            // page paints can't fall back to the previous URL's last frame.
            state.last_good_frame = None;
            state.custom_css_stages = CustomCssStages::default();
            state.first_paint_at = None;
            state.first_render_at = None;
            state.pre_paint_frames = 0;
            state.content_frame_logged = false;
        }
    }

    if css_changed && !url_changed {
        let painted = page_painted(state);
        let loaded = page_loaded(state);
        let settled = state.first_paint_at.is_some_and(|at| at.elapsed() >= POST_PAINT_SETTLE);
        state.custom_css_stages = CustomCssStages::for_css_change(painted, loaded, settled);
        if let Some(ref css) = new_config.custom_css {
            if painted {
                inject_custom_css(&state.webview, servo, css);
            }
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
            plugin_info!(
                state.logger,
                "[{node_id}] Resizing Servo viewport via config update: {}x{} -> {vw}x{vh}",
                state.rc_width,
                state.rc_height
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
            state.first_paint_at = None;
            state.first_render_at = None;
            state.pre_paint_frames = 0;
            state.content_frame_logged = false;
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
    plugin_info!(
        state.logger,
        "[{node_id}] Resized Servo output via upstream hint: {}x{} -> {width}x{height}",
        state.config.width,
        state.config.height
    );
    state.config.width = width;
    state.config.height = height;
    // Invalidate the cached frame since dimensions changed.
    state.last_good_frame = None;
}

/// A freshly created webview plus the navigation deferred to the render
/// loop (see `InstanceState::pending_navigation`), if any.
type CreatedWebView = (WebView, Rc<SoftwareRenderingContext>, Rc<FrameDelegate>, Option<url::Url>);

/// Create a `WebView` with its own `SoftwareRenderingContext` on the shared
/// Servo instance.  The rendering context uses the *viewport* dimensions
/// (which may be larger than the output frame), so the page layout has
/// room to breathe.  Scaling to output dimensions happens in `handle_render`.
fn create_webview(servo: &Servo, config: &ServoConfig) -> Result<CreatedWebView, String> {
    let vw = config.effective_viewport_width();
    let vh = config.effective_viewport_height();
    let size = PhysicalSize::new(vw, vh);
    let rendering_context: Rc<SoftwareRenderingContext> = Rc::new(
        SoftwareRenderingContext::new(size)
            .map_err(|e| format!("Failed to create SoftwareRenderingContext: {e:?}"))?,
    );

    let _ = rendering_context.make_current();

    let delegate: Rc<FrameDelegate> = Rc::new(FrameDelegate {
        loaded: Cell::new(false),
        painted: Cell::new(false),
        basic_auth: config.auth.as_ref().and_then(|a| a.basic.clone()),
    });

    let parsed_url = url::Url::parse(&config.url)
        .map_err(|e| format!("Invalid URL '{}': {e}", crate::config::redact_url(&config.url)))?;

    // Surface malformed auth here even though `validate` already checked it,
    // so a bad config fails registration rather than silently dropping headers.
    let headers = match config.auth.as_ref() {
        Some(auth) => {
            Some(auth.build_request_headers().map_err(|e| format!("invalid auth config: {e}"))?)
        },
        None => None,
    }
    .filter(|h| !h.is_empty());

    let mut builder = WebViewBuilder::new(servo, rendering_context.clone())
        .hidpi_scale_factor(Scale::new(1.0))
        .delegate(delegate.clone() as Rc<dyn WebViewDelegate>);

    // The initial URL must go through the builder: the constellation
    // silently drops a `LoadUrl` (`WebView::load`/`load_request`) sent
    // before it has activated the webview's browsing context, which
    // happens asynchronously after `build()`.  Custom request headers
    // can't be attached to the builder's URL, so the auth path builds
    // with the default `about:blank` and parks the header-carrying
    // navigation as `pending_navigation`, issued from `handle_render`
    // once the context is up — keeping registration non-blocking so
    // other nodes on the shared thread keep rendering.
    let pending_navigation = if headers.is_some() {
        Some(parsed_url)
    } else {
        builder = builder.url(parsed_url);
        None
    };
    let webview: WebView = builder.build();

    Ok((webview, rendering_context, delegate, pending_navigation))
}

/// Navigate `webview` to `url`, re-applying the configured custom request
/// headers / bearer token so authenticated pages keep loading on runtime URL
/// changes — not just the initial navigation.
///
/// Falls back to a plain navigation when no headers are configured (or, defensively,
/// when they fail to build; `validate` already rejects malformed auth at config time).
fn navigate_with_auth(webview: &WebView, auth: Option<&ServoAuth>, url: url::Url) {
    let headers = auth.and_then(|a| a.build_request_headers().ok());
    match headers {
        Some(headers) if !headers.is_empty() => {
            webview.load_request(UrlRequest::new(url).headers(headers));
        },
        _ => {
            webview.load(url);
        },
    }
}

/// Inject custom CSS into a painted page via JavaScript.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_css_stages_fire_once_in_page_lifecycle_order() {
        let mut fired = CustomCssStages::default();

        assert_eq!(
            next_custom_css_stage(true, false, Some(Duration::ZERO), fired),
            Some(CustomCssStage::FirstPaint)
        );
        fired.mark(CustomCssStage::FirstPaint);

        assert_eq!(
            next_custom_css_stage(true, true, Some(Duration::from_secs(1)), fired),
            Some(CustomCssStage::LoadComplete)
        );
        fired.mark(CustomCssStage::LoadComplete);

        assert_eq!(
            next_custom_css_stage(true, true, Some(POST_PAINT_SETTLE), fired),
            Some(CustomCssStage::PostPaintSettle)
        );
        fired.mark(CustomCssStage::PostPaintSettle);

        assert_eq!(next_custom_css_stage(true, true, Some(Duration::from_secs(3)), fired), None);
    }

    #[test]
    fn custom_css_stages_wait_for_paint_and_settle() {
        let fired = CustomCssStages::default();

        assert_eq!(next_custom_css_stage(false, false, None, fired), None);
        assert_eq!(
            next_custom_css_stage(true, false, Some(Duration::from_secs(1)), fired),
            Some(CustomCssStage::FirstPaint)
        );

        let fired = CustomCssStages { first_paint: true, ..fired };
        assert_eq!(next_custom_css_stage(true, false, Some(Duration::from_secs(1)), fired), None);
        assert_eq!(
            next_custom_css_stage(true, false, Some(POST_PAINT_SETTLE), fired),
            Some(CustomCssStage::PostPaintSettle)
        );
    }

    #[test]
    fn custom_css_change_rearms_future_stages_without_reinjecting_first_paint() {
        let rearmed = CustomCssStages::for_css_change(true, true, true);

        assert_eq!(next_custom_css_stage(true, true, Some(Duration::from_secs(3)), rearmed), None);

        let rearmed = CustomCssStages::for_css_change(true, false, false);
        assert_eq!(next_custom_css_stage(true, false, Some(Duration::ZERO), rearmed), None);
        assert_eq!(
            next_custom_css_stage(true, false, Some(POST_PAINT_SETTLE), rearmed),
            Some(CustomCssStage::PostPaintSettle)
        );

        let rearmed = CustomCssStages::for_css_change(false, false, false);
        assert_eq!(next_custom_css_stage(false, false, None, rearmed), None);
        assert_eq!(
            next_custom_css_stage(true, false, Some(Duration::ZERO), rearmed),
            Some(CustomCssStage::FirstPaint)
        );
    }
}
