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
use crate::types::Packet;
use crate::AudioFramePool;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

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
    /// Only provided for nodes that support dynamic pins.
    pub pin_management_rx: Option<mpsc::Receiver<PinManagementMessage>>,
    /// Optional per-pipeline audio buffer pool for hot-path allocations.
    ///
    /// Nodes that produce audio frames (decoders, resamplers, mixers) may use this to
    /// amortize `Vec<f32>` allocations. If `None`, nodes should fall back to allocating.
    pub audio_pool: Option<Arc<AudioFramePool>>,
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

    /// Receives a packet from the given receiver, respecting the cancellation token if present.
    /// Returns None if cancelled or if the channel is closed.
    ///
    /// This is a convenience method that should be used in node loops instead of calling recv()
    /// directly, as it automatically handles cancellation for stateless pipelines.
    pub async fn recv_with_cancellation(&self, rx: &mut mpsc::Receiver<Packet>) -> Option<Packet> {
        if let Some(token) = &self.cancellation_token {
            tokio::select! {
                () = token.cancelled() => None,
                packet = rx.recv() => packet,
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
