// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use opentelemetry::{global, KeyValue};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use streamkit_core::control::NodeControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_core::frame_pool::{AudioFramePool, VideoFramePool};
use streamkit_core::node::{InitContext, NodeContext, OutputRouting, OutputSender, ProcessorNode};
use streamkit_core::packet_meta::{can_connect, packet_type_registry};
use streamkit_core::pins::PinUpdate;
use streamkit_core::state::{NodeState, NodeStateUpdate, StopReason};
use streamkit_core::stats::NodeStatsUpdate;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::PinCardinality;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::constants::{DEFAULT_ONESHOT_CONTROL_CAPACITY, DEFAULT_STATE_CHANNEL_CAPACITY};

/// A handle to a live, running node.
pub struct LiveNode {
    pub control_tx: mpsc::Sender<NodeControlMessage>,
    pub task_handle: JoinHandle<Result<(), StreamKitError>>,
}

/// Wires up and spawns all nodes for a given pipeline definition.
///
/// # Errors
///
/// Returns an error on node init failure, incompatible pin types, or missing connections.
///
/// # Panics
///
/// Panics if a connection references a node not present in the map.
#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::implicit_hasher,
    clippy::too_many_arguments
)]
pub async fn wire_and_spawn_graph(
    mut nodes: HashMap<String, Box<dyn ProcessorNode>>,
    connections: &[crate::Connection],
    node_kinds: &HashMap<String, String>,
    batch_size: usize,
    media_channel_capacity: usize,
    state_tx: Option<mpsc::Sender<NodeStateUpdate>>,
    stats_tx: Option<mpsc::Sender<NodeStatsUpdate>>,
    cancellation_token: Option<tokio_util::sync::CancellationToken>,
    audio_pool: Option<Arc<AudioFramePool>>,
    video_pool: Option<Arc<VideoFramePool>>,
    asset_root: std::path::PathBuf,
    node_attributes: Arc<crate::ResolvedAttributes>,
) -> Result<HashMap<String, LiveNode>, StreamKitError> {
    tracing::info!(
        "Graph builder starting with {} nodes and {} connections",
        nodes.len(),
        connections.len()
    );

    // NOTE: The stateless/oneshot engine currently only supports linear pipelines (no fan-out).
    // Without an output router/fanout distributor, multiple edges from the same output pin would
    // silently drop all but one downstream connection. Fail fast to make this constraint explicit.
    let mut outgoing_counts: HashMap<(String, String), usize> = HashMap::new();
    for conn in connections {
        *outgoing_counts.entry((conn.from_node.clone(), conn.from_pin.clone())).or_insert(0) += 1;
    }
    if let Some(((node_id, pin_name), count)) = outgoing_counts.into_iter().find(|(_, c)| *c > 1) {
        return Err(StreamKitError::Configuration(format!(
            "Oneshot pipelines must be linear: output pin '{node_id}.{pin_name}' has {count} outgoing connections (fan-out not supported yet)"
        )));
    }

    let (init_state_tx, _init_state_rx) = mpsc::channel(DEFAULT_STATE_CHANNEL_CAPACITY);
    let init_state_tx = state_tx.clone().unwrap_or(init_state_tx);

    for (node_id, node) in &mut nodes {
        let init_ctx = InitContext { node_id: node_id.clone(), state_tx: init_state_tx.clone() };

        match node.initialize(&init_ctx).await {
            Ok(PinUpdate::NoChange) => {
                tracing::debug!("Node '{}' initialized with no pin changes", node_id);
            },
            Ok(PinUpdate::Updated { inputs, outputs }) => {
                tracing::info!(
                    "Node '{}' updated pins during initialization: {} inputs, {} outputs",
                    node_id,
                    inputs.len(),
                    outputs.len()
                );
            },
            Err(e) => {
                tracing::error!("Node '{}' failed to initialize: {}", node_id, e);
                return Err(e);
            },
        }
    }

    let mut output_txs: HashMap<(String, String), mpsc::Sender<Packet>> = HashMap::new();
    let mut input_rxs: HashMap<(String, String), mpsc::Receiver<Packet>> = HashMap::new();

    let registry = packet_type_registry();
    let mut out_pin_types: HashMap<(String, String), PacketType> = HashMap::new();
    let mut in_pin_accepts: HashMap<(String, String), Vec<PacketType>> = HashMap::new();
    let mut in_pin_cardinality: HashMap<(String, String), PinCardinality> = HashMap::new();

    for (name, node) in &nodes {
        for pin in node.output_pins() {
            out_pin_types.insert((name.clone(), pin.name.clone()), pin.produces_type.clone());
        }
        for pin in node.input_pins() {
            in_pin_accepts.insert((name.clone(), pin.name.clone()), pin.accepts_types.clone());
            in_pin_cardinality.insert((name.clone(), pin.name.clone()), pin.cardinality.clone());
        }
    }

    // Resolve Passthrough types by tracing inputs backward.
    let mut connections_by_to: HashMap<(String, String), Vec<&crate::Connection>> = HashMap::new();
    for conn in connections {
        connections_by_to
            .entry((conn.to_node.clone(), conn.to_pin.clone()))
            .or_default()
            .push(conn);
    }

    let mut changed = true;
    let mut iteration = 0;
    while changed && iteration < 100 {
        changed = false;
        iteration += 1;

        let mut updates: Vec<((String, String), PacketType)> = Vec::new();

        for ((node_name, pin_name), pin_type) in &out_pin_types {
            if matches!(pin_type, PacketType::Passthrough) {
                let input_pins = nodes.get(node_name).map(|n| n.input_pins()).unwrap_or_default();

                let mut found = false;
                for input_pin in input_pins {
                    if let Some(source_conns) =
                        connections_by_to.get(&(node_name.clone(), input_pin.name.clone()))
                    {
                        for source_conn in source_conns {
                            if let Some(source_type) = out_pin_types
                                .get(&(source_conn.from_node.clone(), source_conn.from_pin.clone()))
                            {
                                if !matches!(source_type, PacketType::Passthrough) {
                                    tracing::debug!(
                                        "Resolved Passthrough type for {}.{} to {:?} (from {}.{})",
                                        node_name,
                                        pin_name,
                                        source_type,
                                        source_conn.from_node,
                                        source_conn.from_pin
                                    );
                                    updates.push((
                                        (node_name.clone(), pin_name.clone()),
                                        source_type.clone(),
                                    ));
                                    found = true;
                                    break;
                                }
                            }
                        }
                        if found {
                            break;
                        }
                    }
                }
            }
        }

        for ((node_name, pin_name), resolved_type) in updates {
            out_pin_types.insert((node_name, pin_name), resolved_type);
            changed = true;
        }
    }

    if iteration >= 100 {
        tracing::warn!("Type inference reached maximum iterations (100), some Passthrough types may remain unresolved");
    }

    let mut input_types: HashMap<(String, String), PacketType> = HashMap::new();

    for conn in connections {
        tracing::debug!(
            "Creating connection: {}.{} -> {}.{}",
            conn.from_node,
            conn.from_pin,
            conn.to_node,
            conn.to_pin
        );
        let out_ty = out_pin_types
            .get(&(conn.from_node.clone(), conn.from_pin.clone()))
            .ok_or_else(|| {
                let err_msg = format!(
                    "Unknown output pin '{}.{}' referenced by connection",
                    conn.from_node, conn.from_pin
                );
                tracing::error!("{}", err_msg);
                StreamKitError::Configuration(err_msg)
            })?;
        let in_accepts =
            in_pin_accepts.get(&(conn.to_node.clone(), conn.to_pin.clone())).ok_or_else(|| {
                let err_msg = format!(
                    "Unknown input pin '{}.{}' referenced by connection",
                    conn.to_node, conn.to_pin
                );
                tracing::error!("{}", err_msg);
                StreamKitError::Configuration(err_msg)
            })?;

        let compatible =
            in_accepts.iter().any(|accepts_ty| can_connect(out_ty, accepts_ty, registry));
        if !compatible {
            let err_msg = format!(
                "Incompatible connection: {}.{} ({:?}) -> {}.{} (accepts {:?})",
                conn.from_node, conn.from_pin, out_ty, conn.to_node, conn.to_pin, in_accepts
            );
            tracing::error!("{}", err_msg);
            return Err(StreamKitError::Configuration(err_msg));
        }

        let (tx, rx) = mpsc::channel(media_channel_capacity);
        let from_key = (conn.from_node.clone(), conn.from_pin.clone());
        let to_key = (conn.to_node.clone(), conn.to_pin.clone());

        let in_cardinality =
            in_pin_cardinality.get(&to_key).cloned().unwrap_or(PinCardinality::One);

        if input_rxs.contains_key(&to_key) {
            match in_cardinality {
                PinCardinality::One => {
                    tracing::error!(
                        "Input pin '{}.{}' (cardinality: One) already has a connection",
                        conn.to_node,
                        conn.to_pin
                    );
                    return Err(StreamKitError::Configuration(format!(
                        "Input pin '{}.{}' (cardinality: One) cannot accept multiple connections",
                        conn.to_node, conn.to_pin
                    )));
                },
                PinCardinality::Broadcast => {
                    tracing::error!(
                        "Input pin '{}.{}' has Broadcast cardinality, which is only valid for outputs",
                        conn.to_node,
                        conn.to_pin
                    );
                    return Err(StreamKitError::Configuration(format!(
                        "Input pin '{}.{}' incorrectly uses Broadcast cardinality",
                        conn.to_node, conn.to_pin
                    )));
                },
                PinCardinality::Dynamic { .. } => {
                    tracing::error!(
                        "Input pin '{}.{}' has Dynamic cardinality but multiple static connections",
                        conn.to_node,
                        conn.to_pin
                    );
                    return Err(StreamKitError::Configuration(format!(
                        "Input pin '{}.{}' with Dynamic cardinality should not have static connections",
                        conn.to_node, conn.to_pin
                    )));
                },
            }
        }

        input_types.insert(to_key.clone(), out_ty.clone());

        output_txs.insert(from_key, tx);
        input_rxs.insert(to_key, rx);
    }

    tracing::debug!(
        "Created {} output channels and {} input channels",
        output_txs.len(),
        input_rxs.len()
    );

    let mut live_nodes = HashMap::new();
    let node_names: Vec<String> = nodes.keys().cloned().collect();

    for name in node_names {
        tracing::debug!("Spawning node '{}'", name);
        #[allow(clippy::unwrap_used)]
        let node = nodes.remove(&name).unwrap();
        let mut node_inputs = HashMap::new();
        let mut node_input_types = HashMap::new();
        let input_pins = node.input_pins();
        tracing::debug!("Node '{}' has {} input pins", name, input_pins.len());

        for pin in input_pins {
            let key = (name.clone(), pin.name.clone());
            if let Some(rx) = input_rxs.remove(&key) {
                tracing::debug!("Connected input pin '{}.{}'", name, pin.name);
                if let Some(ty) = input_types.remove(&key) {
                    node_input_types.insert(pin.name.clone(), ty);
                }
                node_inputs.insert(pin.name, rx);
            } else {
                tracing::debug!("Input pin '{}.{}' not connected", name, pin.name);
            }
        }

        let mut direct_outputs = HashMap::new();
        let output_pins = node.output_pins();
        tracing::debug!("Node '{}' has {} output pins", name, output_pins.len());

        for pin in output_pins {
            if let Some(tx) = output_txs.get(&(name.clone(), pin.name.clone())) {
                tracing::debug!("Connected output pin '{}.{}'", name, pin.name);
                direct_outputs.insert(pin.name, tx.clone());
            } else {
                tracing::debug!("Output pin '{}.{}' not connected", name, pin.name);
            }
        }

        let (control_tx, control_rx) = mpsc::channel(DEFAULT_ONESHOT_CONTROL_CAPACITY);

        let (tx, rx) = mpsc::channel(DEFAULT_STATE_CHANNEL_CAPACITY);
        let (node_state_tx, _dummy_rx) = (tx, Some(rx));

        let context = NodeContext {
            inputs: node_inputs,
            input_types: node_input_types,
            control_rx,
            output_sender: OutputSender::new(name.clone(), OutputRouting::Direct(direct_outputs)),
            batch_size,
            state_tx: node_state_tx.clone(),
            stats_tx: stats_tx.clone(),
            telemetry_tx: None,
            session_id: None,
            cancellation_token: cancellation_token.clone(),
            pin_management_rx: None,
            audio_pool: audio_pool.clone(),
            video_pool: video_pool.clone(),
            pipeline_mode: streamkit_core::PipelineMode::Oneshot,
            view_data_tx: None,
            engine_control_tx: None,
            asset_root: asset_root.clone(),
        };

        tracing::debug!("Starting task for node '{}'", name);
        let kind = node_kinds.get(&name).cloned().unwrap_or_else(|| "unknown".to_string());

        let node_metric_attrs = node_attributes.for_node(&name);
        let name_for_span = name.clone();
        let kind_for_span = kind.clone();
        let name_for_hashmap = name.clone();
        let name_for_debug = name.clone();
        let name_for_state = name.clone();
        let state_tx_clone = state_tx.clone();

        let task_handle = tokio::spawn(
            async move {
                let start_time = Instant::now();
                let result = node.run(context).await;
                let duration = start_time.elapsed();

                match &result {
                    Ok(()) => tracing::debug!(
                        node_id = %name,
                        ?duration,
                        "node task completed successfully",
                    ),
                    Err(e) => tracing::warn!(
                        node_id = %name,
                        ?duration,
                        error = %e,
                        "node task failed",
                    ),
                }

                let meter = global::meter("skit_engine");
                let histogram = meter
                    .f64_histogram("node.execution.duration")
                    .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_NODE_EXECUTION.to_vec())
                    .build();
                let status = if result.is_ok() { "ok" } else { "error" };

                let mut labels = vec![
                    KeyValue::new("node_id", name.clone()),
                    KeyValue::new("node_kind", kind.clone()),
                    KeyValue::new("status", status),
                ];
                labels.extend(node_metric_attrs.iter().cloned());
                histogram.record(duration.as_secs_f64(), &labels);

                let final_state = match &result {
                    Ok(()) => NodeState::Stopped { reason: StopReason::Completed },
                    Err(e) => NodeState::Failed { reason: e.to_string() },
                };

                let _ = node_state_tx.send(NodeStateUpdate {
                    node_id: name_for_state.clone(),
                    state: final_state.clone(),
                    timestamp: SystemTime::now(),
                }).await;

                if let Some(global_tx) = state_tx_clone {
                    let _ = global_tx.send(NodeStateUpdate {
                        node_id: name_for_state,
                        state: final_state,
                        timestamp: SystemTime::now(),
                    }).await;
                }

                result
            }
            .instrument(tracing::info_span!("node_run", node.name = %name_for_span, node.kind = %kind_for_span)),
        );
        live_nodes.insert(name_for_hashmap, LiveNode { control_tx, task_handle });
        tracing::debug!("Successfully spawned node '{}'", name_for_debug);
    }

    tracing::info!("Successfully spawned {} live nodes", live_nodes.len());
    Ok(live_nodes)
}
