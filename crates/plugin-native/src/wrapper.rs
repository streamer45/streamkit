// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Native Node Wrapper
//!
//! This module provides the `NativeNodeWrapper` which implements the `ProcessorNode` trait
//! and bridges to the C ABI plugin interface.
//!
//! ## Callback Lifetime Contract
//!
//! The [`CNodeCallbacks`] struct and all pointers passed to plugin API
//! functions (`process_packet`, `flush`, `tick`) are valid **only for
//! the duration of that single host→plugin call**.  The plugin must not
//! stash callback pointers or call them after returning.
//!
//! ## Timeout and Instance Leak
//!
//! [`call_with_timeout`] wraps `spawn_blocking` calls with
//! `tokio::time::timeout`.  When a call times out, the blocking task
//! **cannot be cancelled** — it keeps running in the background, holding
//! a [`CallGuard`] that keeps `in_flight_calls` elevated.  If the
//! wrapper is subsequently dropped, `destroy_instance` is deferred to
//! the still-running task (via `finish_call` in the `CallGuard` drop).
//! If the plugin never returns, the instance and its `Arc<Library>` leak
//! for the process lifetime.  This is safe (no UB) but not ideal.
//!
//! Additionally, `NativeNodeWrapper::drop` calls `request_drop` →
//! `destroy_instance` synchronously on whatever thread drops the
//! wrapper — typically an async tokio worker.  A slow plugin destroy
//! will stall that worker.  Routing through `block_in_place` or a
//! dedicated thread is a future improvement.

use anyhow::Result;
use async_trait::async_trait;
use libloading::Library;
use std::ffi::{c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::types::Packet;
use streamkit_core::{
    AudioFramePool, InputPin, NodeContext, NodeState, NodeStateUpdate, OutputPin, ProcessorNode,
    StopReason, StreamKitError, VideoFramePool,
};
use streamkit_plugin_sdk_native::{
    conversions,
    types::{
        CAllocAudioResult, CAllocVideoResult, CNativePluginAPI, CNodeCallbacks, CPacket,
        CPluginHandle, CResult,
    },
};
use tracing::{error, info, warn};

use crate::PluginMetadata;

// ── Host-side FFI panic guards ─────────────────────────────────────────────
//
// These guard the `extern "C"` callbacks that the host exposes to plugins.
// A misbehaving plugin could trigger a panic in host code (e.g. via a
// poisoned mutex or an unexpected null pointer); without `catch_unwind` the
// panic would unwind across the C ABI boundary — instant UB.
//
// All guards are implemented in terms of `ffi_guard_with`, which handles
// catch_unwind + panic message extraction + logging.  The `on_panic`
// closure receives the formatted message so the caller can embed it in
// the return value if desired.

/// Generic host-side panic guard.
///
/// Runs `f` inside [`catch_unwind`].  On panic, extracts a human-readable
/// message, logs it at `error!` level with `label`, and calls `on_panic`
/// to produce a fallback return value.
///
/// **Note:** `on_panic` runs *outside* the `catch_unwind`.  If it panics
/// (e.g. `error_to_c` hits a poisoned `RefCell`), the panic will
/// propagate across the C ABI — UB.  All current `on_panic` impls are
/// trivial (construct a null result or call the infallible
/// `error_to_c`), so this is not a practical concern today.
fn ffi_guard_with<T>(label: &str, on_panic: impl FnOnce(String) -> T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let msg = streamkit_plugin_sdk_native::ffi_guard::panic_message(&*payload);
            error!("{label}: {msg}");
            on_panic(format!("{label}: {msg}"))
        },
    }
}

/// Guard a host callback that returns [`CResult`].
fn ffi_guard_result(f: impl FnOnce() -> CResult) -> CResult {
    ffi_guard_with("host callback panicked", |msg| CResult::error(conversions::error_to_c(msg)), f)
}

/// Guard a host callback that returns [`CAllocVideoResult`].
fn ffi_guard_alloc_video(f: impl FnOnce() -> CAllocVideoResult) -> CAllocVideoResult {
    ffi_guard_with("host video allocation callback panicked", |_| CAllocVideoResult::null(), f)
}

/// Guard a host callback that returns [`CAllocAudioResult`].
fn ffi_guard_alloc_audio(f: impl FnOnce() -> CAllocAudioResult) -> CAllocAudioResult {
    ffi_guard_with("host audio allocation callback panicked", |_| CAllocAudioResult::null(), f)
}

/// Guard a host callback that returns nothing (e.g. logging, buffer free).
fn ffi_guard_unit(f: impl FnOnce()) {
    ffi_guard_with(
        "host callback panicked",
        |_| {},
        || {
            f();
        },
    );
}

/// Default timeout for plugin FFI calls (process_packet, flush, tick).
/// 5 minutes — generous to support slow plugins (e.g. ML inference).
// TODO: wire this through user configuration (e.g. pipeline YAML or server
// config) instead of hard-coding.  `set_call_timeout` exists on
// `LoadedNativePlugin` but nothing calls it yet.
pub(crate) const DEFAULT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Wrapper preserving pointer provenance for the plugin's API vtable.
///
/// The raw pointer is derived from `&'static CNativePluginAPI` which lives
/// inside the loaded `Library`.  The library is kept alive by
/// `InstanceState::library: Arc<Library>`, so the pointee is valid for
/// the lifetime of the `InstanceState`.
///
/// # Safety
///
/// `ApiPtr` is `Send + Sync` because the pointee (`CNativePluginAPI`) is
/// a static vtable of function pointers — never mutated after construction
/// — and the `Library` keeping it alive is behind an `Arc`.
struct ApiPtr(*const CNativePluginAPI);

// SAFETY: The pointee is a static vtable of function pointers, never mutated
// after construction.  The `Library` owning the symbol is kept alive by an `Arc`.
unsafe impl Send for ApiPtr {}
// SAFETY: Same as Send — immutable vtable behind Arc<Library>.
unsafe impl Sync for ApiPtr {}

/// RAII guard for an in-flight FFI call.
///
/// Created by [`InstanceState::begin_call`].  Dropping the guard
/// decrements `in_flight_calls` (and triggers deferred destroy if
/// this was the last call and `drop_requested` is set).
///
/// This ensures `finish_call` runs even if the body panics, early-returns,
/// or propagates an error via `?`.
///
/// The borrow ties the guard's lifetime to the `InstanceState`.  Callers
/// that move the guard into a closure (e.g. `spawn_blocking`) must keep
/// the `Arc<InstanceState>` alive in the enclosing scope.
struct CallGuard<'a> {
    state: &'a InstanceState,
    handle: CPluginHandle,
}

impl CallGuard<'_> {
    /// Returns the plugin handle for use in FFI calls.
    const fn handle(&self) -> CPluginHandle {
        self.handle
    }
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        self.state.finish_call();
    }
}

struct InstanceState {
    library: Arc<Library>,
    api: ApiPtr,
    handle: AtomicPtr<c_void>,
    in_flight_calls: AtomicUsize,
    drop_requested: AtomicBool,
    /// One-shot flag: set after the first timeout warning so a wedged
    /// plugin does not spam `warn!` on every subsequent FFI call.
    timeout_warned: AtomicBool,
    /// Plugin's declared API version (6, 7, or 8).  Used to avoid sending
    /// `BinaryWithMeta` packets to v6 plugins that don't understand them.
    /// v7 plugins understand BinaryWithMeta but not EncodedAudio metadata
    /// (which is fine — EncodedAudio is metadata-only, not a runtime packet).
    api_version: u32,
    call_timeout: Option<std::time::Duration>,
}

impl InstanceState {
    const fn new(
        library: Arc<Library>,
        api: &'static CNativePluginAPI,
        handle: CPluginHandle,
        api_version: u32,
        call_timeout: Option<std::time::Duration>,
    ) -> Self {
        Self {
            library,
            api: ApiPtr(std::ptr::from_ref(api)),
            handle: AtomicPtr::new(handle),
            in_flight_calls: AtomicUsize::new(0),
            drop_requested: AtomicBool::new(false),
            timeout_warned: AtomicBool::new(false),
            api_version,
            call_timeout,
        }
    }

    const fn api(&self) -> &'static CNativePluginAPI {
        // SAFETY: The pointer was derived from a valid &'static CNativePluginAPI reference
        // via std::ptr::from_ref, preserving provenance.  The loaded library is kept alive
        // by self.library (Arc<Library>) held by this state.
        unsafe { &*self.api.0 }
    }

    /// Acquire a guard for an in-flight FFI call.
    ///
    /// Returns `None` if the instance has been destroyed or a drop has been
    /// requested.  Uses Dekker-style mutual exclusion with [`request_drop`]:
    ///
    /// ```text
    /// begin_call:   write(in_flight, SeqCst) → read(drop_requested, SeqCst)
    /// request_drop: write(drop_requested, SeqCst) → read(in_flight, SeqCst)
    /// ```
    ///
    /// `SeqCst` on both sides guarantees at least one side observes the
    /// other's write, closing the TOCTOU window between incrementing
    /// `in_flight_calls` and loading the handle.
    fn begin_call(&self) -> Option<CallGuard<'_>> {
        self.in_flight_calls.fetch_add(1, Ordering::SeqCst);

        // Dekker re-check: if drop was requested, roll back.  A
        // concurrent request_drop may have seen our in_flight == 1 and
        // deferred destroy.  If our rollback brings the count back to 0,
        // we are responsible for triggering destroy (idempotent).
        if self.drop_requested.load(Ordering::SeqCst) {
            let prev = self.in_flight_calls.fetch_sub(1, Ordering::SeqCst);
            if prev == 1 {
                self.destroy_instance();
            }
            return None;
        }

        let h = self.handle.load(Ordering::Acquire);
        if h.is_null() {
            self.in_flight_calls.fetch_sub(1, Ordering::SeqCst);
            return None;
        }

        Some(CallGuard { state: self, handle: h })
    }

    /// Decrement `in_flight_calls`; if this was the last call and
    /// `drop_requested` is set, trigger `destroy_instance`.
    ///
    /// Both `finish_call` and `begin_call` (rollback path) may call
    /// `destroy_instance` — this is benign because `destroy_instance`
    /// is idempotent (atomic null-swap guard).
    fn finish_call(&self) {
        let prev = self.in_flight_calls.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(prev > 0, "finish_call called without begin_call");

        if prev == 1 && self.drop_requested.load(Ordering::SeqCst) {
            self.destroy_instance();
        }
    }

    /// Signal that the wrapper is done with this instance.
    ///
    /// If no calls are in flight, destroys immediately.  Otherwise,
    /// defers to the last [`finish_call`].  Uses `SeqCst` ordering
    /// to form a Dekker pair with [`begin_call`].
    fn request_drop(&self) {
        self.drop_requested.store(true, Ordering::SeqCst);
        if self.in_flight_calls.load(Ordering::SeqCst) == 0 {
            self.destroy_instance();
        }
    }

    fn destroy_instance(&self) {
        let h = self.handle.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if h.is_null() {
            return;
        }

        let api = self.api();
        // Wrap in ffi_guard_unit so a panicking destroy (e.g. via
        // InstanceState::Drop) cannot unwind across the C ABI boundary
        // or trigger a double-panic abort.
        ffi_guard_unit(|| (api.destroy_instance)(h));
    }
}

impl Drop for InstanceState {
    fn drop(&mut self) {
        // Defense-in-depth: if NativeNodeWrapper::drop failed to call
        // request_drop (or if request_drop deferred), make one last
        // attempt.  destroy_instance is idempotent (null-swap guard).
        self.destroy_instance();
    }
}

/// C callback function for plugin logging.
/// Routes plugin logs to the tracing infrastructure.
#[allow(clippy::cognitive_complexity)]
extern "C" fn plugin_log_callback(
    level: streamkit_plugin_sdk_native::types::CLogLevel,
    target: *const std::os::raw::c_char,
    message: *const std::os::raw::c_char,
    _user_data: *mut c_void,
) {
    ffi_guard_unit(|| {
        use streamkit_plugin_sdk_native::{conversions, types::CLogLevel};

        let target_str = if target.is_null() {
            "unknown".to_string()
        } else {
            unsafe { conversions::c_str_to_string(target) }
                .unwrap_or_else(|_| "unknown".to_string())
        };

        let message_str = if message.is_null() {
            String::new()
        } else {
            unsafe { conversions::c_str_to_string(message) }
                .unwrap_or_else(|_| "[invalid UTF-8]".to_string())
        };

        match level {
            CLogLevel::Trace => {
                tracing::event!(tracing::Level::TRACE, target = %target_str, "{}", message_str);
            },
            CLogLevel::Debug => {
                tracing::event!(tracing::Level::DEBUG, target = %target_str, "{}", message_str);
            },
            CLogLevel::Info => {
                tracing::event!(tracing::Level::INFO, target = %target_str, "{}", message_str);
            },
            CLogLevel::Warn => {
                tracing::event!(tracing::Level::WARN, target = %target_str, "{}", message_str);
            },
            CLogLevel::Error => {
                tracing::event!(tracing::Level::ERROR, target = %target_str, "{}", message_str);
            },
        }
    });
}

/// Wrapper that implements ProcessorNode for native plugins
pub struct NativeNodeWrapper {
    state: Arc<InstanceState>,
    metadata: PluginMetadata,
}

impl NativeNodeWrapper {
    /// Create a new native node wrapper
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Parameter serialization to JSON fails
    /// - Parameter string contains null bytes
    /// - Plugin fails to create an instance
    pub fn new(
        library: Arc<Library>,
        api: &'static CNativePluginAPI,
        metadata: PluginMetadata,
        params: Option<&serde_json::Value>,
        call_timeout: Option<std::time::Duration>,
    ) -> Result<Self, StreamKitError> {
        // Convert params to JSON string if provided
        let params_json = params
            .map(|p| {
                serde_json::to_string(p).map_err(|e| {
                    StreamKitError::Configuration(format!("Failed to serialize params: {e}"))
                })
            })
            .transpose()?;

        let params_cstr =
            params_json.as_ref().map(|s| CString::new(s.as_str())).transpose().map_err(|e| {
                StreamKitError::Configuration(format!("Invalid params string: {e}"))
            })?;

        let params_ptr = params_cstr.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());

        // Create plugin instance with logging callback
        let handle = (api.create_instance)(params_ptr, plugin_log_callback, std::ptr::null_mut());

        if handle.is_null() {
            return Err(StreamKitError::Configuration(
                "Plugin failed to create instance".to_string(),
            ));
        }

        Ok(Self {
            state: Arc::new(InstanceState::new(library, api, handle, api.version, call_timeout)),
            metadata,
        })
    }
}

#[async_trait]
impl ProcessorNode for NativeNodeWrapper {
    fn input_pins(&self) -> Vec<InputPin> {
        self.metadata.inputs.clone()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        self.metadata.outputs.clone()
    }

    // TODO: runtime_param_schema is synchronous and has no timeout protection —
    // a hung plugin will stall the calling tokio worker.  Refactoring to
    // async + call_with_timeout is a larger change (trait method is sync).
    // NOTE: unlike async call sites, this path does not wrap the FFI call
    // in catch_unwind — a panicking plugin will unwind into the caller.
    // Closing this asymmetry requires moving to spawn_blocking + timeout.
    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        let get_schema = self.state.api().get_runtime_param_schema?;
        let guard = self.state.begin_call()?;

        let result = get_schema(guard.handle());

        if !result.success {
            // FFI call failed — log and return None.
            if !result.error_message.is_null() {
                let msg = unsafe { conversions::c_str_to_string(result.error_message) }
                    .unwrap_or_default();
                warn!(error = %msg, "Plugin runtime_param_schema failed");
            }
            return None;
        }

        // success=true, null json_schema → plugin has no runtime schema.
        if result.json_schema.is_null() {
            return None;
        }

        // success=true, non-null json_schema → JSON string containing the schema.
        // SAFETY: result.json_schema points to a thread-local CString managed
        // by the plugin SDK.  We copy the string BEFORE dropping the guard so
        // no subsequent FFI call can overwrite the thread-local buffer.
        let json_str = unsafe { conversions::c_str_to_string(result.json_schema) }.ok();
        drop(guard);
        json_str.and_then(|s| serde_json::from_str(&s).ok())
    }

    // The run method is complex by necessity - it's an async actor managing FFI calls,
    // control messages, and packet processing. Breaking it up would make the logic harder to follow.
    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError> {
        if self.metadata.is_source {
            return self.run_source(context).await;
        }
        self.run_processor(context).await
    }
}

// ── Private run implementations ────────────────────────────────────────────
impl NativeNodeWrapper {
    /// Run `f` on a blocking thread with an optional timeout.
    ///
    /// If `self.state.call_timeout` is `Some(d)` and the call exceeds `d`,
    /// returns `StreamKitError::Runtime`.  The `spawn_blocking` task itself
    /// cannot be cancelled — it will eventually complete in the background
    /// — but the pipeline node gets an immediate error so downstream can
    /// react.
    ///
    /// **Leak note:** if a call times out and the wrapper is subsequently
    /// dropped, `destroy_instance` is deferred to the still-running
    /// blocking task (via `CallGuard` / `finish_call`).  If the plugin
    /// never returns, the instance and its `Arc<Library>` leak for the
    /// process lifetime.  This is safe but not ideal — a future metric
    /// (`plugin.leaked_instances`) should track this case.
    async fn call_with_timeout<T, F>(
        &self,
        op: &'static str,
        node: &str,
        f: F,
    ) -> Result<T, StreamKitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let task = tokio::task::spawn_blocking(f);
        match self.state.call_timeout {
            Some(timeout) => tokio::time::timeout(timeout, task)
                .await
                .map_err(|_| {
                    // Rate-limit: only warn once per instance to avoid
                    // log spam from a permanently wedged plugin.
                    if !self.state.timeout_warned.swap(true, Ordering::Relaxed) {
                        warn!(
                            node = %node,
                            "Plugin {op} timed out after {timeout:?}; \
                             the blocking task is still running and holds a CallGuard — \
                             in_flight_calls will remain elevated until it completes"
                        );
                    }
                    StreamKitError::Runtime(format!(
                        "Plugin {op} on node {node} timed out after {timeout:?}"
                    ))
                })?
                .map_err(|e| {
                    StreamKitError::Runtime(format!(
                        "Plugin {op} on node {node} task panicked: {e}"
                    ))
                }),
            None => task.await.map_err(|e| {
                StreamKitError::Runtime(format!("Plugin {op} on node {node} task panicked: {e}"))
            }),
        }
    }

    /// Input-driven processing loop (existing behaviour for processor plugins).
    #[allow(clippy::too_many_lines)]
    async fn run_processor(
        self: Box<Self>,
        mut context: NodeContext,
    ) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();

        tracing::info!(node = %node_name, "Native plugin wrapper starting (processor)");

        // Emit initializing state
        if let Err(e) = context
            .state_tx
            .send(NodeStateUpdate::new(node_name.clone(), NodeState::Initializing))
            .await
        {
            warn!(error = %e, node = %node_name, "Failed to send initializing state");
        }

        tracing::debug!(node = %node_name, "Getting input channels");

        let mut inputs = std::mem::take(&mut context.inputs);
        if inputs.is_empty() {
            return Err(StreamKitError::Runtime(
                "Engine did not provide any input pin receivers".to_string(),
            ));
        }

        let mut input_pin_names = Vec::with_capacity(inputs.len());
        let mut input_pin_cstrs = Vec::with_capacity(inputs.len());
        let mut input_tasks = Vec::with_capacity(inputs.len());
        let (merged_tx, mut merged_rx) =
            tokio::sync::mpsc::channel::<(usize, Packet)>(context.batch_size.max(1));
        let cancellation_token = context.cancellation_token.clone();
        let video_pool = context.video_pool.clone();
        let audio_pool = context.audio_pool.clone();

        for (pin_name, mut rx) in inputs.drain() {
            let pin_cstr = CString::new(pin_name.as_str()).map_err(|e| {
                StreamKitError::Runtime(format!("Invalid pin name '{pin_name}': {e}"))
            })?;
            let pin_index = input_pin_names.len();
            input_pin_names.push(pin_name);
            input_pin_cstrs.push(Arc::new(pin_cstr));

            let tx = merged_tx.clone();
            let token = cancellation_token.clone();
            let handle = tokio::spawn(async move {
                loop {
                    let packet = if let Some(token) = &token {
                        tokio::select! {
                            () = token.cancelled() => None,
                            packet = rx.recv() => packet,
                        }
                    } else {
                        rx.recv().await
                    };

                    let Some(packet) = packet else {
                        break;
                    };

                    if tx.send((pin_index, packet)).await.is_err() {
                        break;
                    }
                }
            });
            input_tasks.push(handle);
        }

        drop(merged_tx);

        tracing::debug!(
            node = %node_name,
            inputs = ?input_pin_names,
            "Got input channels, entering main loop"
        );

        // Emit running state
        if let Err(e) =
            context.state_tx.send(NodeStateUpdate::new(node_name.clone(), NodeState::Running)).await
        {
            warn!(error = %e, node = %node_name, "Failed to send running state");
        }

        let mut control_channel_open = true;

        // Main processing loop
        loop {
            tokio::select! {
                biased;

                () = async {
                    match &context.cancellation_token {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    tracing::info!("Native plugin cancelled");
                    break;
                }

                maybe_control = context.control_rx.recv(), if control_channel_open => {
                    match maybe_control {
                        Some(NodeControlMessage::UpdateParams(params_value)) => {
                            self.apply_params_update(&node_name, &params_value).await?;
                        }
                        Some(NodeControlMessage::Start) => {
                            // Native plugins don't implement ready/start lifecycle - ignore
                        }
                        Some(NodeControlMessage::Shutdown) => {
                            tracing::info!("Native plugin received shutdown signal");
                            break;
                        }
                        None => {
                            control_channel_open = false;
                        }
                    }
                }

                maybe_packet = merged_rx.recv() => {
                    let Some((pin_index, packet)) = maybe_packet else {
                        // Input closed - flush any buffered data before shutting down
                        tracing::debug!(node = %node_name, "Native plugin input closed, flushing buffers");

                        // Call flush to process any remaining buffered data
                        let state = Arc::clone(&self.state);
                        let telemetry_tx = context.telemetry_tx.clone();
                        let session_id = context.session_id.clone();
                        let node_id = node_name.clone();

                        let (outputs, error) = self.call_with_timeout("flush", &node_name, move || {
                            let Some(guard) = state.begin_call() else {
                                return (
                                    Vec::new(),
                                    Some("Instance destroyed during flush".to_string()),
                                );
                            };

                            let _lib = Arc::clone(&state.library);
                            let api = state.api();

                            let mut callback_ctx = CallbackContext {
                                output_packets: Vec::new(),
                                error: None,
                                telemetry_tx,
                                session_id,
                                node_id,
                                video_pool,
                                audio_pool,
                            };

                            let callback_data = (&raw mut callback_ctx).cast::<c_void>();
                            let node_callbacks = build_node_callbacks(callback_data);

                            // Call plugin's flush function
                            tracing::info!("Calling api.flush()");
                            let result = (api.flush)(
                                guard.handle(),
                                &raw const node_callbacks,
                            );
                            tracing::info!(success = result.success, "Flush returned");

                            let error = if result.success {
                                callback_ctx.error
                            } else {
                                let error_msg = if result.error_message.is_null() {
                                    "Plugin flush failed".to_string()
                                } else {
                                    unsafe {
                                        conversions::c_str_to_string(result.error_message)
                                            .unwrap_or_else(|_| "Plugin flush failed".to_string())
                                    }
                                };
                                Some(error_msg)
                            };

                            let outputs = callback_ctx.output_packets;
                            drop(guard);
                            (outputs, error)
                        })
                        .await?;

                        // Send flush outputs
                        for (pin, pkt) in outputs {
                            if context.output_sender.send(&pin, pkt).await.is_err() {
                                tracing::debug!("Output channel closed during flush");
                            }
                        }

                        if let Some(error_msg) = error {
                            warn!(node = %node_name, error = %error_msg, "Plugin flush failed");
                        }

                        break;
                    };

                    // Move the blocking FFI call to spawn_blocking to avoid blocking the async runtime
                    let state = Arc::clone(&self.state);
                    let telemetry_tx = context.telemetry_tx.clone();
                    let session_id = context.session_id.clone();
                    let node_id = node_name.clone();
                    let pin_cstr = Arc::clone(&input_pin_cstrs[pin_index]);
                    let video_pool = video_pool.clone();
                    let audio_pool = audio_pool.clone();
                    let (outputs, error) = self.call_with_timeout("process_packet", &node_name, move || {
                        let Some(guard) = state.begin_call() else {
                            return (
                                Vec::new(),
                                Some("Instance destroyed during process_packet".to_string()),
                            );
                        };

                        let _lib = Arc::clone(&state.library);
                        let api = state.api();
                        // Convert packet to C representation
                        let mut packet_repr = conversions::packet_to_c(&packet);

                        // v6 plugins do not understand BinaryWithMeta (discriminant 10).
                        // Downgrade to plain Binary so the raw bytes still arrive; the
                        // metadata/content_type fields are lost but the plugin won't crash.
                        // Note: no downgrade needed for EncodedAudio (discriminant 11)
                        // because it is a metadata-only type used in CPacketTypeInfo for
                        // pin declarations — it never appears in runtime CPacket transport.
                        if state.api_version < 7 {
                            packet_repr.downgrade_binary_with_meta();
                        }

                        // Create callback context
                        let mut callback_ctx = CallbackContext {
                            output_packets: Vec::new(),
                            error: None,
                            telemetry_tx,
                            session_id,
                            node_id,
                            video_pool,
                            audio_pool,
                        };

                        let callback_data = (&raw mut callback_ctx).cast::<c_void>();
                        let node_callbacks = build_node_callbacks(callback_data);

                        // Call plugin's process function (BLOCKING - but we're in call_with_timeout)
                        let result = (api.process_packet)(
                            guard.handle(),
                            pin_cstr.as_ptr(),
                            &raw const packet_repr.packet,
                            &raw const node_callbacks,
                        );

                        // Check for errors
                        let error = if result.success {
                            callback_ctx.error
                        } else {
                            let error_msg = if result.error_message.is_null() {
                                "Unknown plugin error".to_string()
                            } else {
                                // SAFETY: The error_message pointer is provided by the plugin
                                // and is valid for the duration of this call.
                                unsafe {
                                    conversions::c_str_to_string(result.error_message)
                                        .unwrap_or_else(|_| "Unknown plugin error".to_string())
                                }
                            };
                            Some(error_msg)
                        };

                        let outputs = callback_ctx.output_packets;
                        drop(guard);
                        (outputs, error)
                    })
                    .await?;

            // Now send outputs (after dropping c_packet and result)
            for (pin, pkt) in outputs {
                if context.output_sender.send(&pin, pkt).await.is_err() {
                    tracing::debug!("Output channel closed, stopping node");
                    break;
                }
            }

            // Handle errors
            if let Some(error_msg) = error {
                error!(node = %node_name, error = %error_msg, "Plugin process failed");

                if let Err(e) = context
                    .state_tx
                    .send(NodeStateUpdate::new(
                        node_name.clone(),
                        NodeState::Failed { reason: error_msg.clone() },
                    ))
                    .await
                {
                    warn!(error = %e, node = %node_name, "Failed to send failed state");
                }

                        for handle in &input_tasks {
                            handle.abort();
                        }
                        return Err(StreamKitError::Runtime(error_msg));
                    }
                }
            }
        }

        for handle in &input_tasks {
            handle.abort();
        }

        // Input closed, emit stopped state
        info!(node = %node_name, "Input closed, shutting down");
        if let Err(e) = context
            .state_tx
            .send(NodeStateUpdate::new(
                node_name.clone(),
                NodeState::Stopped { reason: StopReason::InputClosed },
            ))
            .await
        {
            warn!(error = %e, node = %node_name, "Failed to send stopped state");
        }

        Ok(())
    }

    /// Tick-driven loop for source plugins (no inputs, host drives timing).
    ///
    /// Lifecycle: Initializing → Ready → (wait for Start) → Running → tick loop → Stopped.
    #[allow(clippy::too_many_lines)]
    async fn run_source(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        // Defined up-front to satisfy `items_after_statements` lint.
        struct TickOutcome {
            outputs: Vec<(String, Packet)>,
            success: bool,
            done: bool,
            error_msg: Option<String>,
        }

        let node_name = context.output_sender.node_name().to_string();
        let video_pool = context.video_pool.clone();
        let audio_pool = context.audio_pool.clone();

        tracing::info!(node = %node_name, "Native source plugin wrapper starting");

        // Emit initializing state
        if let Err(e) = context
            .state_tx
            .send(NodeStateUpdate::new(node_name.clone(), NodeState::Initializing))
            .await
        {
            warn!(error = %e, node = %node_name, "Failed to send initializing state");
        }

        // ── Ready → Start handshake ─────────────────────────────────────
        // Emit Ready so the pipeline coordinator knows this node is waiting
        // for the Start signal before producing data.
        if let Err(e) =
            context.state_tx.send(NodeStateUpdate::new(node_name.clone(), NodeState::Ready)).await
        {
            warn!(error = %e, node = %node_name, "Failed to send ready state");
        }

        // Wait for Start (or Shutdown / cancellation).
        loop {
            tokio::select! {
                biased;

                () = async {
                    match &context.cancellation_token {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    tracing::info!(node = %node_name, "Source plugin cancelled before start");
                    if let Err(e) = context
                        .state_tx
                        .send(NodeStateUpdate::new(
                            node_name.clone(),
                            NodeState::Stopped { reason: StopReason::Completed },
                        ))
                        .await
                    {
                        warn!(error = %e, node = %node_name, "Failed to send stopped state");
                    }
                    return Ok(());
                }

                maybe_ctrl = context.control_rx.recv() => {
                    match maybe_ctrl {
                        Some(NodeControlMessage::Start) => {
                            tracing::info!(node = %node_name, "Source plugin received Start");
                            break; // proceed to tick loop
                        }
                        Some(NodeControlMessage::Shutdown) => {
                            tracing::info!(node = %node_name, "Source plugin received Shutdown before start");
                            if let Err(e) = context
                                .state_tx
                                .send(NodeStateUpdate::new(
                                    node_name.clone(),
                                    NodeState::Stopped { reason: StopReason::Completed },
                                ))
                                .await
                            {
                                warn!(error = %e, node = %node_name, "Failed to send stopped state");
                            }
                            return Ok(());
                        }
                        Some(NodeControlMessage::UpdateParams(params_value)) => {
                            // Apply parameter updates even before Start.
                            self.apply_params_update(&node_name, &params_value).await?;
                        }
                        None => {
                            // Control channel closed before Start — shut down gracefully.
                            if let Err(e) = context
                                .state_tx
                                .send(NodeStateUpdate::new(
                                    node_name.clone(),
                                    NodeState::Stopped { reason: StopReason::Completed },
                                ))
                                .await
                            {
                                warn!(error = %e, node = %node_name, "Failed to send stopped state");
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }

        // ── Running ─────────────────────────────────────────────────────
        if let Err(e) =
            context.state_tx.send(NodeStateUpdate::new(node_name.clone(), NodeState::Running)).await
        {
            warn!(error = %e, node = %node_name, "Failed to send running state");
        }

        // Re-query source config from the live instance (created with actual
        // params) so per-instance values like `max_ticks` (from `frame_count`)
        // override the defaults obtained during the load-time probe.
        let fallback = || {
            let ti = std::time::Duration::from_micros(self.metadata.tick_interval_us.max(1));
            (ti, self.metadata.max_ticks)
        };
        // TODO: get_source_config is synchronous (no timeout, no catch_unwind) —
        // same concern as runtime_param_schema.  Needs async refactor for full coverage.
        let (tick_interval, max_ticks) = self
            .state
            .api()
            .get_source_config
            .and_then(|get_source_config_fn| {
                self.state.begin_call().map(|guard| {
                    let cfg = get_source_config_fn(guard.handle());
                    let ti = std::time::Duration::from_micros(cfg.tick_interval_us.max(1));
                    (ti, cfg.max_ticks)
                })
            })
            .unwrap_or_else(fallback);
        let mut tick_count: u64 = 0;

        let tick_fn = self.state.api().tick.ok_or_else(|| {
            StreamKitError::Runtime("Source plugin missing tick function".to_string())
        })?;

        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the first (immediate) tick so we don't double-fire on entry.
        interval.tick().await;

        // Hint receivers from downstream consumers, delivered via
        // OutputHintChannel pin management messages.  Keyed by pin name
        // so multi-output sources can distinguish which output the hint
        // targets (currently single-output only, but future-proofed).
        let mut hint_receivers: Vec<(
            String,
            tokio::sync::mpsc::Receiver<streamkit_core::UpstreamHint>,
        )> = Vec::new();

        loop {
            // Check tick limit
            if max_ticks > 0 && tick_count >= max_ticks {
                tracing::info!(node = %node_name, ticks = tick_count, "Source reached max ticks");
                break;
            }

            // Non-blocking drain of pending control messages.
            while let Ok(ctrl) = context.control_rx.try_recv() {
                match ctrl {
                    NodeControlMessage::Shutdown => {
                        tracing::info!(node = %node_name, "Source plugin received shutdown");
                        if let Err(e) = context
                            .state_tx
                            .send(NodeStateUpdate::new(
                                node_name.clone(),
                                NodeState::Stopped { reason: StopReason::Completed },
                            ))
                            .await
                        {
                            warn!(error = %e, node = %node_name, "Failed to send stopped state");
                        }
                        return Ok(());
                    },
                    NodeControlMessage::UpdateParams(params_value) => {
                        self.apply_params_update(&node_name, &params_value).await?;
                    },
                    NodeControlMessage::Start => {
                        // Already started — ignore duplicate.
                    },
                }
            }

            // Non-blocking drain of pin management messages to pick up
            // OutputHintChannel deliveries from the engine.
            // NOTE: this consumes ALL variants but only extracts
            // OutputHintChannel.  Safe today because source plugins
            // don't receive AddedOutputPin/RemoveOutputPin/InputTypeResolved.
            // If dynamic output pins are added to sources in the future,
            // this drain must be updated to handle those variants.
            if let Some(ref mut pin_mgmt_rx) = context.pin_management_rx {
                while let Ok(msg) = pin_mgmt_rx.try_recv() {
                    if let streamkit_core::pins::PinManagementMessage::OutputHintChannel {
                        pin_name: ref pn,
                        hint_rx,
                    } = msg
                    {
                        tracing::info!(node = %node_name, pin = %pn, "Received OutputHintChannel from engine");
                        hint_receivers.push((pn.clone(), hint_rx));
                    }
                }
            }

            // Drain all hint receivers and deliver to plugin via C ABI.
            // Retain only receivers whose channels are still open.
            // First collect pending hints (non-blocking), then deliver
            // via call_with_timeout to avoid blocking the tokio worker
            // and to keep timeout hardening consistent across all FFI
            // call sites.  Hints are best-effort — timeout errors are
            // ignored.
            if !hint_receivers.is_empty() {
                if let Some(on_hint_fn) = self.state.api().on_upstream_hint {
                    let mut pending_hints: Vec<std::ffi::CString> = Vec::new();
                    hint_receivers.retain_mut(|(_pin, rx)| loop {
                        match rx.try_recv() {
                            Ok(hint) => {
                                tracing::info!(node = %node_name, ?hint, "Delivering upstream hint to plugin");
                                if let Ok(json) = serde_json::to_string(&hint) {
                                    if let Ok(c_str) = std::ffi::CString::new(json) {
                                        pending_hints.push(c_str);
                                    }
                                }
                            },
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return true,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                return false
                            },
                        }
                    });
                    if !pending_hints.is_empty() {
                        let state = Arc::clone(&self.state);
                        let _ = self
                            .call_with_timeout("on_upstream_hint", &node_name, move || {
                                for c_str in &pending_hints {
                                    if let Some(guard) = state.begin_call() {
                                        let _ = on_hint_fn(guard.handle(), c_str.as_ptr());
                                    }
                                }
                            })
                            .await;
                    }
                }
            }

            // ── Tick ────────────────────────────────────────────────────
            let state = Arc::clone(&self.state);
            let telemetry_tx = context.telemetry_tx.clone();
            let session_id = context.session_id.clone();
            let node_id = node_name.clone();
            let video_pool = video_pool.clone();
            let audio_pool = audio_pool.clone();
            let outcome = self
                .call_with_timeout("tick", &node_name, move || {
                    let Some(guard) = state.begin_call() else {
                        return TickOutcome {
                            outputs: Vec::new(),
                            success: false,
                            done: false,
                            error_msg: Some("Instance handle is null".to_string()),
                        };
                    };

                    let _lib = Arc::clone(&state.library);

                    let mut callback_ctx = CallbackContext {
                        output_packets: Vec::new(),
                        error: None,
                        telemetry_tx,
                        session_id,
                        node_id,
                        video_pool,
                        audio_pool,
                    };

                    let callback_data = (&raw mut callback_ctx).cast::<c_void>();
                    let node_callbacks = build_node_callbacks(callback_data);

                    let result = tick_fn(guard.handle(), &raw const node_callbacks);

                    // Extract error string while pointers are still valid.
                    let error_msg = if result.result.success {
                        callback_ctx.error
                    } else if result.result.error_message.is_null() {
                        Some("Source tick failed".to_string())
                    } else {
                        Some(unsafe {
                            conversions::c_str_to_string(result.result.error_message)
                                .unwrap_or_else(|_| "Source tick failed".to_string())
                        })
                    };

                    let outputs = callback_ctx.output_packets;
                    drop(guard);

                    TickOutcome {
                        outputs,
                        success: result.result.success,
                        done: result.done,
                        error_msg,
                    }
                })
                .await?;

            // Send outputs produced by tick.  If the output channel is closed,
            // stop ticking — source nodes have no input-close backstop so we must
            // detect consumer disconnect here.
            let mut output_closed = false;
            for (pin, pkt) in outcome.outputs {
                if context.output_sender.send(&pin, pkt).await.is_err() {
                    tracing::debug!(node = %node_name, "Output channel closed during tick");
                    output_closed = true;
                    break;
                }
            }
            if output_closed {
                break;
            }

            tick_count += 1;

            // Check tick result
            if !outcome.success {
                let error_msg =
                    outcome.error_msg.unwrap_or_else(|| "Source tick failed".to_string());
                error!(node = %node_name, error = %error_msg, "Source tick error");
                if let Err(e) = context
                    .state_tx
                    .send(NodeStateUpdate::new(
                        node_name.clone(),
                        NodeState::Failed { reason: error_msg.clone() },
                    ))
                    .await
                {
                    warn!(error = %e, node = %node_name, "Failed to send failed state");
                }
                return Err(StreamKitError::Runtime(error_msg));
            }

            if outcome.done {
                tracing::info!(node = %node_name, ticks = tick_count, "Source signalled done");
                break;
            }

            // Wait for next tick — cancellation-aware so shutdown is responsive.
            tokio::select! {
                biased;
                () = async {
                    match &context.cancellation_token {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    tracing::info!(node = %node_name, "Source plugin cancelled during tick wait");
                    break;
                }
                _ = interval.tick() => {}
            }
        }

        // Emit stopped state
        if let Err(e) = context
            .state_tx
            .send(NodeStateUpdate::new(
                node_name.clone(),
                NodeState::Stopped { reason: StopReason::Completed },
            ))
            .await
        {
            warn!(error = %e, node = %node_name, "Failed to send stopped state");
        }

        Ok(())
    }

    /// Helper to apply a parameter update via the C ABI.
    async fn apply_params_update(
        &self,
        node_name: &str,
        params_value: &serde_json::Value,
    ) -> Result<(), StreamKitError> {
        let params_json = serde_json::to_string(params_value).map_err(|e| {
            StreamKitError::Configuration(format!("Failed to serialize params: {e}"))
        })?;
        let params_cstr = CString::new(params_json)
            .map_err(|e| StreamKitError::Configuration(format!("Invalid params string: {e}")))?;

        let state = Arc::clone(&self.state);
        let error_msg = self
            .call_with_timeout("update_params", node_name, move || {
                let Some(guard) = state.begin_call() else {
                    return Some("Instance destroyed during update_params".to_string());
                };

                let _lib = Arc::clone(&state.library);
                let api = state.api();
                let result = (api.update_params)(guard.handle(), params_cstr.as_ptr());

                if result.success {
                    None
                } else if result.error_message.is_null() {
                    Some("Failed to update parameters".to_string())
                } else {
                    unsafe {
                        Some(
                            conversions::c_str_to_string(result.error_message)
                                .unwrap_or_else(|_| "Failed to update parameters".to_string()),
                        )
                    }
                }
            })
            .await?;

        if let Some(err) = error_msg {
            warn!(node = %node_name, error = %err, "Parameter update failed");
        }

        Ok(())
    }
}

impl Drop for NativeNodeWrapper {
    fn drop(&mut self) {
        self.state.request_drop();
    }
}

/// Context passed to the output callback
struct CallbackContext {
    output_packets: Vec<(String, Packet)>,
    error: Option<String>,
    telemetry_tx: Option<tokio::sync::mpsc::Sender<TelemetryEvent>>,
    session_id: Option<String>,
    node_id: String,
    video_pool: Option<Arc<VideoFramePool>>,
    audio_pool: Option<Arc<AudioFramePool>>,
}

/// Free any pool-allocated `buffer_handle` embedded in a raw [`CPacket`].
///
/// This is the safety net for error paths in [`output_callback_shim`]: if
/// `packet_from_c` is never called (e.g. invalid pin name) or if it fails
/// before reclaiming the handle, the pooled buffer would leak because the
/// SDK already marked it as consumed (suppressing `Drop`).
///
/// # Safety
///
/// `c_packet` must be a valid, non-null pointer to a [`CPacket`].
unsafe fn free_packet_buffer_handle(c_packet: *const CPacket) {
    use streamkit_core::frame_pool::{PooledSamples, PooledVideoData};
    use streamkit_plugin_sdk_native::types::CPacketType;

    let pkt = &*c_packet;
    if pkt.data.is_null() {
        return;
    }
    match pkt.packet_type {
        CPacketType::RawVideo => {
            let frame = &*pkt.data.cast::<streamkit_plugin_sdk_native::types::CVideoFrame>();
            if !frame.buffer_handle.is_null() {
                drop(Box::from_raw(frame.buffer_handle.cast::<PooledVideoData>()));
            }
        },
        CPacketType::RawAudio => {
            let frame = &*pkt.data.cast::<streamkit_plugin_sdk_native::types::CAudioFrame>();
            if !frame.buffer_handle.is_null() {
                drop(Box::from_raw(frame.buffer_handle.cast::<PooledSamples>()));
            }
        },
        _ => {},
    }
}

/// RAII guard for a converted [`Packet`].
///
/// Wraps the gap between [`ConvertedPacketGuard::new`] and
/// [`consume()`](Self::consume).  In the current code that gap is a
/// single expression (`push(guard.consume())`), so the guard
/// effectively only fires if the `Vec::push` allocation itself panics
/// — a narrow but real edge case.
///
/// If the guard is dropped without consuming (i.e. during panic
/// unwinding), [`Packet::drop`] returns pooled buffers to their pool
/// and a warning is logged.
struct ConvertedPacketGuard(Option<(String, Packet)>);

impl ConvertedPacketGuard {
    const fn new(pin: String, packet: Packet) -> Self {
        Self(Some((pin, packet)))
    }

    /// Take ownership of the guarded entry for pushing into output_packets.
    fn consume(mut self) -> (String, Packet) {
        // SAFETY: always constructed with Some; consumed at most once (self is moved).
        #[allow(clippy::unwrap_used)]
        self.0.take().unwrap()
    }
}

impl Drop for ConvertedPacketGuard {
    fn drop(&mut self) {
        if self.0.is_some() {
            // Panic unwinding — Packet::drop returns pooled buffers.
            warn!("output_callback_shim: converted packet dropped by panic guard");
        }
    }
}

/// C callback function for sending output packets.
/// This collects packets and they are sent asynchronously after the callback returns.
extern "C" fn output_callback_shim(
    pin_name: *const std::os::raw::c_char,
    c_packet: *const CPacket,
    user_data: *mut c_void,
) -> CResult {
    ffi_guard_result(|| {
        if pin_name.is_null() || c_packet.is_null() || user_data.is_null() {
            return CResult::error(std::ptr::null());
        }

        // SAFETY: user_data is a valid pointer to CallbackContext that we passed to process_packet.
        // The pointer remains valid for the duration of this callback.
        let ctx = unsafe { &mut *user_data.cast::<CallbackContext>() };

        // SAFETY: pin_name is a valid C string pointer provided by the plugin.
        let pin_str = match unsafe { conversions::c_str_to_string(pin_name) } {
            Ok(s) => s,
            Err(e) => {
                ctx.error = Some(format!("Invalid pin name: {e}"));
                // Free any pooled buffer the plugin already consumed.
                unsafe { free_packet_buffer_handle(c_packet) };
                return CResult::error(std::ptr::null());
            },
        };

        // SAFETY: c_packet is a valid pointer to CPacket provided by the plugin.
        let packet = match unsafe { conversions::packet_from_c(c_packet) } {
            Ok(p) => p,
            Err(e) => {
                // packet_from_c already frees the buffer_handle on its own error
                // paths (Critical #1), so no extra cleanup needed here.
                ctx.error = Some(format!("Failed to convert packet: {e}"));
                return CResult::error(std::ptr::null());
            },
        };

        // RAII guard — see ConvertedPacketGuard doc for scope.
        let guard = ConvertedPacketGuard::new(pin_str, packet);
        ctx.output_packets.push(guard.consume());

        CResult::success()
    })
}

/// C callback function for emitting telemetry events.
///
/// Telemetry is best-effort: failures are logged and the callback returns success to avoid
/// impacting the main data path.
extern "C" fn telemetry_callback_shim(
    event_type: *const std::os::raw::c_char,
    data_json: *const u8,
    data_len: usize,
    metadata: *const streamkit_plugin_sdk_native::types::CPacketMetadata,
    user_data: *mut c_void,
) -> CResult {
    ffi_guard_result(|| {
        if event_type.is_null() || user_data.is_null() {
            return CResult::success();
        }

        let ctx = unsafe { &mut *user_data.cast::<CallbackContext>() };
        let Some(ref tx) = ctx.telemetry_tx else {
            return CResult::success();
        };

        let event_type_str = match unsafe { conversions::c_str_to_string(event_type) } {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, node = %ctx.node_id, "Invalid telemetry event_type");
                return CResult::success();
            },
        };

        let data_value = if data_json.is_null() || data_len == 0 {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(data_json, data_len) };
            match serde_json::from_slice::<serde_json::Value>(bytes) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, node = %ctx.node_id, event_type = %event_type_str, "Invalid telemetry JSON payload");
                    return CResult::success();
                },
            }
        };

        let timestamp_us = if metadata.is_null() {
            None
        } else {
            let meta = unsafe { &*metadata };
            if meta.has_timestamp_us {
                Some(meta.timestamp_us)
            } else {
                None
            }
        }
        .unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| u64::try_from(d.as_micros()).ok())
                .unwrap_or(0)
        });

        let mut event_data = match data_value {
            serde_json::Value::Object(map) => serde_json::Value::Object(map),
            other => serde_json::json!({ "value": other }),
        };

        if let Some(obj) = event_data.as_object_mut() {
            obj.insert("event_type".to_string(), serde_json::Value::String(event_type_str));
        }

        let event = TelemetryEvent::new(
            ctx.session_id.clone(),
            ctx.node_id.clone(),
            event_data,
            timestamp_us,
        );

        if tx.try_send(event).is_err() {
            // Drop silently: best-effort.
        }

        CResult::success()
    })
}

// ── Frame pool allocation shims (v6) ─────────────────────────────────────

/// Allocate a video buffer from the host's frame pool.
extern "C" fn alloc_video_shim(min_bytes: usize, user_data: *mut c_void) -> CAllocVideoResult {
    ffi_guard_alloc_video(|| {
        use streamkit_core::frame_pool::PooledVideoData;

        if user_data.is_null() {
            return CAllocVideoResult::null();
        }

        let ctx = unsafe { &*user_data.cast::<CallbackContext>() };
        let Some(pool) = ctx.video_pool.as_ref() else {
            return CAllocVideoResult::null();
        };

        let mut pooled: PooledVideoData = pool.get(min_bytes);
        let data_ptr = pooled.as_mut_ptr();
        let len = pooled.len();
        let handle = Box::into_raw(Box::new(pooled)).cast::<c_void>();

        CAllocVideoResult { data: data_ptr, len, handle, free_fn: Some(free_video_buffer) }
    })
}

/// Free a video buffer without sending it (error/discard path).
extern "C" fn free_video_buffer(handle: *mut c_void) {
    ffi_guard_unit(|| {
        use streamkit_core::frame_pool::PooledVideoData;

        if !handle.is_null() {
            // SAFETY: handle was created by alloc_video_shim via Box::into_raw.
            let _ = unsafe { Box::from_raw(handle.cast::<PooledVideoData>()) };
        }
    });
}

/// Allocate an audio buffer from the host's frame pool.
extern "C" fn alloc_audio_shim(min_samples: usize, user_data: *mut c_void) -> CAllocAudioResult {
    ffi_guard_alloc_audio(|| {
        use streamkit_core::frame_pool::PooledSamples;

        if user_data.is_null() {
            return CAllocAudioResult::null();
        }

        let ctx = unsafe { &*user_data.cast::<CallbackContext>() };
        let Some(pool) = ctx.audio_pool.as_ref() else {
            return CAllocAudioResult::null();
        };

        let mut pooled: PooledSamples = pool.get(min_samples);
        let data_ptr = pooled.as_mut_ptr();
        let sample_count = pooled.len();
        let handle = Box::into_raw(Box::new(pooled)).cast::<c_void>();

        CAllocAudioResult { data: data_ptr, sample_count, handle, free_fn: Some(free_audio_buffer) }
    })
}

/// Free an audio buffer without sending it (error/discard path).
extern "C" fn free_audio_buffer(handle: *mut c_void) {
    ffi_guard_unit(|| {
        use streamkit_core::frame_pool::PooledSamples;

        if !handle.is_null() {
            let _ = unsafe { Box::from_raw(handle.cast::<PooledSamples>()) };
        }
    });
}

/// Build a `CNodeCallbacks` struct from a `CallbackContext` pointer.
///
/// The returned struct borrows `callback_data` — it must not outlive the
/// `CallbackContext`.
fn build_node_callbacks(callback_data: *mut c_void) -> CNodeCallbacks {
    CNodeCallbacks {
        struct_size: std::mem::size_of::<CNodeCallbacks>(),
        output_callback: output_callback_shim,
        output_user_data: callback_data,
        telemetry_callback: Some(telemetry_callback_shim),
        telemetry_user_data: callback_data,
        alloc_video: Some(alloc_video_shim),
        alloc_audio: Some(alloc_audio_shim),
        alloc_user_data: callback_data,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ffi_guard_tests {
    use super::*;

    #[test]
    fn host_guard_result_catches_panic_and_preserves_message() {
        let r = ffi_guard_result(|| panic!("host boom"));
        assert!(!r.success);
        assert!(!r.error_message.is_null());
        let msg = unsafe { conversions::c_str_to_string(r.error_message) }.expect("valid UTF-8");
        assert!(msg.contains("host boom"), "expected panic message, got: {msg}");
    }

    #[test]
    fn host_guard_alloc_video_catches_panic() {
        let r = ffi_guard_alloc_video(|| panic!("video alloc boom"));
        assert!(r.data.is_null());
    }

    #[test]
    fn host_guard_alloc_audio_catches_panic() {
        let r = ffi_guard_alloc_audio(|| panic!("audio alloc boom"));
        assert!(r.data.is_null());
    }

    #[test]
    fn host_guard_unit_catches_panic() {
        ffi_guard_unit(|| panic!("unit boom"));
    }

    // ── Real shim tests ────────────────────────────────────────────────
    //
    // These exercise the actual `extern "C"` callback shims (not just the
    // guard helpers) so that accidentally removing a guard from a shim
    // body would be caught by CI.

    #[test]
    fn output_callback_shim_null_args_returns_error() {
        let r = output_callback_shim(std::ptr::null(), std::ptr::null(), std::ptr::null_mut());
        assert!(!r.success);
    }

    #[test]
    fn output_callback_shim_valid_text_packet() {
        let mut ctx = CallbackContext {
            output_packets: Vec::new(),
            error: None,
            telemetry_tx: None,
            session_id: None,
            node_id: "test-node".to_string(),
            video_pool: None,
            audio_pool: None,
        };
        let user_data = (&raw mut ctx).cast::<c_void>();

        let pin = CString::new("output").expect("valid pin");
        // packet_from_c reads Text packets via CStr::from_ptr, so the
        // data must be a null-terminated C string.
        let text = CString::new("hello").expect("valid text");
        let c_packet = CPacket {
            packet_type: streamkit_plugin_sdk_native::types::CPacketType::Text,
            data: text.as_ptr().cast(),
            len: text.as_bytes_with_nul().len(),
        };

        let r = output_callback_shim(pin.as_ptr(), &raw const c_packet, user_data);
        assert!(r.success, "expected success for valid text packet");
        assert_eq!(ctx.output_packets.len(), 1);
        assert_eq!(ctx.output_packets[0].0, "output");
    }

    #[test]
    fn telemetry_callback_shim_null_args_returns_success() {
        // telemetry is best-effort — null args should not panic
        let r = telemetry_callback_shim(
            std::ptr::null(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        assert!(r.success);
    }

    #[test]
    fn alloc_video_shim_null_user_data_returns_null() {
        let r = alloc_video_shim(1024, std::ptr::null_mut());
        assert!(r.data.is_null());
    }

    #[test]
    fn alloc_audio_shim_null_user_data_returns_null() {
        let r = alloc_audio_shim(960, std::ptr::null_mut());
        assert!(r.data.is_null());
    }

    // ── CallGuard / ApiPtr tests ───────────────────────────────────────

    /// Dummy `extern "C"` stubs used to populate a test `CNativePluginAPI`.
    mod test_stubs {
        use super::*;
        use streamkit_plugin_sdk_native::types::{CNodeCallbacks, CNodeMetadata};

        pub extern "C" fn get_metadata() -> *const CNodeMetadata {
            std::ptr::null()
        }
        pub extern "C" fn create_instance(
            _: *const std::os::raw::c_char,
            _: streamkit_plugin_sdk_native::types::CLogCallback,
            _: *mut c_void,
        ) -> CPluginHandle {
            std::ptr::null_mut()
        }
        pub extern "C" fn process_packet(
            _: CPluginHandle,
            _: *const std::os::raw::c_char,
            _: *const CPacket,
            _: *const CNodeCallbacks,
        ) -> CResult {
            CResult::success()
        }
        pub extern "C" fn update_params(
            _: CPluginHandle,
            _: *const std::os::raw::c_char,
        ) -> CResult {
            CResult::success()
        }
        pub extern "C" fn flush(_: CPluginHandle, _: *const CNodeCallbacks) -> CResult {
            CResult::success()
        }
        pub extern "C" fn destroy_instance(_: CPluginHandle) {}
    }

    /// Build a valid `CNativePluginAPI` populated with no-op stubs.
    fn dummy_api() -> CNativePluginAPI {
        CNativePluginAPI {
            version: 8,
            get_metadata: test_stubs::get_metadata,
            create_instance: test_stubs::create_instance,
            process_packet: test_stubs::process_packet,
            update_params: test_stubs::update_params,
            flush: test_stubs::flush,
            destroy_instance: test_stubs::destroy_instance,
            get_source_config: None,
            tick: None,
            get_runtime_param_schema: None,
            on_upstream_hint: None,
        }
    }

    /// Helper: build a minimal `InstanceState` for guard tests.
    fn test_instance_state() -> Arc<InstanceState> {
        // SAFETY: loading libc is harmless; we never call any symbols from it.
        let lib = unsafe { Library::new("libc.so.6").expect("libc must be loadable") };
        let api: &'static CNativePluginAPI = Box::leak(Box::new(dummy_api()));
        Arc::new(InstanceState::new(
            Arc::new(lib),
            api,
            std::ptr::without_provenance_mut::<c_void>(1), // non-null dummy handle
            8,
            None,
        ))
    }

    #[test]
    fn call_guard_drops_on_normal_path() {
        let state = test_instance_state();
        {
            let guard = state.begin_call().expect("begin_call should succeed");
            assert_eq!(state.in_flight_calls.load(Ordering::Acquire), 1);
            let _h = guard.handle();
        }
        assert_eq!(state.in_flight_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn call_guard_drops_on_panic() {
        let state = test_instance_state();
        let state2 = Arc::clone(&state);
        let result = std::panic::catch_unwind(AssertUnwindSafe(move || {
            let _guard = state2.begin_call().expect("begin_call should succeed");
            panic!("boom");
        }));
        assert!(result.is_err(), "should have panicked");
        assert_eq!(
            state.in_flight_calls.load(Ordering::Acquire),
            0,
            "guard must decrement in_flight_calls even on panic"
        );
    }

    #[test]
    fn begin_call_returns_none_after_request_drop() {
        let state = test_instance_state();
        state.request_drop();
        // After request_drop the handle is swapped to null, so begin_call
        // must return None.
        assert!(state.begin_call().is_none(), "begin_call must return None after request_drop");
        assert_eq!(
            state.in_flight_calls.load(Ordering::Acquire),
            0,
            "failed begin_call must not leave in_flight_calls elevated"
        );
    }

    /// Stress test: hammer `begin_call` from many threads while another
    /// thread calls `request_drop`.  Asserts that after all threads join:
    /// - `in_flight_calls` is 0  (no stranded counts)
    /// - `handle` is null        (destroy was called exactly once)
    /// - `drop_requested` is set
    ///
    /// This exercises the Dekker pair and the rollback-triggers-destroy
    /// path that was missing before the fix.
    #[test]
    fn concurrent_begin_call_and_request_drop() {
        use std::sync::Barrier;

        for _round in 0..50 {
            let state = test_instance_state();
            let num_callers = 8;
            let barrier = Arc::new(Barrier::new(num_callers + 1));

            let mut handles = Vec::new();

            // Spawn begin_call threads
            for _ in 0..num_callers {
                let s = Arc::clone(&state);
                let b = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    b.wait();
                    // Try to begin a call; if we get a guard, hold it
                    // briefly then drop it.
                    let _guard = s.begin_call();
                }));
            }

            // Spawn request_drop thread
            {
                let s = Arc::clone(&state);
                let b = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    b.wait();
                    s.request_drop();
                }));
            }

            for h in handles {
                h.join().expect("thread panicked");
            }

            assert_eq!(
                state.in_flight_calls.load(Ordering::SeqCst),
                0,
                "in_flight_calls must be 0 after all threads complete"
            );
            assert!(
                state.handle.load(Ordering::SeqCst).is_null(),
                "handle must be null (destroy_instance must have been called)"
            );
            assert!(state.drop_requested.load(Ordering::SeqCst), "drop_requested must be set");
        }
    }

    // Pointer-equality alone does not prove provenance is preserved.
    // Under Miri this test authoritatively validates that ApiPtr never
    // strips provenance.  Outside Miri, it is a no-op.
    // TODO: add `cargo +nightly miri test -p streamkit-plugin-native` to CI.
    #[test]
    #[cfg(miri)]
    fn api_ptr_preserves_provenance() {
        let api: &'static CNativePluginAPI = Box::leak(Box::new(dummy_api()));
        let original_addr = std::ptr::from_ref(api);
        let wrapper = ApiPtr(original_addr);
        let recovered: *const CNativePluginAPI = wrapper.0;
        assert_eq!(
            original_addr, recovered,
            "ApiPtr must round-trip the pointer without losing provenance"
        );
    }
}
