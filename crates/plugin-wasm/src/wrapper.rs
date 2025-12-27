// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! WASM node wrapper that implements the ProcessorNode trait

use crate::bindings::Plugin;
use crate::{wit_types, HostState};
use async_trait::async_trait;
use futures::future::poll_fn;
use std::{sync::Arc, task::Poll};
use streamkit_core::control::NodeControlMessage;
use streamkit_core::{
    state_helpers::emit_state, InputPin, NodeContext, NodeState, OutputPin, PinCardinality,
    ProcessorNode, StopReason, StreamKitError,
};
use tokio::sync::Mutex;
use wasmtime::component::{Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimitsBuilder};
use wasmtime_wasi::WasiCtx;

/// Wraps a WASM component to implement the ProcessorNode trait
pub struct WasmNodeWrapper {
    component: wasmtime::component::Component,
    metadata: wit_types::NodeMetadata,
    params: Option<serde_json::Value>,
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    max_memory_bytes: usize,
}

impl WasmNodeWrapper {
    // Cannot be const: wasmtime types (Component, Engine) and Arc are not const-constructible
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(
        component: wasmtime::component::Component,
        metadata: wit_types::NodeMetadata,
        params: Option<serde_json::Value>,
        engine: Engine,
        linker: Arc<Linker<HostState>>,
        max_memory_bytes: usize,
    ) -> Self {
        Self { component, metadata, params, engine, linker, max_memory_bytes }
    }
}

#[async_trait]
impl ProcessorNode for WasmNodeWrapper {
    fn input_pins(&self) -> Vec<InputPin> {
        self.metadata
            .inputs
            .iter()
            .map(|pin| InputPin {
                name: pin.name.clone(),
                accepts_types: pin
                    .accepts_types
                    .iter()
                    .map(streamkit_core::types::PacketType::from)
                    .collect(),
                cardinality: PinCardinality::One,
            })
            .collect()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        self.metadata
            .outputs
            .iter()
            .map(|pin| OutputPin {
                name: pin.name.clone(),
                produces_type: streamkit_core::types::PacketType::from(&pin.produces_type),
                cardinality: PinCardinality::Broadcast,
            })
            .collect()
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let Self { component, metadata: _metadata, params, engine, linker, max_memory_bytes } =
            *self;

        let node_id = context.output_sender.node_name().to_string();
        tracing::info!(node = %node_id, "WASM plugin node starting");
        emit_state(&context.state_tx, &node_id, NodeState::Initializing);
        let state_tx_clone = context.state_tx.clone();

        // Create WASI context
        let wasi = WasiCtx::builder().inherit_stdio().build();

        // Create host state with output sender
        let output_sender = Arc::new(Mutex::new(context.output_sender.clone()));
        let host_state = HostState {
            wasi,
            resource_table: ResourceTable::new(),
            output_sender: Some(output_sender),
            limits: StoreLimitsBuilder::new().memory_size(max_memory_bytes).build(),
        };

        let mut store = Store::new(&engine, host_state);
        store.limiter(|s| &mut s.limits);

        // Instantiate the component
        let instance = match linker.instantiate_async(&mut store, &component).await {
            Ok(instance) => instance,
            Err(e) => {
                let err =
                    StreamKitError::Configuration(format!("Failed to instantiate plugin: {e}"));
                emit_state(
                    &state_tx_clone,
                    &node_id,
                    NodeState::Failed { reason: err.to_string() },
                );
                return Err(err);
            },
        };

        let plugin = match Plugin::new(&mut store, &instance) {
            Ok(plugin) => plugin,
            Err(e) => {
                let err =
                    StreamKitError::Configuration(format!("Failed to bind plugin interface: {e}"));
                emit_state(
                    &state_tx_clone,
                    &node_id,
                    NodeState::Failed { reason: err.to_string() },
                );
                return Err(err);
            },
        };

        let node = plugin.streamkit_plugin_node();

        let initial_params_json = match serialize_params_to_json(params.as_ref()) {
            Ok(json) => json,
            Err(err) => {
                emit_state(
                    &state_tx_clone,
                    &node_id,
                    NodeState::Failed { reason: err.to_string() },
                );
                return Err(err);
            },
        };

        // Access the resource interface for `node-instance`
        let instance_iface = node.node_instance();

        tracing::debug!(node = %node_id, "Calling plugin constructor");

        // Construct a new stateful instance in the plugin with parameters
        let instance_handle =
            match instance_iface.call_constructor(&mut store, initial_params_json.as_deref()).await
            {
                Ok(handle) => {
                    tracing::debug!(node = %node_id, "Plugin constructor succeeded");
                    handle
                },
                Err(e) => {
                    let err = StreamKitError::Configuration(format!("Plugin construct error: {e}"));
                    tracing::error!(node = %node_id, error = %e, "Plugin constructor failed");
                    emit_state(
                        &state_tx_clone,
                        &node_id,
                        NodeState::Failed { reason: err.to_string() },
                    );
                    return Err(err);
                },
            };

        tracing::info!(node = %node_id, "Plugin instance created, entering main loop");
        emit_state(&state_tx_clone, &node_id, NodeState::Running);

        // Convert inputs to a vector so we can poll them efficiently with tokio
        let mut inputs: Vec<(String, tokio::sync::mpsc::Receiver<streamkit_core::types::Packet>)> =
            context.inputs.into_iter().collect();

        let mut control_channel_open = true;

        // Main processing loop
        loop {
            tokio::select! {
                biased;

                maybe_control = context.control_rx.recv(), if control_channel_open => {
                    match maybe_control {
                        Some(NodeControlMessage::UpdateParams(params_value)) => {
                            let params_json = match serialize_params_to_json(Some(&params_value)) {
                                Ok(json) => json,
                                Err(err) => {
                                    emit_state(
                                        &state_tx_clone,
                                        &node_id,
                                        NodeState::Failed {
                                            reason: err.to_string(),
                                        },
                                    );
                                    return Err(err);
                                }
                            };

                            match instance_iface
                                .call_update_params(&mut store, instance_handle, params_json.as_deref())
                                .await
                            {
                                Ok(Ok(())) => {
                                    if matches!(params_value, serde_json::Value::Null) {
                                        tracing::debug!("Plugin parameters reset to defaults");
                                    } else {
                                        tracing::debug!("Plugin parameters updated");
                                    }
                                }
                                Ok(Err(e)) => {
                                    let err = StreamKitError::Configuration(format!(
                                        "Plugin rejected params update: {e}"
                                    ));
                                    emit_state(
                                        &state_tx_clone,
                                        &node_id,
                                        NodeState::Failed {
                                            reason: err.to_string(),
                                        },
                                    );
                                    return Err(err);
                                }
                                Err(e) => {
                                    let err = StreamKitError::Configuration(format!(
                                        "Plugin update_params invocation error: {e}"
                                    ));
                                    emit_state(
                                        &state_tx_clone,
                                        &node_id,
                                        NodeState::Failed {
                                            reason: err.to_string(),
                                        },
                                    );
                                    return Err(err);
                                }
                            }
                        }
                        Some(NodeControlMessage::Start) => {
                            // WASM plugins don't implement ready/start lifecycle - ignore
                        }
                        Some(NodeControlMessage::Shutdown) => {
                            tracing::info!("WASM plugin received shutdown signal");
                            break;
                        }
                        None => {
                            control_channel_open = false;
                        }
                    }
                }

                maybe_input = receive_from_any_input(&mut inputs) => {
                    match maybe_input {
                        Some((input_pin, packet)) => {
                            let wit_packet: wit_types::Packet = packet.into();

                            match instance_iface
                                .call_process(&mut store, instance_handle, &input_pin, &wit_packet)
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    let err = StreamKitError::Runtime(format!(
                                        "Plugin process failed: {e}"
                                    ));
                                    tracing::error!(
                                        node = %node_id,
                                        error = %err,
                                        "Plugin returned error from process()"
                                    );
                                    emit_state(
                                        &state_tx_clone,
                                        &node_id,
                                        NodeState::Failed {
                                            reason: err.to_string(),
                                        },
                                    );
                                    return Err(err);
                                }
                                Err(e) => {
                                    // This catches WASM traps/panics
                                    let err_string = format!("{e:?}");
                                    let err = StreamKitError::Runtime(format!(
                                        "Plugin process error (WASM trap/panic): {err_string}"
                                    ));
                                    tracing::error!(
                                        node = %node_id,
                                        error = %err_string,
                                        backtrace = ?e.source(),
                                        "Plugin WASM trap/panic in process()"
                                    );
                                    emit_state(
                                        &state_tx_clone,
                                        &node_id,
                                        NodeState::Failed {
                                            reason: err.to_string(),
                                        },
                                    );
                                    return Err(err);
                                }
                            }
                        }
                        None => {
                            // All inputs closed
                            break;
                        }
                    }
                }
            }
        }

        // Clean up
        if let Err(e) = instance_iface.call_cleanup(&mut store, instance_handle).await {
            tracing::warn!("Plugin cleanup error: {}", e);
        }

        emit_state(
            &state_tx_clone,
            &node_id,
            NodeState::Stopped { reason: StopReason::InputClosed },
        );

        Ok(())
    }
}

fn serialize_params_to_json(
    value: Option<&serde_json::Value>,
) -> Result<Option<String>, StreamKitError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => serde_json::to_string(v).map(Some).map_err(|e| {
            StreamKitError::Configuration(format!("Failed to serialize plugin params as JSON: {e}"))
        }),
    }
}

/// Helper to receive from any available input pin
async fn receive_from_any_input(
    inputs: &mut Vec<(String, tokio::sync::mpsc::Receiver<streamkit_core::types::Packet>)>,
) -> Option<(String, streamkit_core::types::Packet)> {
    loop {
        if inputs.is_empty() {
            return None;
        }

        let polled = poll_fn(|cx| {
            for (idx, (_pin, rx)) in inputs.iter_mut().enumerate() {
                match rx.poll_recv(cx) {
                    Poll::Ready(Some(packet)) => return Poll::Ready(Some(Ok((idx, packet)))),
                    Poll::Ready(None) => return Poll::Ready(Some(Err(idx))),
                    Poll::Pending => {},
                }
            }

            Poll::Pending
        })
        .await;

        match polled {
            Some(Ok((idx, packet))) => {
                let pin_name = inputs[idx].0.clone();
                return Some((pin_name, packet));
            },
            Some(Err(idx)) => {
                inputs.swap_remove(idx);
            },
            None => return None,
        }
    }
}
