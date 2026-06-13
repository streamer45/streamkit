// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! WASM node wrapper that implements the ProcessorNode trait

use crate::bindings::Plugin;
use crate::{arm_epoch_deadline, rearm_call_deadline, wit_types, HostState};
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
    call_timeout: std::time::Duration,
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
        call_timeout: std::time::Duration,
    ) -> Self {
        Self { component, metadata, params, engine, linker, max_memory_bytes, call_timeout }
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
        let Self {
            component,
            metadata: _metadata,
            params,
            engine,
            linker,
            max_memory_bytes,
            call_timeout,
        } = *self;

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
            call_deadline: None,
        };

        let mut store = Store::new(&engine, host_state);
        store.limiter(|s| &mut s.limits);
        arm_epoch_deadline(&mut store, call_timeout);

        // Instantiate the component
        let instance = match linker.instantiate_async(&mut store, &component).await {
            Ok(instance) => instance,
            Err(e) => {
                let err =
                    StreamKitError::Configuration(format!("Failed to instantiate plugin: {e:#}"));
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
        rearm_call_deadline(&mut store, call_timeout);
        let instance_handle = match instance_iface
            .call_constructor(&mut store, initial_params_json.as_deref())
            .await
        {
            Ok(handle) => {
                tracing::debug!(node = %node_id, "Plugin constructor succeeded");
                handle
            },
            Err(e) => {
                let err = StreamKitError::Configuration(format!("Plugin construct error: {e:#}"));
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

                            rearm_call_deadline(&mut store, call_timeout);
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
                                        "Plugin update_params invocation error: {e:#}"
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

                            rearm_call_deadline(&mut store, call_timeout);
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
        rearm_call_deadline(&mut store, call_timeout);
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

#[cfg(test)]
// Tests rely on expect/unwrap to fail fast with readable assertion context.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use streamkit_core::types::Packet;

    #[test]
    fn serialize_params_to_json_returns_none_for_none_input() {
        let out = serialize_params_to_json(None).expect("must succeed");
        assert!(out.is_none());
    }

    #[test]
    fn serialize_params_to_json_normalizes_null_to_none() {
        let null = serde_json::Value::Null;
        let out = serialize_params_to_json(Some(&null)).expect("must succeed");
        assert!(
            out.is_none(),
            "JSON null should normalize to None so plugins fall back to defaults"
        );
    }

    #[test]
    fn serialize_params_to_json_serializes_object_value() {
        let value = serde_json::json!({"gain": 1.5, "muted": false});
        let out = serialize_params_to_json(Some(&value))
            .expect("must succeed")
            .expect("non-null object yields Some");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed, value);
    }

    #[test]
    fn serialize_params_to_json_serializes_primitive_value() {
        let value = serde_json::json!(42);
        let out = serialize_params_to_json(Some(&value))
            .expect("must succeed")
            .expect("non-null primitive yields Some");
        assert_eq!(out, "42");
    }

    #[tokio::test]
    async fn receive_from_any_input_returns_none_when_no_inputs() {
        let mut inputs: Vec<(String, tokio::sync::mpsc::Receiver<Packet>)> = Vec::new();
        assert!(receive_from_any_input(&mut inputs).await.is_none());
    }

    #[tokio::test]
    async fn receive_from_any_input_yields_pending_packet_with_pin_name() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(Packet::Text(Arc::from("hello"))).await.expect("send succeeds");
        let mut inputs = vec![("in".to_string(), rx)];

        let (pin, packet) =
            receive_from_any_input(&mut inputs).await.expect("packet should be ready");
        assert_eq!(pin, "in");
        assert!(matches!(packet, Packet::Text(t) if t.as_ref() == "hello"));
    }

    #[tokio::test]
    async fn receive_from_any_input_drops_closed_inputs_and_returns_none() {
        // Two receivers, both immediately closed (senders dropped).
        let (tx1, rx1) = tokio::sync::mpsc::channel::<Packet>(1);
        let (tx2, rx2) = tokio::sync::mpsc::channel::<Packet>(1);
        drop(tx1);
        drop(tx2);
        let mut inputs = vec![("a".to_string(), rx1), ("b".to_string(), rx2)];

        assert!(receive_from_any_input(&mut inputs).await.is_none());
        assert!(inputs.is_empty(), "closed receivers must be drained out of the input vector");
    }

    #[tokio::test]
    async fn receive_from_any_input_skips_closed_input_and_returns_from_live_input() {
        let (tx_closed, rx_closed) = tokio::sync::mpsc::channel::<Packet>(1);
        drop(tx_closed);
        let (tx_live, rx_live) = tokio::sync::mpsc::channel::<Packet>(1);
        tx_live.send(Packet::Text(Arc::from("from-live"))).await.expect("send succeeds");

        let mut inputs = vec![("closed".to_string(), rx_closed), ("live".to_string(), rx_live)];

        let (pin, packet) =
            receive_from_any_input(&mut inputs).await.expect("live input must yield packet");
        assert_eq!(pin, "live");
        assert!(matches!(packet, Packet::Text(t) if t.as_ref() == "from-live"));
        assert_eq!(inputs.len(), 1, "closed input must be removed");
        assert_eq!(inputs[0].0, "live");
    }

    use crate::{PluginRuntime, PluginRuntimeConfig};
    use streamkit_core::{types::PacketType as CorePacketType, ProcessorNode};

    // Compile the trivial component against the SAME engine the wrapper
    // is constructed with. wasmtime rejects cross-engine Component +
    // Engine pairs at instantiation time with an opaque error, so reusing
    // one engine throughout keeps these helpers safe to extend with
    // happy-path lifecycle tests later.
    fn empty_component(engine: &wasmtime::Engine) -> wasmtime::component::Component {
        wasmtime::component::Component::new(engine, b"(component)")
            .expect("trivial WAT component must compile")
    }

    fn runtime_parts() -> (wasmtime::Engine, Arc<wasmtime::component::Linker<crate::HostState>>) {
        // Borrow the linker/engine the production code uses so the wrapper's
        // construction path matches what the host sets up at runtime.
        let runtime = PluginRuntime::new(PluginRuntimeConfig::default())
            .expect("default runtime config must initialize");
        let engine = runtime.engine_for_test();
        let linker = runtime.linker_for_test();
        (engine, linker)
    }

    fn metadata_with_pins(
        inputs: Vec<wit_types::InputPin>,
        outputs: Vec<wit_types::OutputPin>,
    ) -> wit_types::NodeMetadata {
        wit_types::NodeMetadata {
            kind: "test-node".to_string(),
            inputs,
            outputs,
            param_schema: String::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn new_stores_constructor_arguments_and_input_output_pins_round_trip() {
        let (engine, linker) = runtime_parts();
        let metadata = metadata_with_pins(
            vec![
                wit_types::InputPin {
                    name: "audio_in".to_string(),
                    accepts_types: vec![wit_types::PacketType::RawAudio(wit_types::AudioFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        sample_format: wit_types::SampleFormat::Float32,
                    })],
                },
                wit_types::InputPin {
                    name: "text_in".to_string(),
                    accepts_types: vec![wit_types::PacketType::Text, wit_types::PacketType::Any],
                },
            ],
            vec![wit_types::OutputPin {
                name: "out".to_string(),
                produces_type: wit_types::PacketType::Binary,
            }],
        );

        let component = empty_component(&engine);
        let wrapper = WasmNodeWrapper::new(
            component,
            metadata,
            Some(serde_json::json!({"gain": 0.5})),
            engine,
            linker,
            32 * 1024 * 1024,
            crate::DEFAULT_CALL_TIMEOUT,
        );

        let inputs = wrapper.input_pins();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "audio_in");
        assert_eq!(inputs[0].accepts_types.len(), 1);
        assert!(matches!(inputs[0].accepts_types[0], CorePacketType::RawAudio(_)));
        assert_eq!(inputs[0].cardinality, streamkit_core::PinCardinality::One);

        assert_eq!(inputs[1].name, "text_in");
        assert_eq!(inputs[1].accepts_types.len(), 2);
        assert!(matches!(inputs[1].accepts_types[0], CorePacketType::Text));
        assert!(matches!(inputs[1].accepts_types[1], CorePacketType::Any));

        let outputs = wrapper.output_pins();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "out");
        assert!(matches!(outputs[0].produces_type, CorePacketType::Binary));
        assert_eq!(outputs[0].cardinality, streamkit_core::PinCardinality::Broadcast);
    }

    #[test]
    fn pins_methods_return_empty_vectors_for_metadata_without_pins() {
        let (engine, linker) = runtime_parts();
        let component = empty_component(&engine);
        let wrapper = WasmNodeWrapper::new(
            component,
            metadata_with_pins(Vec::new(), Vec::new()),
            None,
            engine,
            linker,
            8 * 1024 * 1024,
            crate::DEFAULT_CALL_TIMEOUT,
        );
        assert!(wrapper.input_pins().is_empty());
        assert!(wrapper.output_pins().is_empty());
    }

    #[test]
    fn input_pins_preserves_multiple_accepts_types_per_pin_in_declared_order() {
        let (engine, linker) = runtime_parts();
        let pin = wit_types::InputPin {
            name: "multi".to_string(),
            accepts_types: vec![
                wit_types::PacketType::RawAudio(wit_types::AudioFormat {
                    sample_rate: 16_000,
                    channels: 1,
                    sample_format: wit_types::SampleFormat::S16Le,
                }),
                wit_types::PacketType::OpusAudio,
                wit_types::PacketType::Custom("plugin::custom/x@1".to_string()),
            ],
        };
        let component = empty_component(&engine);
        let wrapper = WasmNodeWrapper::new(
            component,
            metadata_with_pins(vec![pin], Vec::new()),
            None,
            engine,
            linker,
            16 * 1024 * 1024,
            crate::DEFAULT_CALL_TIMEOUT,
        );

        let inputs = wrapper.input_pins();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].accepts_types.len(), 3);
        match &inputs[0].accepts_types[0] {
            CorePacketType::RawAudio(fmt) => {
                assert_eq!(fmt.sample_rate, 16_000);
                assert_eq!(fmt.channels, 1);
                assert_eq!(fmt.sample_format, streamkit_core::types::SampleFormat::S16Le);
            },
            other => panic!("expected RawAudio first, got {other:?}"),
        }
        assert!(matches!(inputs[0].accepts_types[1], CorePacketType::EncodedAudio(_)));
        match &inputs[0].accepts_types[2] {
            CorePacketType::Custom { type_id } => {
                assert_eq!(type_id, "plugin::custom/x@1");
            },
            other => panic!("expected Custom variant, got {other:?}"),
        }
    }
}
