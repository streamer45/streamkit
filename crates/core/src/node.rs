// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Core node abstractions and ProcessorNode trait.
//!
//! This module defines the fundamental interface for processing nodes:
//! - [`ProcessorNode`]: The core trait that all nodes must implement
//! - [`NodeContext`]: Runtime context passed to nodes during execution
//! - [`InitContext`]: Context for asynchronous initialization
//! - [`OutputSender`]: Handle for sending packets to downstream nodes

use crate::control::NodeControlMessage;
use crate::error::StreamKitError;
use crate::pins::{InputPin, OutputPin, PinManagementMessage, PinUpdate};
use crate::state::NodeStateUpdate;
use crate::stats::NodeStatsUpdate;
use crate::telemetry::TelemetryEvent;
use crate::types::{Packet, PacketType};
use crate::view_data::NodeViewDataUpdate;
use crate::{AudioFramePool, VideoFramePool};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

/// The execution mode of the pipeline a node is running in.
///
/// Nodes may use this to adjust their behaviour — for example, a compositor
/// can skip real-time tick pacing in [`Oneshot`](PipelineMode::Oneshot) mode
/// to maximise throughput, while still draining to the latest frame in
/// [`Dynamic`](PipelineMode::Dynamic) mode for low-latency live compositing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PipelineMode {
    /// Long-running dynamic pipeline (real-time processing).
    #[default]
    Dynamic,
    /// Oneshot / batch pipeline (process as fast as possible).
    Oneshot,
}

/// Message type for routed packet delivery.
/// Uses `Arc<str>` for node and pin names to avoid heap allocations on every send.
pub type RoutedPacketMessage = (Arc<str>, Arc<str>, Packet);

/// An enum representing the two ways a node's output can be routed.
#[derive(Clone)]
pub enum OutputRouting {
    /// Packets are sent directly to the input channels of downstream nodes.
    Direct(HashMap<String, mpsc::Sender<Packet>>),
    /// Packets are sent to a central engine actor for routing.
    /// Uses Arc<str> for node/pin names to avoid heap allocations on every packet.
    Routed(mpsc::Sender<RoutedPacketMessage>),
}

/// A handle given to a node to send its output packets.
#[derive(Clone)]
pub struct OutputSender {
    /// Node name as Arc<str> to avoid cloning allocations
    node_name: Arc<str>,
    routing: OutputRouting,
    /// Cached pin names as Arc<str> to avoid repeated allocations
    pin_name_cache: HashMap<String, Arc<str>>,
}

/// Error returned by [`OutputSender::send`] when a packet cannot be delivered.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OutputSendError {
    /// The requested output pin does not exist on this node.
    #[error("unknown output pin '{pin_name}' on node '{node_name}'")]
    PinNotFound { node_name: String, pin_name: String },

    /// The downstream channel (direct) or engine channel (routed) is closed.
    #[error("output channel closed for pin '{pin_name}' on node '{node_name}'")]
    ChannelClosed { node_name: String, pin_name: String },

    /// The downstream channel is full (non-blocking send).
    #[error("output channel full for pin '{pin_name}' on node '{node_name}'")]
    ChannelFull { node_name: String, pin_name: String },
}

impl OutputSender {
    /// Creates a new OutputSender.
    /// Note: The node_name String is converted to Arc<str> for efficient cloning on the hot path.
    pub fn new(node_name: String, routing: OutputRouting) -> Self {
        Self { node_name: Arc::from(node_name), routing, pin_name_cache: HashMap::new() }
    }

    /// Returns the node's name.
    #[must_use]
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Get or cache the pin name as Arc<str> to avoid repeated allocations.
    fn get_cached_pin_name(&mut self, pin_name: &str) -> Arc<str> {
        if let Some(cached) = self.pin_name_cache.get(pin_name) {
            cached.clone() // O(1) Arc clone
        } else {
            let arc_name: Arc<str> = Arc::from(pin_name);
            self.pin_name_cache.insert(pin_name.to_string(), arc_name.clone());
            arc_name
        }
    }

    /// Non-blocking send from a specific output pin.
    ///
    /// Returns [`OutputSendError::ChannelFull`] when the downstream channel
    /// has no capacity — callers may drop the packet and continue.
    /// Returns [`OutputSendError::ChannelClosed`] or [`OutputSendError::PinNotFound`]
    /// for permanent errors — callers should stop processing.
    ///
    /// Used by real-time nodes (e.g. compositor) that prefer dropping a frame
    /// over stalling and accumulating latency.
    pub fn try_send(&mut self, pin_name: &str, packet: Packet) -> Result<(), OutputSendError> {
        use tokio::sync::mpsc::error::TrySendError;

        // Cache the pin name up front so the mutable borrow is released
        // before we immutably borrow `self.routing` in the match below.
        let cached_pin = self.get_cached_pin_name(pin_name);

        match &self.routing {
            OutputRouting::Direct(senders) => {
                if let Some(sender) = senders.get(pin_name) {
                    match sender.try_send(packet) {
                        Ok(()) => {},
                        Err(TrySendError::Full(_)) => {
                            return Err(OutputSendError::ChannelFull {
                                node_name: self.node_name.to_string(),
                                pin_name: pin_name.to_string(),
                            });
                        },
                        Err(TrySendError::Closed(_)) => {
                            return Err(OutputSendError::ChannelClosed {
                                node_name: self.node_name.to_string(),
                                pin_name: pin_name.to_string(),
                            });
                        },
                    }
                } else {
                    return Err(OutputSendError::PinNotFound {
                        node_name: self.node_name.to_string(),
                        pin_name: pin_name.to_string(),
                    });
                }
            },
            OutputRouting::Routed(engine_tx) => {
                let message = (self.node_name.clone(), cached_pin, packet);
                match engine_tx.try_send(message) {
                    Ok(()) => {},
                    Err(TrySendError::Full(_)) => {
                        return Err(OutputSendError::ChannelFull {
                            node_name: self.node_name.to_string(),
                            pin_name: pin_name.to_string(),
                        });
                    },
                    Err(TrySendError::Closed(_)) => {
                        return Err(OutputSendError::ChannelClosed {
                            node_name: self.node_name.to_string(),
                            pin_name: pin_name.to_string(),
                        });
                    },
                }
            },
        }
        Ok(())
    }

    /// Sends a packet from a specific output pin of this node.
    /// Returns `Ok(())` if sent successfully.
    ///
    /// Nodes should stop processing when this returns an error, as it indicates
    /// either a programming mistake (unknown pin) or that the pipeline is shutting down.
    ///
    /// # Errors
    ///
    /// Returns [`OutputSendError::PinNotFound`] if the pin doesn't exist, or
    /// [`OutputSendError::ChannelClosed`] if the receiving channel is closed.
    pub async fn send(&mut self, pin_name: &str, packet: Packet) -> Result<(), OutputSendError> {
        use tokio::sync::mpsc::error::TrySendError;

        match &self.routing {
            OutputRouting::Direct(senders) => {
                if let Some(sender) = senders.get(pin_name) {
                    // Fast path: avoid allocating/awaiting a future if the channel has capacity.
                    match sender.try_send(packet) {
                        Ok(()) => {},
                        Err(TrySendError::Full(packet)) => {
                            if sender.send(packet).await.is_err() {
                                // This is expected during cancellation/shutdown, so use debug level
                                tracing::debug!(
                                    "Directly connected channel for pin '{}' is closed.",
                                    pin_name
                                );
                                return Err(OutputSendError::ChannelClosed {
                                    node_name: self.node_name.to_string(),
                                    pin_name: pin_name.to_string(),
                                });
                            }
                        },
                        Err(TrySendError::Closed(_packet)) => {
                            // This is expected during cancellation/shutdown, so use debug level
                            tracing::debug!(
                                "Directly connected channel for pin '{}' is closed.",
                                pin_name
                            );
                            return Err(OutputSendError::ChannelClosed {
                                node_name: self.node_name.to_string(),
                                pin_name: pin_name.to_string(),
                            });
                        },
                    }
                } else {
                    // Pin not found - this is a programming error, log warning and return error
                    tracing::warn!(
                        "OutputSender::send() called with unknown pin '{}' on node '{}'. \
                         Available pins: {:?}. Packet dropped.",
                        pin_name,
                        self.node_name,
                        senders.keys().collect::<Vec<_>>()
                    );
                    return Err(OutputSendError::PinNotFound {
                        node_name: self.node_name.to_string(),
                        pin_name: pin_name.to_string(),
                    });
                }
            },
            OutputRouting::Routed(engine_tx) => {
                // Clone engine_tx first to release the immutable borrow on self,
                // allowing us to call get_cached_pin_name() which needs &mut self
                let engine_tx = engine_tx.clone();

                // Use cached Arc<str> for node and pin names to avoid heap allocations
                let cached_pin = self.get_cached_pin_name(pin_name);
                let message = (self.node_name.clone(), cached_pin, packet);
                match engine_tx.try_send(message) {
                    Ok(()) => {},
                    Err(TrySendError::Full(message)) => {
                        if engine_tx.send(message).await.is_err() {
                            tracing::warn!("Engine channel is closed. Cannot send packet.");
                            return Err(OutputSendError::ChannelClosed {
                                node_name: self.node_name.to_string(),
                                pin_name: pin_name.to_string(),
                            });
                        }
                    },
                    Err(TrySendError::Closed(_message)) => {
                        tracing::warn!("Engine channel is closed. Cannot send packet.");
                        return Err(OutputSendError::ChannelClosed {
                            node_name: self.node_name.to_string(),
                            pin_name: pin_name.to_string(),
                        });
                    },
                }
            },
        }
        Ok(())
    }
}

/// Context provided to nodes during initialization.
///
/// This allows nodes to perform async operations (like probing external resources)
/// before the pipeline starts executing.
pub struct InitContext {
    /// The node's unique identifier in the pipeline
    pub node_id: String,
    /// Channel to report state changes during initialization
    pub state_tx: tokio::sync::mpsc::Sender<NodeStateUpdate>,
}

/// The context provided by the engine to a node when it is run.
pub struct NodeContext {
    pub inputs: HashMap<String, mpsc::Receiver<Packet>>,
    /// The [`PacketType`] that each connected input pin will receive, keyed by
    /// pin name.  Populated by the graph builder from the upstream node's
    /// output type so that nodes can make decisions based on the connected
    /// media type without having to inspect packets at runtime.
    ///
    /// Only contains entries for *connected* pins (unconnected pins are absent).
    /// May be empty for dynamic pipelines where connections are made after the
    /// node is already running.
    pub input_types: HashMap<String, PacketType>,
    pub control_rx: mpsc::Receiver<NodeControlMessage>,
    pub output_sender: OutputSender,
    pub batch_size: usize,
    /// Channel for the node to report state changes.
    /// Nodes should send updates when transitioning between states to enable
    /// monitoring and debugging. It's acceptable if sends fail (e.g., in stateless
    /// pipelines where state tracking may not be enabled).
    pub state_tx: mpsc::Sender<NodeStateUpdate>,
    /// Channel for the node to report statistics updates.
    /// Nodes should throttle these updates (e.g., every 10s or 1000 packets)
    /// to prevent overloading the monitoring system. Like state_tx, it's
    /// acceptable if sends fail.
    pub stats_tx: Option<mpsc::Sender<NodeStatsUpdate>>,
    /// Channel for the node to emit telemetry events.
    /// Telemetry is best-effort and should never block audio processing.
    /// Nodes should use `try_send()` or the `TelemetryEmitter` helper which
    /// handles rate limiting and drop accounting automatically.
    pub telemetry_tx: Option<mpsc::Sender<TelemetryEvent>>,
    /// Session ID for gateway registration and routing (if applicable)
    pub session_id: Option<String>,
    /// Cancellation token for coordinated shutdown of pipeline tasks.
    /// When this token is cancelled, nodes should stop processing and exit gracefully.
    /// This is primarily used in stateless pipelines to abort processing when the
    /// client disconnects or the request is interrupted.
    pub cancellation_token: Option<tokio_util::sync::CancellationToken>,
    /// Channel for runtime pin management messages (Tier 2).
    /// Always provided in dynamic pipelines so the engine can deliver
    /// [`PinManagementMessage::InputTypeResolved`] to every node.
    /// Dynamic-pin nodes additionally receive `AddedInputPin`,
    /// `RemoveInputPin`, etc. through this channel.
    /// `None` in oneshot/static pipelines (type info is delivered via
    /// [`NodeContext::input_types`] at build time).
    pub pin_management_rx: Option<mpsc::Receiver<PinManagementMessage>>,
    /// Optional per-pipeline audio buffer pool for hot-path allocations.
    ///
    /// Nodes that produce audio frames (decoders, resamplers, mixers) may use this to
    /// amortize `Vec<f32>` allocations. If `None`, nodes should fall back to allocating.
    pub audio_pool: Option<Arc<AudioFramePool>>,
    /// Optional per-pipeline video buffer pool for hot-path allocations.
    ///
    /// Nodes that produce video frames (decoders, scalers, compositors) may use this to
    /// amortize `Vec<u8>` allocations. If `None`, nodes should fall back to allocating.
    pub video_pool: Option<Arc<VideoFramePool>>,
    /// The execution mode of the pipeline this node is running in.
    ///
    /// Nodes can use this to adjust behaviour — e.g. skip real-time
    /// pacing in [`PipelineMode::Oneshot`] for maximum throughput.
    pub pipeline_mode: PipelineMode,
    /// Channel for the node to emit structured view data for frontend consumption.
    /// Like stats_tx, this is optional and best-effort.
    pub view_data_tx: Option<mpsc::Sender<NodeViewDataUpdate>>,
    /// Optional sender for engine-level control messages.
    ///
    /// Allows nodes to send [`EngineControlMessage`] to the engine actor,
    /// enabling cross-node control (e.g. sending `UpdateParams` to a sibling
    /// node by name via [`EngineControlMessage::TuneNode`]).
    ///
    /// Only provided in dynamic pipelines.  `None` in oneshot/static
    /// pipelines where the graph is fixed at build time.
    pub engine_control_tx: Option<mpsc::Sender<crate::control::EngineControlMessage>>,
}

impl NodeContext {
    /// Retrieves an input pin receiver by name, returning an error if not found.
    /// This is a convenience method to avoid repeated error handling boilerplate.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Runtime` if the requested input pin doesn't exist.
    pub fn take_input(&mut self, pin_name: &str) -> Result<mpsc::Receiver<Packet>, StreamKitError> {
        self.inputs.remove(pin_name).ok_or_else(|| {
            StreamKitError::Runtime(format!("Engine did not provide '{pin_name}' pin receiver"))
        })
    }

    /// Send an `UpdateParams` control message to a sibling node by name.
    ///
    /// This is a convenience wrapper around [`EngineControlMessage::TuneNode`]
    /// that routes through the engine actor's control channel — the same path
    /// the WebSocket/REST API uses.
    ///
    /// Only works in dynamic pipelines (where `engine_control_tx` is `Some`).
    ///
    /// # Errors
    ///
    /// Returns a [`StreamKitError::Runtime`] if the engine control channel is
    /// unavailable (oneshot pipeline) or closed (engine shut down).
    pub async fn tune_sibling(
        &self,
        target_node_id: &str,
        params: serde_json::Value,
    ) -> Result<(), StreamKitError> {
        let tx = self.engine_control_tx.as_ref().ok_or_else(|| {
            StreamKitError::Runtime(
                "engine_control_tx not available (oneshot pipeline?)".to_string(),
            )
        })?;
        tx.send(crate::control::EngineControlMessage::TuneNode {
            node_id: target_node_id.to_string(),
            message: crate::control::NodeControlMessage::UpdateParams(params),
        })
        .await
        .map_err(|_| StreamKitError::Runtime("engine control channel closed".to_string()))
    }

    /// Receives a packet from the given receiver, respecting the cancellation token if present.
    /// Returns None if cancelled or if the channel is closed.
    ///
    /// This is a convenience method that should be used in node loops instead of calling recv()
    /// directly, as it automatically handles cancellation for stateless pipelines.
    pub async fn recv_with_cancellation(&self, rx: &mut mpsc::Receiver<Packet>) -> Option<Packet> {
        if let Some(token) = &self.cancellation_token {
            tokio::select! {
                () = token.cancelled() => {
                    tracing::debug!(
                        node = %self.output_sender.node_name(),
                        "recv_with_cancellation: cancelled by token"
                    );
                    None
                }
                packet = rx.recv() => {
                    if packet.is_none() {
                        tracing::debug!(
                            node = %self.output_sender.node_name(),
                            "recv_with_cancellation: input channel closed"
                        );
                    }
                    packet
                }
            }
        } else {
            rx.recv().await
        }
    }
}

/// The fundamental trait for any processing node, designed as an actor.
#[async_trait]
pub trait ProcessorNode: Send + Sync {
    /// Returns the input pins for this specific node instance.
    fn input_pins(&self) -> Vec<InputPin>;

    /// Returns the output pins for this specific node instance.
    fn output_pins(&self) -> Vec<OutputPin>;

    /// For nodes that produce a final, self-contained file format, this method
    /// should return the appropriate MIME type string.
    fn content_type(&self) -> Option<String> {
        None // Default implementation for nodes that don't produce a final format.
    }

    /// Tier 1: Initialization-time discovery.
    ///
    /// Called after instantiation but before pipeline execution.
    /// Allows nodes to probe external resources and finalize pin definitions.
    ///
    /// Default implementation does nothing (static pins).
    ///
    /// # Example
    /// ```ignore
    /// async fn initialize(&mut self, ctx: &InitContext) -> Result<PinUpdate, StreamKitError> {
    ///     // Probe external resource
    ///     let tracks = probe_broadcast(&self.url).await?;
    ///
    ///     // Update pins based on discovery
    ///     self.tracks = tracks;
    ///     Ok(PinUpdate::Updated {
    ///         inputs: self.input_pins(),
    ///         outputs: self.output_pins(),
    ///     })
    /// }
    /// ```
    async fn initialize(&mut self, _ctx: &InitContext) -> Result<PinUpdate, StreamKitError> {
        Ok(PinUpdate::NoChange)
    }

    /// Return a runtime-discovered param schema after initialization.
    ///
    /// Plugins whose tunable parameters depend on runtime configuration
    /// (e.g., properties discovered after compiling a `.slint` file) can
    /// override this to return a JSON Schema fragment.  The engine will
    /// deep-merge it with the static `param_schema` from registration
    /// and deliver the enriched schema to the UI.
    ///
    /// The returned value should be a JSON Schema `"type": "object"` with
    /// a `"properties"` map.  Each property can include `"tunable": true`
    /// and an optional `"path"` override for dot-notation addressing.
    ///
    /// **Called once** — the engine queries this immediately after
    /// [`initialize`](Self::initialize) and caches the result for the
    /// lifetime of the node.  There is currently no mechanism to refresh
    /// the schema at runtime; if the underlying configuration changes
    /// (e.g. a different `.slint` file), the node must be re-created.
    ///
    /// Default: `None` (use static schema only).
    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Tier 2: Runtime pin management capability.
    ///
    /// Returns true if this node supports adding/removing pins while running.
    /// Nodes that return true must handle PinManagementMessage messages.
    ///
    /// Default implementation returns false (static pins after init).
    fn supports_dynamic_pins(&self) -> bool {
        false
    }

    /// The main actor loop for the node. The engine will spawn this method as a task.
    async fn run(self: Box<Self>, context: NodeContext) -> Result<(), StreamKitError>;
}

/// A factory function that creates a new instance of a node, accepting optional configuration.
/// Wrapped in an Arc to make it cloneable.
pub type NodeFactory = Arc<
    dyn Fn(Option<&serde_json::Value>) -> Result<Box<dyn ProcessorNode>, StreamKitError>
        + Send
        + Sync,
>;

/// A factory function that computes a hash of parameters for resource caching.
///
/// Given parameters, returns a deterministic hash string used as part of the ResourceKey.
/// Plugins should hash only the parameters that affect resource initialization (e.g., model path, GPU settings).
pub type ResourceKeyHasher = Arc<dyn Fn(Option<&serde_json::Value>) -> String + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[test]
    fn pipeline_mode_default_is_dynamic() {
        assert_eq!(PipelineMode::default(), PipelineMode::Dynamic);
    }

    #[test]
    fn pipeline_mode_variants_are_distinct() {
        assert_ne!(PipelineMode::Dynamic, PipelineMode::Oneshot);
    }

    #[test]
    fn output_sender_node_name() {
        let routing = OutputRouting::Direct(HashMap::new());
        let sender = OutputSender::new("test_node".into(), routing);
        assert_eq!(sender.node_name(), "test_node");
    }

    #[test]
    fn output_sender_try_send_pin_not_found() {
        let routing = OutputRouting::Direct(HashMap::new());
        let mut sender = OutputSender::new("node_a".into(), routing);
        let packet = Packet::Text(Arc::from("hello"));
        let err = sender.try_send("missing_pin", packet).unwrap_err();
        assert!(matches!(err, OutputSendError::PinNotFound { .. }));
        assert!(err.to_string().contains("missing_pin"));
        assert!(err.to_string().contains("node_a"));
    }

    #[test]
    fn output_sender_try_send_direct_success() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut senders = HashMap::new();
        senders.insert("out".to_string(), tx);
        let routing = OutputRouting::Direct(senders);
        let mut sender = OutputSender::new("node_a".into(), routing);

        let packet = Packet::Text(Arc::from("hello"));
        sender.try_send("out", packet).unwrap();

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, Packet::Text(ref s) if &**s == "hello"));
    }

    #[test]
    fn output_sender_try_send_direct_channel_full() {
        let (tx, _rx) = mpsc::channel(1);
        let mut senders = HashMap::new();
        senders.insert("out".to_string(), tx);
        let routing = OutputRouting::Direct(senders);
        let mut sender = OutputSender::new("node_a".into(), routing);

        sender.try_send("out", Packet::Text(Arc::from("1"))).unwrap();
        let err = sender.try_send("out", Packet::Text(Arc::from("2"))).unwrap_err();
        assert!(matches!(err, OutputSendError::ChannelFull { .. }));
    }

    #[test]
    fn output_sender_try_send_direct_channel_closed() {
        let (tx, rx) = mpsc::channel(4);
        let mut senders = HashMap::new();
        senders.insert("out".to_string(), tx);
        let routing = OutputRouting::Direct(senders);
        let mut sender = OutputSender::new("node_a".into(), routing);
        drop(rx);

        let err = sender.try_send("out", Packet::Text(Arc::from("x"))).unwrap_err();
        assert!(matches!(err, OutputSendError::ChannelClosed { .. }));
    }

    #[test]
    fn output_sender_try_send_routed_success() {
        let (engine_tx, mut engine_rx) = mpsc::channel(4);
        let routing = OutputRouting::Routed(engine_tx);
        let mut sender = OutputSender::new("source".into(), routing);

        sender.try_send("video_out", Packet::Text(Arc::from("frame"))).unwrap();

        let (node_name, pin_name, _packet) = engine_rx.try_recv().unwrap();
        assert_eq!(&*node_name, "source");
        assert_eq!(&*pin_name, "video_out");
    }

    #[test]
    fn output_sender_try_send_routed_closed() {
        let (engine_tx, rx) = mpsc::channel(4);
        let routing = OutputRouting::Routed(engine_tx);
        let mut sender = OutputSender::new("source".into(), routing);
        drop(rx);

        let err = sender.try_send("out", Packet::Text(Arc::from("x"))).unwrap_err();
        assert!(matches!(err, OutputSendError::ChannelClosed { .. }));
    }

    #[test]
    fn output_send_error_display_messages() {
        let err = OutputSendError::PinNotFound { node_name: "N".into(), pin_name: "P".into() };
        assert!(err.to_string().contains("N"));
        assert!(err.to_string().contains("P"));

        let err = OutputSendError::ChannelClosed { node_name: "N".into(), pin_name: "P".into() };
        assert!(err.to_string().contains("closed"));

        let err = OutputSendError::ChannelFull { node_name: "N".into(), pin_name: "P".into() };
        assert!(err.to_string().contains("full"));
    }

    #[test]
    fn output_send_error_clone_and_eq() {
        let err = OutputSendError::PinNotFound { node_name: "n".into(), pin_name: "p".into() };
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn init_context_field_access() {
        let (state_tx, _rx) = mpsc::channel(4);
        let ctx = InitContext { node_id: "my_node".into(), state_tx };
        assert_eq!(ctx.node_id, "my_node");
    }

    #[test]
    fn output_sender_pin_name_caching() {
        let (engine_tx, mut engine_rx) = mpsc::channel(16);
        let routing = OutputRouting::Routed(engine_tx);
        let mut sender = OutputSender::new("node".into(), routing);

        sender.try_send("pin_a", Packet::Text(Arc::from("1"))).unwrap();
        sender.try_send("pin_a", Packet::Text(Arc::from("2"))).unwrap();

        let (_, pin1, _) = engine_rx.try_recv().unwrap();
        let (_, pin2, _) = engine_rx.try_recv().unwrap();
        assert!(Arc::ptr_eq(&pin1, &pin2));
    }
}
