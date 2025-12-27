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

    // The run method is complex by necessity - it's an async actor managing FFI calls,
    // control messages, and packet processing. Breaking it up would make the logic harder to follow.
    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();

        tracing::info!(node = %node_name, "Native plugin wrapper starting");

        // Emit initializing state
        if let Err(e) = context
            .state_tx
            .send(NodeStateUpdate::new(node_name.clone(), NodeState::Initializing))
            .await
        {
            warn!(error = %e, node = %node_name, "Failed to send initializing state");
        }

        tracing::debug!(node = %node_name, "Getting input channel");

        // Get input channel
        let mut input_rx = context.take_input("in").map_err(|e| {
            tracing::error!(node = %node_name, error = %e, "Failed to get input channel");
            StreamKitError::Runtime(format!("Failed to get input channel: {e}"))
        })?;

        tracing::debug!(node = %node_name, "Got input channel, entering main loop");

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
                            // spawn_blocking can only fail with JoinError if the task panics.
                            // If that happens, it's a serious bug that should crash.
                            #[allow(clippy::expect_used)]
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
                            // spawn_blocking only panics if the task panics, which indicates a serious bug
                            .expect("Update params task panicked");

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

                maybe_packet = input_rx.recv() => {
                    let Some(packet) = maybe_packet else {
                        // Input closed - flush any buffered data before shutting down
                        tracing::debug!(node = %node_name, "Native plugin input closed, flushing buffers");

                        // Call flush to process any remaining buffered data
                        let state = Arc::clone(&self.state);
                        let telemetry_tx = context.telemetry_tx.clone();
                        let session_id = context.session_id.clone();
                        let node_id = node_name.clone();

                        #[allow(clippy::expect_used)]
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
                        .expect("Plugin flush task panicked");

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
                    // spawn_blocking can only fail with JoinError if the task panics.
                    // If that happens, it's a serious bug that should crash.
                    #[allow(clippy::expect_used)]
                    let (outputs, error) = tokio::task::spawn_blocking(move || {
                        let Some(handle) = state.begin_call() else {
                            return (Vec::new(), None);
                        };

                        let _lib = Arc::clone(&state.library);
                        let api = state.api();
                        // Convert packet to C representation
                        let packet_repr = conversions::packet_to_c(&packet);

                        // Prepare input pin name - hardcoded ASCII string "in" can never contain null bytes
                        #[allow(clippy::expect_used)]
                        let pin_cstr = CString::new("in").expect("Hardcoded ASCII string is always valid C string");

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
                    // spawn_blocking only panics if the task panics, which indicates a serious bug
                    .expect("Plugin processing task panicked");

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

                return Err(StreamKitError::Runtime(error_msg));
            }
                }
            }
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
