// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Native Node Wrapper
//!
//! This module provides the `NativeNodeWrapper` which implements the `ProcessorNode` trait
//! and bridges to the C ABI plugin interface.

use anyhow::Result;
use async_trait::async_trait;
use libloading::Library;
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::telemetry::TelemetryEvent;
use streamkit_core::types::Packet;
use streamkit_core::{
    InputPin, NodeContext, NodeState, NodeStateUpdate, OutputPin, ProcessorNode, StopReason,
    StreamKitError,
};
use streamkit_plugin_sdk_native::{
    conversions,
    types::{CNativePluginAPI, CPacket, CPluginHandle, CResult},
};
use tracing::{error, info, warn};

use crate::PluginMetadata;

struct InstanceState {
    library: Arc<Library>,
    api_addr: usize,
    handle_addr: AtomicUsize,
    in_flight_calls: AtomicUsize,
    drop_requested: AtomicBool,
}

impl InstanceState {
    fn new(library: Arc<Library>, api: &'static CNativePluginAPI, handle: CPluginHandle) -> Self {
        Self {
            library,
            api_addr: std::ptr::from_ref(api) as usize,
            handle_addr: AtomicUsize::new(handle as usize),
            in_flight_calls: AtomicUsize::new(0),
            drop_requested: AtomicBool::new(false),
        }
    }

    const fn api(&self) -> &'static CNativePluginAPI {
        // SAFETY: api_addr was created from a valid &'static CNativePluginAPI reference.
        // The loaded library is kept alive by self.library (Arc<Library>) held by this state,
        // which is itself held by any in-flight spawn_blocking tasks.
        unsafe { &*(self.api_addr as *const CNativePluginAPI) }
    }

    fn begin_call(&self) -> Option<CPluginHandle> {
        self.in_flight_calls.fetch_add(1, Ordering::AcqRel);

        let handle_addr = self.handle_addr.load(Ordering::Acquire);
        if handle_addr == 0 {
            self.in_flight_calls.fetch_sub(1, Ordering::AcqRel);
            return None;
        }

        Some(handle_addr as CPluginHandle)
    }

    fn finish_call(&self) {
        let prev = self.in_flight_calls.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "finish_call called without begin_call");

        if prev == 1 && self.drop_requested.load(Ordering::Acquire) {
            self.destroy_instance();
        }
    }

    fn request_drop(&self) {
        self.drop_requested.store(true, Ordering::Release);
        if self.in_flight_calls.load(Ordering::Acquire) == 0 {
            self.destroy_instance();
        }
    }

    fn destroy_instance(&self) {
        let handle_addr = self.handle_addr.swap(0, Ordering::AcqRel);
        if handle_addr == 0 {
            return;
        }

        // Keep the library alive for the duration of the destroy call.
        let _lib = Arc::clone(&self.library);
        let api = self.api();
        (api.destroy_instance)(handle_addr as CPluginHandle);
    }
}

/// C callback function for plugin logging
/// Routes plugin logs to the tracing infrastructure
#[allow(clippy::cognitive_complexity)]
extern "C" fn plugin_log_callback(
    level: streamkit_plugin_sdk_native::types::CLogLevel,
    target: *const std::os::raw::c_char,
    message: *const std::os::raw::c_char,
    _user_data: *mut c_void,
) {
    use streamkit_plugin_sdk_native::{conversions, types::CLogLevel};

    // Convert C strings to Rust strings
    let target_str = if target.is_null() {
        "unknown".to_string()
    } else {
        unsafe { conversions::c_str_to_string(target) }.unwrap_or_else(|_| "unknown".to_string())
    };

    let message_str = if message.is_null() {
        String::new()
    } else {
        unsafe { conversions::c_str_to_string(message) }
            .unwrap_or_else(|_| "[invalid UTF-8]".to_string())
    };

    // Route to tracing based on log level
    // Use the event! macro which allows dynamic targets
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

        Ok(Self { state: Arc::new(InstanceState::new(library, api, handle)), metadata })
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

    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        let get_schema = self.state.api().get_runtime_param_schema?;
        let handle = self.state.begin_call()?;

        let result = get_schema(handle);

        if !result.success {
            // FFI call failed — log and return None.
            if !result.error_message.is_null() {
                let msg = unsafe { conversions::c_str_to_string(result.error_message) }
                    .unwrap_or_default();
                warn!(error = %msg, "Plugin runtime_param_schema failed");
            }
            self.state.finish_call();
            return None;
        }

        // success=true, null json_schema → plugin has no runtime schema.
        if result.json_schema.is_null() {
            self.state.finish_call();
            return None;
        }

        // success=true, non-null json_schema → JSON string containing the schema.
        // SAFETY: result.json_schema points to a thread-local CString set by
        // error_to_c (used here as a generic "String → *const c_char" helper).
        // We must copy the string BEFORE any other FFI call on this thread
        // (including finish_call) that could invoke error_to_c again and
        // overwrite the thread-local buffer.
        let json_str = unsafe { conversions::c_str_to_string(result.json_schema) }.ok();
        self.state.finish_call();
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
                            // Serialize params to JSON string
                            let params_json = serde_json::to_string(&params_value)
                                .map_err(|e| StreamKitError::Configuration(format!("Failed to serialize params: {e}")))?;
                            let params_cstr = CString::new(params_json)
                                .map_err(|e| StreamKitError::Configuration(format!("Invalid params string: {e}")))?;

                            // Move the blocking FFI call to spawn_blocking
                            let state = Arc::clone(&self.state);
                            let error_msg = tokio::task::spawn_blocking(move || {
                                let handle = state.begin_call()?;

                                let _lib = Arc::clone(&state.library);
                                let api = state.api();
                                let result = (api.update_params)(handle, params_cstr.as_ptr());

                                // Convert error message immediately to String (CResult is not Send)
                                let error = if result.success {
                                    None
                                } else if result.error_message.is_null() {
                                    Some("Failed to update parameters".to_string())
                                } else {
                                    // SAFETY: The error_message pointer is provided by the plugin
                                    // and is valid for the duration of this call.
                                    unsafe {
                                        Some(conversions::c_str_to_string(result.error_message)
                                            .unwrap_or_else(|_| "Failed to update parameters".to_string()))
                                    }
                                };

                                state.finish_call();
                                error
                            })
                            .await
                            .map_err(|e| {
                                StreamKitError::Runtime(format!(
                                    "Update params task panicked: {e}"
                                ))
                            })?;

                            if let Some(err) = error_msg {
                                warn!(node = %node_name, error = %err, "Parameter update failed");
                            }
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

                        let (outputs, error) = tokio::task::spawn_blocking(move || {
                            let Some(handle) = state.begin_call() else {
                                return (Vec::new(), None);
                            };

                            let _lib = Arc::clone(&state.library);
                            let api = state.api();

                            let mut callback_ctx = CallbackContext {
                                output_packets: Vec::new(),
                                error: None,
                                telemetry_tx,
                                session_id,
                                node_id,
                            };

                            let callback_data = (&raw mut callback_ctx).cast::<c_void>();

                            // Call plugin's flush function
                            tracing::info!("Calling api.flush()");
                            let result = (api.flush)(
                                handle,
                                output_callback_shim,
                                callback_data,
                                Some(telemetry_callback_shim),
                                callback_data,
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
                            state.finish_call();
                            (outputs, error)
                        })
                        .await
                        .map_err(|e| StreamKitError::Runtime(format!("Plugin flush task panicked: {e}")))?;

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
                    let (outputs, error) = tokio::task::spawn_blocking(move || {
                        let Some(handle) = state.begin_call() else {
                            return (Vec::new(), None);
                        };

                        let _lib = Arc::clone(&state.library);
                        let api = state.api();
                        // Convert packet to C representation
                        let packet_repr = conversions::packet_to_c(&packet);

                        // Create callback context
                        let mut callback_ctx = CallbackContext {
                            output_packets: Vec::new(),
                            error: None,
                            telemetry_tx,
                            session_id,
                            node_id,
                        };

                        let callback_data = (&raw mut callback_ctx).cast::<c_void>();

                        // Call plugin's process function (BLOCKING - but we're in spawn_blocking)
                        let result = (api.process_packet)(
                            handle,
                            pin_cstr.as_ptr(),
                            &raw const packet_repr.packet,
                            output_callback_shim,
                            callback_data,
                            Some(telemetry_callback_shim),
                            callback_data,
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
                        state.finish_call();
                        (outputs, error)
                    })
                    .await
                    .map_err(|e| {
                        StreamKitError::Runtime(format!("Plugin processing task panicked: {e}"))
                    })?;

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
        let (tick_interval, max_ticks) = self
            .state
            .api()
            .get_source_config
            .and_then(|get_source_config_fn| {
                self.state.begin_call().map(|h| {
                    let cfg = get_source_config_fn(h);
                    self.state.finish_call();
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
            // via spawn_blocking to avoid blocking the tokio worker —
            // consistent with how tick_fn and other C ABI calls are made.
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
                        let _ = tokio::task::spawn_blocking(move || {
                            for c_str in &pending_hints {
                                if let Some(handle) = state.begin_call() {
                                    let _ = on_hint_fn(handle, c_str.as_ptr());
                                    state.finish_call();
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
            let outcome = tokio::task::spawn_blocking(move || {
                let Some(handle) = state.begin_call() else {
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
                };

                let callback_data = (&raw mut callback_ctx).cast::<c_void>();

                let result = tick_fn(
                    handle,
                    output_callback_shim,
                    callback_data,
                    Some(telemetry_callback_shim),
                    callback_data,
                );

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
                state.finish_call();

                TickOutcome {
                    outputs,
                    success: result.result.success,
                    done: result.done,
                    error_msg,
                }
            })
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Source tick task panicked: {e}")))?;

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
        let node_name_owned = node_name.to_string();
        let error_msg = tokio::task::spawn_blocking(move || {
            let handle = state.begin_call()?;

            let _lib = Arc::clone(&state.library);
            let api = state.api();
            let result = (api.update_params)(handle, params_cstr.as_ptr());

            let error = if result.success {
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
            };

            state.finish_call();
            error
        })
        .await
        .map_err(|e| StreamKitError::Runtime(format!("Update params task panicked: {e}")))?;

        if let Some(err) = error_msg {
            warn!(node = %node_name_owned, error = %err, "Parameter update failed");
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
}

/// C callback function for sending output packets
/// This collects packets and they are sent asynchronously after the callback returns
extern "C" fn output_callback_shim(
    pin_name: *const std::os::raw::c_char,
    c_packet: *const CPacket,
    user_data: *mut c_void,
) -> CResult {
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
            return CResult::error(std::ptr::null());
        },
    };

    // SAFETY: c_packet is a valid pointer to CPacket provided by the plugin.
    let packet = match unsafe { conversions::packet_from_c(c_packet) } {
        Ok(p) => p,
        Err(e) => {
            ctx.error = Some(format!("Failed to convert packet: {e}"));
            return CResult::error(std::ptr::null());
        },
    };

    // Store packet for async sending after callback returns
    ctx.output_packets.push((pin_str, packet));

    CResult::success()
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

    let event =
        TelemetryEvent::new(ctx.session_id.clone(), ctx.node_id.clone(), event_data, timestamp_us);

    if tx.try_send(event).is_err() {
        // Drop silently: best-effort.
    }

    CResult::success()
}
